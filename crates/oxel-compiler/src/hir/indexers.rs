//! Resolves CLR default indexed properties using normal overload resolution.

use super::{
    Assignment, Checked, Checker, Expr, ExprKind, Member, OccurrenceKind, OverloadUse, Signature,
    Span, Structs, Target, Type, Value, describe_scoped,
};
use crate::{Diagnostic, clr::PropertyAccess, syntax};

pub(crate) struct IndexUpdate {
    pub receiver: Expr,
    pub temporary: Option<u16>,
    pub indices: Vec<(u16, Expr)>,
    pub getter: Member,
    pub setter: Member,
    pub previous: u16,
    pub value: Expr,
}

impl Structs<'_, '_> {
    pub(super) fn indexer_signature(
        &self,
        property: Member,
        access: PropertyAccess,
        span: Span,
    ) -> Checked<Option<Signature>> {
        let Some((id, found)) = self.references.indexer_accessor(property.id, access) else {
            return Ok(None);
        };
        let mut signature = self.method_signature(
            Member {
                id,
                owner: property.owner,
            },
            span,
        )?;
        if found == PropertyAccess::Set {
            signature.result = signature.parameters.pop().expect("indexer setter value");
        }
        Ok(Some(signature))
    }

    fn instance_indexers(&self, owner: Type, access: PropertyAccess) -> Vec<Member> {
        let Some(id) = self.ancestor_id(owner) else {
            return Vec::new();
        };
        let interfaces = self.references.imported_type(id).is_interface;
        let mut candidates: Vec<(Type, Member, Vec<Type>)> = Vec::new();
        for ancestor in self.ancestors(owner, interfaces) {
            let id = self.ancestor_id(ancestor).expect("imported ancestor");
            for &property in self.references.declared_indexers(id) {
                let member = Member::declared(property, Some(ancestor));
                let Ok(Some(signature)) = self.indexer_signature(member, access, Span::default())
                else {
                    continue;
                };
                let indices: Vec<_> = signature
                    .parameters
                    .into_iter()
                    .map(Type::non_nullable)
                    .collect();
                // A more-derived declaration hides the same index signature.
                // Keep distinguishable overloads and unrelated interface candidates.
                if candidates.iter().any(|(near, _, parameters)| {
                    *near != ancestor
                        && *parameters == indices
                        && (!interfaces || self.inherits(*near, ancestor))
                }) {
                    continue;
                }
                if interfaces {
                    candidates.retain(|(far, _, parameters)| {
                        *parameters != indices || !self.inherits(ancestor, *far)
                    });
                }
                candidates.push((ancestor, member, indices));
            }
        }
        candidates
            .into_iter()
            .map(|(_, member, _)| member)
            .collect()
    }
}

impl<'src> Checker<'_, 'src> {
    pub(super) fn indexer(
        &mut self,
        receiver: Expr,
        arguments: &[syntax::Expr<'src>],
        span: Span,
        assignment: Option<Assignment<'_, 'src>>,
    ) -> Checked<Expr> {
        if receiver.ty == Type::Never {
            return Ok(receiver);
        }
        self.require_nonnull_receiver(receiver.ty, span)?;
        if receiver.ty == Type::String {
            if assignment.is_some() {
                return Err(Diagnostic::new(
                    "E0204",
                    span,
                    "string elements are read-only",
                ));
            }
            let [argument] = arguments else {
                return Err(Diagnostic::new("E0205", span, "a string takes one index"));
            };
            let index = self.expression(argument, Some(Type::I32))?;
            let ty = if index.ty == Type::Never {
                Type::Never
            } else {
                Type::Char
            };
            return Ok(Expr {
                kind: ExprKind::StringIndex(Box::new(receiver), Box::new(index)),
                ty,
                span,
            });
        }
        if let Some(element) = self.types.array_element(receiver.ty) {
            return self.index_array(receiver, element, arguments, span, assignment);
        }
        self.require_mutable_struct_receiver(&receiver, assignment.is_some(), span)?;
        let access = match assignment {
            Some(Assignment { operator: None, .. }) => PropertyAccess::Set,
            _ => PropertyAccess::Get,
        };
        let name = format!(
            "{} indexer",
            describe_scoped(self.types, self.type_parameters, receiver.ty)
        );
        let candidates = self.bound_members(receiver.ty, |owner| {
            self.types.instance_indexers(owner, access)
        });
        if candidates.is_empty() {
            return Err(Diagnostic::new(
                "E0305",
                span,
                format!("no supported CLR {name}"),
            ));
        }
        let resolved = self.resolve_overload(
            &name,
            span,
            &candidates,
            &[],
            arguments,
            OverloadUse::Indexer(access),
        )?;
        let getter_or_setter = self.require_indexer_accessor(resolved.member, access, span)?;
        let types = self.types;
        let type_parameters = self.type_parameters;
        self.record(span, None, OccurrenceKind::Property, || {
            format!(
                "{name}: {}",
                describe_scoped(types, type_parameters, resolved.result)
            )
        });
        if let Some(Assignment {
            operator: Some(operator),
            value,
        }) = assignment
        {
            let setter =
                self.require_indexer_accessor(resolved.member, PropertyAccess::Set, span)?;
            let signature = self.types.method_signature(setter, span)?;
            let (&value_type, index_types) = signature
                .parameters
                .split_last()
                .expect("indexer setter value");

            let mut indices = Vec::with_capacity(resolved.arguments.len());
            for (index, &expected) in resolved.arguments.into_iter().zip(index_types) {
                if index.ty != Type::Never && index.ty.non_nullable() != expected.non_nullable() {
                    return Err(Diagnostic::new(
                        "E0308",
                        index.span,
                        "CLR indexer accessors have incompatible index types",
                    ));
                }
                self.require(index.ty, expected, index.span)?;
                let local = self.local(expected, index.span)?;
                indices.push((local, index));
            }
            let previous = self.local(resolved.result, span)?;
            let get_result = Expr {
                kind: ExprKind::Load(Value::Local(previous)),
                ty: resolved.result,
                span,
            };
            let value = self.assigned_value(value, Some((operator, get_result)), value_type)?;
            let temporary =
                if receiver.ty.clr_struct() || matches!(receiver.ty, Type::MethodParameter(_)) {
                    self.constrained_temporary(&receiver, true, span)?
                } else {
                    None
                };
            let ty = if value.ty == Type::Never
                || indices.iter().any(|(_, index)| index.ty == Type::Never)
            {
                Type::Never
            } else {
                Type::Void
            };
            return Ok(Expr {
                kind: ExprKind::IndexUpdate(Box::new(IndexUpdate {
                    receiver,
                    temporary,
                    indices,
                    getter: getter_or_setter,
                    setter,
                    previous,
                    value,
                })),
                ty,
                span,
            });
        }
        let mut args = resolved.arguments;
        let result = if let Some(assignment) = assignment {
            args.push(self.expression(assignment.value, Some(resolved.result))?);
            Type::Void
        } else {
            resolved.result
        };
        self.call_property_accessor(getter_or_setter, receiver, args, result, span)
    }

    fn require_indexer_accessor(
        &self,
        property: Member,
        access: PropertyAccess,
        span: Span,
    ) -> Checked<Member> {
        let id = self
            .references
            .accessor(property.id, access)
            .ok_or_else(|| {
                let accessor = match access {
                    PropertyAccess::Get => "getter",
                    PropertyAccess::Set => "setter",
                };
                Diagnostic::new(
                    "E0308",
                    span,
                    format!("CLR indexer has no supported public {accessor}"),
                )
            })?;
        Ok(Member {
            id,
            owner: property.owner,
        })
    }

    pub(super) fn call_property_accessor(
        &mut self,
        method: Member,
        receiver: Expr,
        mut args: Vec<Expr>,
        result: Type,
        span: Span,
    ) -> Checked<Expr> {
        let (kind, ty) =
            if receiver.ty.clr_struct() || matches!(receiver.ty, Type::MethodParameter(_)) {
                self.constrained_call(method, Box::new([]), receiver, args, result, span)?
            } else {
                args.insert(0, receiver);
                self.call_result(
                    Target::External {
                        method: method.id,
                        owner: method.owner,
                        arguments: Box::new([]),
                    },
                    args,
                    result,
                )
            };
        Ok(Expr { kind, ty, span })
    }
}
