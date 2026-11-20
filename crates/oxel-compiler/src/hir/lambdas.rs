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
    pub(super) fn delegate_signature(
        &self,
        ty: Type,
        span: Span,
    ) -> Checked<Option<(usize, Signature)>> {
        let Some(definition) = self.types.ancestor_id(ty.non_nullable()) else {
            return Ok(None);
        };
        if !self.references.imported_type(definition).is_delegate {
            return Ok(None);
        }
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
        if self.comptime || !self.type_parameters.is_empty() {
            return Err(Diagnostic::new(
                "E0340",
                span,
                "lambdas require a non-generic runtime function",
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
            comptime: false,
            names: self.names,
            functions: self.functions,
            signatures: self.signatures,
            constants: self.constants,
            references: self.references,
            types: self.types,
            result: signature.result,
            type_parameters: &[],
            bounds: &[],
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
        self.lambdas.push(Function {
            has_exception_regions,
            internal: true,
            name: "<lambda>",
            span,
            signature,
            type_parameters: Vec::new(),
            locals,
            captured_cells,
            body: super::MethodBody::Managed(body),
            lambdas,
        });
        Ok((
            ExprKind::Lambda {
                method,
                definition,
                environment: Box::new(environment),
            },
            expected.non_nullable(),
        ))
    }
}
