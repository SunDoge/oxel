use super::*;

type OuterBinding<'a> = dyn FnMut(&str, Span) -> Option<Binding> + 'a;

pub(super) struct CaptureScope<'a, 'src> {
    scopes: &'a [Scope<'src>],
    nonnull: &'a [Value],
    outer: &'a mut OuterBinding<'a>,
    captured_cells: &'a mut HashSet<u16>,
    uses: Vec<Expr>,
}

impl CaptureScope<'_, '_> {
    pub(super) fn binding(&mut self, name: &str, span: Span) -> Option<Binding> {
        let binding = self
            .scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
            .or_else(|| (self.outer)(name, span))?;
        if binding.mutable
            && let Value::Local(index) = binding.value
        {
            self.captured_cells.insert(index);
        }
        let ty = if self.nonnull.contains(&binding.value) {
            binding.ty.non_nullable()
        } else {
            binding.ty
        };
        let slot = self.uses.iter().position(|value| {
            matches!(value.kind,
                ExprKind::Load(captured) | ExprKind::CaptureCell(captured) if captured == binding.value)
        });
        let slot = slot.unwrap_or_else(|| {
            let slot = self.uses.len();
            let (kind, ty) = if binding.mutable {
                (ExprKind::CaptureCell(binding.value), Type::Object)
            } else {
                (ExprKind::Load(binding.value), ty)
            };
            self.uses.push(Expr { kind, ty, span });
            slot
        });
        Some(Binding {
            value: if binding.mutable {
                Value::CapturedCell(slot, ty)
            } else {
                Value::Captured(
                    slot,
                    matches!(
                        binding.value,
                        Value::Parameter(_) | Value::Captured(_, true)
                    ),
                )
            },
            ty,
            mutable: binding.mutable,
            uninitialized: false,
            can_make_mutable: false,
            definition: binding.definition,
        })
    }
}

impl<'src> Checker<'_, 'src> {
    fn delegate_definition(&self, ty: Type) -> Option<usize> {
        self.types
            .ancestor_id(ty.non_nullable())
            .filter(|&id| self.references.imported_type(id).is_delegate)
    }

    pub(super) fn invoke_delegate(
        &mut self,
        receiver: Expr,
        arguments: &[syntax::Expr<'src>],
    ) -> Checked<(ExprKind, Type)> {
        if receiver.ty == Type::Never {
            return Ok((receiver.kind, Type::Never));
        }
        let span = receiver.span;
        if self.delegate_definition(receiver.ty).is_none() {
            let ty = describe_scoped(self.types, self.type_parameters, receiver.ty);
            return Err(Diagnostic::new(
                "E0205",
                span,
                format!("cannot call a value of type {ty}; expected a CLR delegate"),
            ));
        }
        let binding_occurrence = self.occurrences.as_ref().and_then(|occurrences| {
            let occurrence = occurrences.last()?;
            (occurrence.span == span && occurrence.definition.is_some())
                .then_some(occurrences.len() - 1)
        });
        let call = self.instance_call(receiver, "Invoke", span, &[], arguments)?;
        if let (Some(index), Some(occurrences)) = (binding_occurrence, &mut self.occurrences)
            && let Some(offset) = occurrences[index + 1..].iter().position(|occurrence| {
                occurrence.span == span && occurrence.call_parameters.is_some()
            })
        {
            // Attach signature help to this use of the binding and keep its definition.
            let signature = occurrences.remove(index + 1 + offset);
            occurrences[index].call_parameters = signature.call_parameters;
            occurrences[index].detail = signature.detail;
        }
        Ok(call)
    }

    pub(super) fn delegate_signature(
        &self,
        ty: Type,
        span: Span,
    ) -> Checked<Option<(usize, Signature)>> {
        let Some(definition) = self.delegate_definition(ty) else {
            return Ok(None);
        };
        let [invoke] = self.references.members(definition, "Invoke") else {
            return Ok(None);
        };
        let method = self.references.method(*invoke);
        if !method.modes.is_empty() || method.signature.type_arity != 0 {
            return Ok(None);
        }
        self.types
            .method_signature(Member::declared(*invoke, Some(ty.non_nullable())), span)
            .map(|signature| Some((definition, signature)))
    }

    pub(super) fn lambda(
        &mut self,
        source: &syntax::Lambda<'src>,
        expected: Option<Type>,
        span: Span,
    ) -> Checked<(ExprKind, Type)> {
        if self.comptime {
            return Err(Diagnostic::new(
                "E0340",
                span,
                "lambdas require a runtime function",
            ));
        }
        let expected = expected.ok_or_else(|| {
            Diagnostic::new("E0340", span, "a lambda requires a known CLR delegate type")
        })?;
        let (definition, mut signature) =
            self.delegate_signature(expected, span)?.ok_or_else(|| {
                Diagnostic::new(
                    "E0340",
                    span,
                    "a lambda requires a CLR delegate with a supported Invoke signature",
                )
            })?;
        if self.types.stack_only(signature.result) {
            return Err(stack_only_error(span, "a lambda return type"));
        }
        if source.parameters.len() != signature.parameters.len()
            || source.parameters.len() >= usize::from(u16::MAX)
        {
            return Err(Diagnostic::new(
                "E0340",
                span,
                format!(
                    "delegate expects {} lambda parameters, found {}",
                    signature.parameters.len(),
                    source.parameters.len()
                ),
            ));
        }

        let capture = &mut self.capture;
        let mut outer = |name: &str, span| capture.as_mut()?.binding(name, span);
        let mut nested = Checker {
            in_test: self.in_test,
            comptime: false,
            names: self.names,
            functions: self.functions,
            signatures: self.signatures,
            constants: self.constants,
            references: self.references,
            types: self.types,
            result: signature.result,
            type_parameters: self.type_parameters,
            bounds: self.bounds,
            scopes: vec![Scope::Empty],
            locals: Vec::new(),
            uninitialized: Vec::new(),
            loop_depth: 0,
            try_context: None,
            has_exception_regions: false,
            nonnull: Vec::new(),
            lookups: self.lookups.as_deref_mut(),
            occurrences: self.occurrences.as_deref_mut(),
            capture: Some(CaptureScope {
                scopes: &self.scopes,
                nonnull: &self.nonnull,
                outer: &mut outer,
                captured_cells: &mut self.captured_cells,
                uses: Vec::new(),
            }),
            captured_cells: HashSet::new(),
            lambdas: Vec::new(),
        };
        for (index, (&(name, name_span), &ty)) in source
            .parameters
            .iter()
            .zip(&signature.parameters)
            .enumerate()
        {
            if name == "_" {
                continue;
            }
            nested.record_scoped_binding(
                name,
                name_span,
                OccurrenceKind::Parameter,
                ty,
                source.body.interior(),
            );
            let previous = nested.scopes[0].insert(
                name,
                Binding {
                    value: Value::Parameter(index as u16 + 1),
                    ty,
                    mutable: false,
                    uninitialized: false,
                    can_make_mutable: false,
                    definition: name_span,
                },
            );
            if previous.is_some() {
                return Err(Diagnostic::new(
                    "E0340",
                    name_span,
                    format!("duplicate lambda parameter '{name}'"),
                ));
            }
        }
        let body = nested.block(&source.body, Some(signature.result))?;
        let captures = nested.capture.take().expect("active capture scope");
        for capture in &captures.uses {
            let ty = match capture.kind {
                ExprKind::CaptureCell(Value::Local(index)) => self.locals[usize::from(index)],
                ExprKind::CaptureCell(Value::CapturedCell(_, ty)) => ty,
                _ => capture.ty,
            };
            if self.types.stack_only(ty) {
                return Err(stack_only_error(capture.span, "a captured value"));
            }
        }
        if !nested.uninitialized.is_empty() {
            assignment::validate(&body, &nested.uninitialized)?;
        }
        let has_exception_regions = nested.has_exception_regions;
        let locals = nested.locals;
        let lambdas = nested.lambdas;
        let captured_cells = nested.captured_cells;
        let environment_type = self.types.array(Type::NullableObject, span)?;
        signature.parameters.insert(0, environment_type);
        let items = captures
            .uses
            .into_iter()
            .map(|value| self.coerce(value, Type::NullableObject))
            .collect();
        let environment = Expr {
            kind: ExprKind::ArrayLiteral {
                element: Type::NullableObject,
                items,
            },
            ty: environment_type,
            span,
        };
        let method = self.lambdas.len();
        // The synthetic method has the same generic context as its enclosing
        // method. Delegate creation binds it to the current CLR type arguments.
        signature.type_arity = self.type_parameters.len() as u16;
        signature.bounds = self.bounds.to_vec();
        let type_arguments = (0..signature.type_arity)
            .map(Type::MethodParameter)
            .collect();
        self.lambdas.push(Function {
            has_exception_regions,
            internal: true,
            name: "<lambda>",
            span,
            signature,
            type_parameters: self.type_parameters.iter().map(|&(name, _)| name).collect(),
            locals,
            captured_cells,
            body: super::MethodBody::Managed(body),
            lambdas,
        });
        Ok((
            ExprKind::Lambda {
                method,
                definition,
                type_arguments,
                environment: Box::new(environment),
            },
            expected.non_nullable(),
        ))
    }
}
