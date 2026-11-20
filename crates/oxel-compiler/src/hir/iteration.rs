//! Lowers CLR collection loops through the standard enumeration interfaces.

use super::{
    Block, Checked, Checker, Expr, ExprKind, Member, Statement, Target, Type, TypeBoundKind, Value,
    bounds::reachable_bounds,
};
use crate::{Diagnostic, Span, syntax};

impl<'src> Checker<'_, 'src> {
    pub(super) fn enumeration_loop(
        &mut self,
        loop_: &syntax::ForLoop<'src>,
        source: Expr,
    ) -> Checked<Statement> {
        let span = source.span;
        let source_type = source.ty;
        let enumerable = self.enumerable_interface(source_type, span)?;
        if self.comptime {
            return Err(Diagnostic::new(
                "E0501",
                span,
                "CLR enumeration is unavailable in comptime functions",
            ));
        }
        let unavailable = || {
            Diagnostic::new(
                "E0315",
                span,
                "required CLR enumeration methods are unavailable",
            )
        };
        let (Some(enumerator), Some(disposable)) =
            (self.references.enumerator, self.references.disposable)
        else {
            return Err(unavailable());
        };
        let get_enumerator = self.enumeration_member(enumerable, "GetEnumerator", span)?;
        let enumerator_type = self.types.method_signature(get_enumerator, span)?.result;
        let current = self.enumeration_member(enumerator_type, "get_Current", span)?;
        let element = self.types.method_signature(current, span)?.result;
        let [move_next] = self.references.members(enumerator, "MoveNext") else {
            return Err(unavailable());
        };
        let [dispose] = self.references.members(disposable, "Dispose") else {
            return Err(unavailable());
        };

        let collection = self.local(source_type, span)?;
        let cursor = self.local(enumerator_type, span)?;
        let binding_span = loop_.binding.1;
        let binding = self.local(element, binding_span)?;
        let mut body = self.for_body(loop_, binding, element)?;
        body.statements.insert(
            0,
            Statement::Store {
                target: Value::Local(binding),
                value: local_interface_call(cursor, enumerator_type, current, element, span),
                span: binding_span,
            },
        );
        let iteration = Statement::While {
            condition: local_interface_call(
                cursor,
                enumerator_type,
                Member::declared(*move_next, None),
                Type::Bool,
                span,
            ),
            body,
        };
        let (kind, ty) = self.disposal_region(
            local_interface_call(
                collection,
                source_type,
                get_enumerator,
                enumerator_type,
                span,
            ),
            cursor,
            void_block(vec![iteration]),
            Member::declared(*dispose, None),
            span,
        )?;
        Ok(Statement::Discard(Expr {
            kind: ExprKind::Block(void_block(vec![
                Statement::Store {
                    target: Value::Local(collection),
                    value: source,
                    span,
                },
                Statement::Discard(Expr { kind, ty, span }),
            ])),
            ty: Type::Void,
            span,
        }))
    }

    fn enumerable_interface(&self, source: Type, span: Span) -> Checked<Type> {
        let mut found = None;
        if let Some(definition) = self.references.enumerable
            && !source.is_nullable()
        {
            let bounds = reachable_bounds(source, self.bounds).filter_map(|bound| match bound {
                TypeBoundKind::Target(target) => Some(target),
                _ => None,
            });
            for root in std::iter::once(source).chain(bounds) {
                for parent in self.types.ancestors(root, true) {
                    if self.types.imported_id(parent) != Some(definition) {
                        continue;
                    }
                    if found.is_some_and(|previous| previous != parent) {
                        return Err(Diagnostic::new(
                            "E0315",
                            span,
                            "ambiguous CLR element type. Convert the source to a specific IEnumerable<T>",
                        ));
                    }
                    found = Some(parent);
                }
            }
        }
        found.ok_or_else(|| {
            Diagnostic::new(
                "E0315",
                span,
                "for loops require a non-nullable array, string, or CLR IEnumerable<T>, or an integer range",
            )
        })
    }

    fn enumeration_member(&self, owner: Type, name: &str, span: Span) -> Checked<Member> {
        if let Some(id) = self.types.imported_id(owner)
            && let [method] = self.references.members(id, name)
        {
            return Ok(Member::declared(*method, Some(owner)));
        }
        Err(Diagnostic::new(
            "E0315",
            span,
            format!("required CLR enumeration method '{name}' is unavailable"),
        ))
    }
}

pub(super) fn local_interface_call(
    local: u16,
    receiver_type: Type,
    method: Member,
    ty: Type,
    span: Span,
) -> Expr {
    let receiver = Expr {
        kind: ExprKind::Load(Value::Local(local)),
        ty: receiver_type,
        span,
    };
    let kind =
        if receiver_type.imported_value() || matches!(receiver_type, Type::MethodParameter(_)) {
            ExprKind::ConstrainedCall {
                method,
                type_arguments: Box::new([]),
                receiver: Box::new(receiver),
                temporary: None,
                args: Vec::new(),
            }
        } else {
            ExprKind::Call(
                Target::External {
                    method: method.id,
                    owner: method.owner,
                    arguments: Box::new([]),
                },
                vec![receiver],
            )
        };
    Expr { kind, ty, span }
}

fn void_block(statements: Vec<Statement>) -> Block {
    Block {
        statements,
        tail: None,
        ty: Type::Void,
    }
}
