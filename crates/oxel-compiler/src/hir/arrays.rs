//! Type-checks CLR zero-based vectors without introducing a runtime wrapper.

use std::collections::HashSet;

use super::{
    Assignment, Block, Checked, Checker, Expr, ExprKind, InstanceKind, Path, Span, Statement,
    Structs, Type, Value, constants,
};
use crate::{Diagnostic, syntax};

pub(crate) struct ArrayAccess {
    pub array: Expr,
    pub index: Expr,
    pub element: Type,
    pub value: Option<Expr>,
}

impl Structs<'_, '_> {
    pub(super) fn array(&self, element: Type, span: Span) -> Checked<Type> {
        self.constructed_type(InstanceKind::Array, vec![element], span)
    }

    pub(super) fn array_element(&self, ty: Type) -> Option<Type> {
        let Type::Array(id) = ty.non_nullable() else {
            return None;
        };
        Some(self.instance(id).arguments[0])
    }

    pub(super) fn zero_initializable(&self, ty: Type, span: Span) -> Checked<bool> {
        let mut pending = vec![ty];
        let mut seen = HashSet::new();
        while let Some(ty) = pending.pop() {
            if ty.is_nullable()
                || self.nullable_underlying(ty).is_some()
                || ty.numeric()
                || matches!(ty, Type::Bool | Type::Char | Type::ClrEnum(_))
                || !seen.insert(ty)
            {
                continue;
            }
            let Some(definition) = self.definition_of(ty) else {
                return Ok(false);
            };
            let definition = &self.definitions[definition];
            if let Some(union) = &definition.union {
                if union.zero_variant.is_none() {
                    return Ok(false);
                }
                // The zero variant carries no payload even when another variant does.
                continue;
            }
            for field in &definition.fields {
                pending.push(self.field_type(ty, field.ty, span)?);
            }
        }
        Ok(true)
    }
}

impl<'src> Checker<'_, 'src> {
    pub(super) fn new_array(
        &mut self,
        path: &Path<'src>,
        length: &syntax::Expr<'src>,
        span: Span,
    ) -> Checked<(ExprKind, Type)> {
        let element = self.scoped_value_type(path)?;
        self.record_annotation(path, element);
        if !self.types.zero_initializable(element, path.span)? {
            return Err(Diagnostic::new(
                "E0314",
                path.span,
                "array allocation requires elements with a valid zero value; use a nullable reference element type for reference arrays",
            ));
        }
        self.require_comptime_array_element(element, path.span)?;
        let length = self.expression(length, Some(Type::I32))?;
        let ty = if length.ty == Type::Never {
            Type::Never
        } else {
            self.types.array(element, span)?
        };
        Ok((
            ExprKind::NewArray {
                element,
                length: Box::new(length),
            },
            ty,
        ))
    }

    pub(super) fn array_literal(
        &mut self,
        items: &[syntax::Expr<'src>],
        expected: Option<Type>,
        span: Span,
    ) -> Checked<(ExprKind, Type)> {
        let mut element = expected.and_then(|ty| self.types.array_element(ty));
        if items.is_empty() && element.is_none() {
            return Err(Diagnostic::new(
                "E0314",
                span,
                "an empty array literal needs an element type",
            ));
        }
        i32::try_from(items.len())
            .map_err(|_| Diagnostic::new("E0314", span, "CLR vector length exceeds i32"))?;
        let mut typed = Vec::with_capacity(items.len());
        let mut diverges = false;
        for item in items {
            let value = self.expression(item, element)?;
            match value.ty {
                Type::Never => diverges = true,
                Type::Void | Type::Null => {
                    return Err(Diagnostic::new(
                        "E0314",
                        item.span,
                        "array elements must produce a value; null needs a nullable element type",
                    ));
                }
                ty => {
                    element.get_or_insert(ty);
                }
            }
            typed.push(value);
        }
        let element = element.unwrap_or(Type::I32); // All elements diverge.
        self.require_comptime_array_element(element, span)?;
        let ty = if diverges {
            Type::Never
        } else {
            self.types.array(element, span)?
        };
        Ok((
            ExprKind::ArrayLiteral {
                element,
                items: typed,
            },
            ty,
        ))
    }

    pub(super) fn index_array(
        &mut self,
        array: Expr,
        element: Type,
        arguments: &[syntax::Expr<'src>],
        span: Span,
        assignment: Option<Assignment<'_, 'src>>,
    ) -> Checked<Expr> {
        let [argument] = arguments else {
            return Err(Diagnostic::new(
                "E0205",
                span,
                "a CLR vector takes one index",
            ));
        };
        let index = self.expression(argument, Some(Type::I32))?;
        let (array, index, previous, saved) =
            if let Some(operator) = assignment.and_then(|assignment| assignment.operator) {
                let (array_slot, array_load) = self.array_operand(array.ty, array.span)?;
                let (index_slot, index_load) = self.array_operand(Type::I32, index.span)?;
                let read = array_access(array_load(), index_load(), element, None, span);
                let saved = vec![
                    Statement::Store {
                        target: Value::Local(array_slot),
                        span: array.span,
                        value: array,
                    },
                    Statement::Store {
                        target: Value::Local(index_slot),
                        span: index.span,
                        value: index,
                    },
                ];
                (array_load(), index_load(), Some((operator, read)), saved)
            } else {
                (array, index, None, Vec::new())
            };
        let value = assignment
            .map(|assignment| self.assigned_value(assignment.value, previous, element))
            .transpose()?;
        let access = array_access(array, index, element, value, span);
        if saved.is_empty() {
            return Ok(access);
        }
        let ty = if saved.iter().any(|statement| {
            matches!(
                statement, Statement::Store { value, .. } if value.ty == Type::Never
            )
        }) {
            Type::Never
        } else {
            access.ty
        };
        Ok(Expr {
            kind: ExprKind::Block(Block {
                statements: saved,
                tail: Some(Box::new(access)),
                ty,
            }),
            ty,
            span,
        })
    }

    fn array_operand(&mut self, ty: Type, span: Span) -> Checked<(u16, impl Fn() -> Expr + use<>)> {
        let slot = self.local(ty, span)?;
        Ok((slot, move || Expr {
            kind: ExprKind::Load(Value::Local(slot)),
            ty,
            span,
        }))
    }

    fn require_comptime_array_element(&self, element: Type, span: Span) -> Checked<()> {
        if self.comptime
            && !constants::supported_value(
                self.types,
                element,
                constants::ValueContext::Function,
                span,
            )?
        {
            return Err(Diagnostic::new(
                "E0501",
                span,
                "compile-time array requires a supported element type",
            ));
        }
        Ok(())
    }
}

pub(super) fn array_access(
    array: Expr,
    index: Expr,
    element: Type,
    value: Option<Expr>,
    span: Span,
) -> Expr {
    let ty =
        if index.ty == Type::Never || value.as_ref().is_some_and(|value| value.ty == Type::Never) {
            Type::Never
        } else if value.is_some() {
            Type::Void
        } else {
            element
        };
    Expr {
        kind: ExprKind::ArrayAccess(Box::new(ArrayAccess {
            array,
            index,
            element,
            value,
        })),
        ty,
        span,
    }
}
