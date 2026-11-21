//! Source interfaces and their explicit, nominal implementations.

use super::*;

pub(crate) struct InterfaceMethod<'src> {
    pub name: &'src str,
    pub span: Span,
    pub signature: Signature,
    pub parameter_names: Vec<Option<Box<str>>>,
}

pub(crate) struct InterfaceImplementation {
    pub ty: Type,
    pub receiver: Type,
    /// Static body helpers in interface slot order.
    pub methods: Vec<ImplementationMethod>,
}

pub(crate) struct ImplementationMethod {
    pub declaration: Declaration,
    pub helper: usize,
    pub signature: Signature,
}

#[derive(Clone, Copy)]
pub(crate) enum Declaration {
    Source(InterfaceMember),
    Clr(Member),
}

struct ContractMethod<'a> {
    declaration: Declaration,
    name: &'a str,
    span: Option<Span>,
    signature: Signature,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct InterfaceMember {
    pub owner: Type,
    pub slot: usize,
}

impl<'src> Structs<'src, '_> {
    pub(super) fn interface_id(&self, ty: Type) -> Option<usize> {
        let Type::Interface(id) = ty.non_nullable() else {
            return None;
        };
        match self.instance(id).kind {
            InstanceKind::Interface(definition) => Some(definition),
            _ => unreachable!("source interface instance"),
        }
    }

    pub(super) fn local_interfaces(&self, ty: Type) -> impl Iterator<Item = Type> + '_ {
        let implementations = self
            .definition_of(ty)
            .map(|id| self.definitions[id].implementations.as_slice())
            .unwrap_or_default();
        let arguments = if implementations.is_empty() {
            None
        } else {
            ty.instance_id().map(|id| self.instance(id).arguments)
        };
        implementations.iter().filter_map(move |implementation| {
            self.specialize(
                implementation.ty,
                arguments.as_deref().unwrap_or_default(),
                ParameterKind::Struct,
                Span::default(),
            )
            .ok()
        })
    }

    fn implementation_types(
        &self,
        item: &syntax::Implementation<'_>,
    ) -> Checked<(usize, Type, Type)> {
        validate_type_parameters(&item.type_parameters, item.target.span)?;
        let receiver = self.scoped_type(&item.target, &item.type_parameters, None, false, true)?;
        let Some(id) = self.definition_of(receiver) else {
            return Err(interface_error(
                item.target.span,
                "an implementation requires an Oxel struct",
            ));
        };
        let arguments = receiver.instance_id().map(|id| self.instance(id).arguments);
        let arguments = arguments.as_deref().unwrap_or_default();
        if arguments.len() != item.type_parameters.len()
            || arguments
                .iter()
                .enumerate()
                .any(|(index, ty)| *ty != Type::TypeParameter(index as u16))
        {
            return Err(interface_error(
                item.target.span,
                "implement the generic struct for its declared type parameters in order",
            ));
        }
        if self.definitions[id].builtin {
            return Err(interface_error(
                item.target.span,
                "a built-in type cannot declare an implementation",
            ));
        }
        let ty = self.scoped_type(&item.interface, &item.type_parameters, None, false, true)?;
        if ty.is_nullable()
            || (!matches!(ty, Type::Interface(_))
                && !self
                    .imported_id(ty)
                    .is_some_and(|id| self.references.imported_type(id).is_interface))
        {
            return Err(interface_error(
                item.interface.span,
                "expected a non-null interface",
            ));
        }
        Ok((id, receiver, ty))
    }

    fn interfaces_may_overlap(&self, left: Type, right: Type) -> bool {
        let (left, right) = (left.non_nullable(), right.non_nullable());
        if left == right
            || matches!(left, Type::TypeParameter(_))
            || matches!(right, Type::TypeParameter(_))
        {
            return true;
        }
        let (Some(left), Some(right)) = (left.instance_id(), right.instance_id()) else {
            return false;
        };
        let (left, right) = (self.instance(left), self.instance(right));
        left.kind == right.kind
            && left
                .arguments
                .iter()
                .zip(right.arguments.iter())
                .all(|(&left, &right)| self.interfaces_may_overlap(left, right))
    }

    pub(super) fn declare_implementations(
        &mut self,
        items: &[&syntax::Implementation<'_>],
    ) -> Checked<()> {
        for item in items {
            let (owner, receiver, ty) = self.implementation_types(item)?;
            if self.definitions[owner]
                .implementations
                .iter()
                .any(|implementation| self.interfaces_may_overlap(implementation.ty, ty))
            {
                return Err(interface_error(
                    item.interface.span,
                    "duplicate or overlapping interface implementation",
                ));
            }
            self.definitions[owner]
                .implementations
                .push(InterfaceImplementation {
                    ty,
                    receiver,
                    methods: Vec::new(),
                });
        }
        Ok(())
    }

    pub(super) fn interface_methods(
        &self,
        declaration: &syntax::TypeDefinition<'src>,
        methods: &[syntax::InterfaceMethod<'src>],
        id: usize,
    ) -> Checked<Vec<InterfaceMethod<'src>>> {
        let definition = &self.definitions[id];
        let mut names = HashSet::new();
        let mut result = Vec::with_capacity(methods.len());
        let annotation = |path: &Path<'_>, allow_void| {
            let ty = self.scoped_type(
                path,
                &declaration.type_parameters,
                Some(&definition.bounds),
                allow_void,
                true,
            )?;
            if !definition.internal {
                self.require_public_type(ty, path.span)?;
            }
            Ok(ty)
        };
        for method in methods {
            if !names.insert(method.name) {
                return Err(interface_error(
                    method.name_span,
                    "duplicate interface method name",
                ));
            }
            let mut parameters = HashSet::new();
            for parameter in &method.parameters {
                if !parameters.insert(parameter.name) {
                    return Err(interface_error(
                        parameter.span,
                        "duplicate interface parameter",
                    ));
                }
            }
            let signature = Signature {
                type_arity: 0,
                bounds: Vec::new(),
                parameters: method
                    .parameters
                    .iter()
                    .map(|param| annotation(&param.annotation, false))
                    .collect::<Checked<_>>()?,
                result: method
                    .result
                    .as_ref()
                    .map_or(Ok(Type::Void), |path| annotation(path, true))?,
            };
            result.push(InterfaceMethod {
                name: method.name,
                span: method.name_span,
                signature,
                parameter_names: method
                    .parameters
                    .iter()
                    .map(|parameter| Some(parameter.name.into()))
                    .collect(),
            });
        }
        Ok(result)
    }

    fn specialized_interface(&self, member: InterfaceMember, span: Span) -> Checked<Signature> {
        let Type::Interface(id) = member.owner else {
            unreachable!("checked source interface");
        };
        let instance = self.instance(id);
        let InstanceKind::Interface(definition) = instance.kind else {
            unreachable!("interface instance");
        };
        let signature = &self.definitions[definition]
            .interface
            .as_ref()
            .expect("interface methods")[member.slot]
            .signature;
        let specialize = |ty| self.specialize(ty, &instance.arguments, ParameterKind::Struct, span);
        Ok(Signature {
            type_arity: 0,
            bounds: Vec::new(),
            parameters: signature
                .parameters
                .iter()
                .copied()
                .map(specialize)
                .collect::<Checked<_>>()?,
            result: specialize(signature.result)?,
        })
    }

    fn interface_method(&self, member: InterfaceMember) -> &InterfaceMethod<'_> {
        let definition = self
            .interface_id(member.owner)
            .expect("checked source interface");
        &self.definitions[definition]
            .interface
            .as_ref()
            .expect("interface methods")[member.slot]
    }

    fn interface_contract(&self, ty: Type, span: Span) -> Checked<Vec<ContractMethod<'_>>> {
        if let Some(id) = self.interface_id(ty) {
            return self.definitions[id]
                .interface
                .as_ref()
                .expect("source interface")
                .iter()
                .enumerate()
                .map(|(slot, method)| {
                    let member = InterfaceMember { owner: ty, slot };
                    Ok(ContractMethod {
                        declaration: Declaration::Source(member),
                        name: method.name,
                        span: Some(method.span),
                        signature: self.specialized_interface(member, span)?,
                    })
                })
                .collect();
        }
        let id = self.imported_id(ty).expect("checked imported interface");
        let methods = self.references.interface_requirements(id).ok_or_else(|| {
            interface_error(
                span,
                "the CLR interface has base interfaces or unsupported requirements",
            )
        })?;
        methods
            .into_iter()
            .map(|id| {
                let method = self.references.method(id);
                if method.signature.type_arity != 0
                    || !method.modes.is_empty()
                    || !syntax::valid_identifier(&method.name)
                {
                    return Err(interface_error(
                        span,
                        format!("unsupported CLR interface method '{}'", method.name),
                    ));
                }
                let member = Member::declared(id, Some(ty));
                Ok(ContractMethod {
                    declaration: Declaration::Clr(member),
                    name: &method.name,
                    span: None,
                    signature: self.method_signature(member, span)?,
                })
            })
            .collect()
    }

    pub(super) fn bind_implementations(
        &mut self,
        items: &[&syntax::Implementation<'_>],
        functions: &[&syntax::Function<'_>],
        signatures: &[Signature],
    ) -> Checked<()> {
        let mut next_body = functions
            .iter()
            .position(|function| function.implementation)
            .unwrap_or(functions.len());
        for item in items {
            let (owner, receiver, ty) = self.implementation_types(item)?;
            if self.definitions[owner].union.is_some() {
                return Err(interface_error(
                    item.target.span,
                    "an implementation requires a struct",
                ));
            }
            let bounds = bounds::resolve(&item.type_parameters, &item.type_bounds, self, true)?;
            let required = &self.definitions[owner].bounds;
            if required.len() != bounds.len()
                || !required.iter().all(|bound| bounds.contains(bound))
            {
                return Err(interface_error(
                    item.target.span,
                    "implementation type bounds must match the struct's bounds",
                ));
            }
            self.scoped_type(
                &item.interface,
                &item.type_parameters,
                Some(&bounds),
                false,
                true,
            )?;
            let helper_arguments: Vec<_> = (0..item.type_parameters.len())
                .map(|index| Type::MethodParameter(index as u16))
                .collect();
            let helper_type = |ty| {
                self.specialize(
                    ty,
                    &helper_arguments,
                    ParameterKind::Struct,
                    item.target.span,
                )
            };
            let helper_receiver = helper_type(receiver)?;
            let methods = self.interface_contract(ty, item.interface.span)?;
            let helper_methods = methods
                .iter()
                .map(|method| {
                    let parameters = method
                        .signature
                        .parameters
                        .iter()
                        .copied()
                        .map(helper_type)
                        .collect::<Checked<Vec<_>>>()?;
                    Ok((parameters, helper_type(method.signature.result)?))
                })
                .collect::<Checked<Vec<_>>>()?;
            let mut bindings: Vec<_> = methods.iter().map(|_| None).collect();
            for method in &item.methods {
                let body = next_body;
                next_body += 1;
                let actual = &signatures[body];
                let named = methods
                    .iter()
                    .enumerate()
                    .filter(|(_, expected)| expected.name == method.name);
                let mut no_match = None;
                let slot = named.into_iter().find_map(|(slot, expected)| {
                    no_match = Some(expected);
                    (matches!(method.parameters.first(), Some(receiver) if receiver.name == "self")
                        && actual.parameters.first() == Some(&helper_receiver)
                        && actual.parameters[1..] == helper_methods[slot].0
                        && actual.result == helper_methods[slot].1)
                        .then_some(slot)
                }).ok_or_else(|| {
                    let Some(expected) = no_match else {
                        return interface_error(method.name_span, format!("'{}' is not a method of this interface", method.name));
                    };
                    let diagnostic = interface_error(
                        method.name_span,
                        format!("'{}' must match the interface signature after a first 'self: {}' parameter", method.name, self.definitions[owner].name),
                    );
                    if let Some(span) = expected.span {
                        diagnostic.with_related(span, "interface method declared here")
                    } else {
                        diagnostic
                    }
                })?;
                if bindings[slot].replace(body).is_some() {
                    return Err(interface_error(
                        method.name_span,
                        "duplicate implementation method",
                    ));
                }
            }
            let bindings = bindings
                .into_iter()
                .zip(methods)
                .map(|(helper, method)| {
                    Ok(ImplementationMethod {
                        declaration: method.declaration,
                        helper: helper.ok_or_else(|| {
                            interface_error(
                                item.interface.span,
                                format!("missing implementation of '{}'", method.name),
                            )
                        })?,
                        signature: method.signature,
                    })
                })
                .collect::<Checked<_>>()?;
            let implementation = self.definitions[owner]
                .implementations
                .iter_mut()
                .find(|implementation| implementation.ty == ty)
                .expect("declared implementation");
            implementation.methods = bindings;
        }
        Ok(())
    }
}

impl<'src> Checker<'_, 'src> {
    pub(super) fn source_interface_members(
        &self,
        receiver: Type,
        method: &str,
    ) -> Vec<InterfaceMember> {
        let mut members = Vec::new();
        let mut check = |ty| {
            if let Some(id) = self.types.interface_id(ty)
                && let Some(slot) = self.types.definitions[id]
                    .interface
                    .as_ref()
                    .expect("checked interface")
                    .iter()
                    .position(|member| member.name == method)
            {
                let member = InterfaceMember { owner: ty, slot };
                if !members.contains(&member) {
                    members.push(member);
                }
            }
        };
        if matches!(receiver, Type::MethodParameter(_)) {
            for bound in bounds::reachable_bounds(receiver, self.bounds) {
                if let TypeBoundKind::Target(ty) = bound {
                    check(ty);
                }
            }
        } else {
            check(receiver);
            for ty in self.types.local_interfaces(receiver) {
                check(ty);
            }
        }
        members
    }

    pub(super) fn call_source_interface(
        &mut self,
        receiver: Expr,
        member: InterfaceMember,
        span: Span,
        type_arguments: &[Path<'src>],
        arguments: &[syntax::Expr<'src>],
    ) -> Checked<(ExprKind, Type)> {
        let signature = self.types.specialized_interface(member, span)?;
        if !type_arguments.is_empty() || signature.parameters.len() != arguments.len() {
            return Err(interface_error(
                span,
                format!(
                    "interface method expects {} arguments and no method type arguments",
                    signature.parameters.len()
                ),
            ));
        }
        if self.occurrences.is_some() {
            let method = self.types.interface_method(member);
            let label = describe_scoped(self.types, self.type_parameters, member.owner);
            let detail = format!("{label}.{}", method.name);
            self.record(span, Some(method.span), OccurrenceKind::Method, || detail);
        }
        let mut args = Vec::with_capacity(arguments.len() + 1);
        let receiver_type = receiver.ty;
        let temporary = if matches!(receiver_type, Type::Interface(_)) {
            args.push(receiver);
            None
        } else {
            // The source receiver is read-only. Evaluate it before the arguments
            // and dispatch through its address without boxing a value type.
            let local = self.local(receiver_type, span)?;
            args.push(Expr {
                kind: ExprKind::LocalAddress(Value::Local(local), ArgumentMode::In),
                ty: receiver_type,
                span: receiver.span,
            });
            Some((local, receiver))
        };
        for (&expected, argument) in signature.parameters.iter().zip(arguments) {
            let value = self.expression(argument, Some(expected))?;
            self.require(value.ty, expected, value.span)?;
            args.push(self.coerce(value, expected));
        }
        let constrained = temporary.as_ref().map(|_| receiver_type);
        let (kind, ty) = self.call_result(
            Target::Interface {
                member,
                constrained,
            },
            args,
            signature.result,
        );
        if let Some((local, receiver)) = temporary {
            let start = Statement::Store {
                span: receiver.span,
                target: Value::Local(local),
                value: receiver,
            };
            Ok((
                ExprKind::Block(Block {
                    statements: vec![start],
                    tail: Some(Box::new(Expr { kind, ty, span })),
                    ty,
                }),
                ty,
            ))
        } else {
            Ok((kind, ty))
        }
    }
}

pub(super) fn interface_error(span: Span, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new("E0360", span, message)
}
