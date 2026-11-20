//! Mutable CLR vector values live in the current initializer's interpreter.

use super::{
    ConstValue, Diagnostic, Evaluated, Evaluation, Expr, Frame, Interpreter, Span, unsupported,
};
use crate::{Type, hir::ArrayAccess};

impl Interpreter<'_, '_, '_> {
    pub(super) fn allocate_array(&mut self, items: Vec<Evaluated>) -> Evaluated {
        let index = self.arrays.len();
        self.arrays.push(items);
        Evaluated::Array(index)
    }

    pub(super) fn array_literal(
        &mut self,
        items: &[Expr],
        frame: &mut Frame,
        span: Span,
        depth: usize,
    ) -> Evaluation<Evaluated> {
        self.charge(items.len() + 1, span)?;
        let mut values = Vec::with_capacity(items.len());
        for item in items {
            values.push(self.value(item, frame, depth)?);
        }
        Ok(self.allocate_array(values))
    }

    pub(super) fn new_array(
        &mut self,
        element: Type,
        length: &Expr,
        frame: &mut Frame,
        span: Span,
        depth: usize,
    ) -> Evaluation<Evaluated> {
        let ConstValue::Integer(count, Type::I32) = self.scalar(length, frame, depth)? else {
            return Err(unsupported(length.span).into());
        };
        let count = usize::try_from(count).map_err(|_| {
            Diagnostic::new(
                "E0503",
                length.span,
                "array length is negative in constant evaluation",
            )
        })?;
        self.charge(count + 1, span)?;
        let items = if count == 0 {
            Vec::new()
        } else {
            vec![self.zero_value(element, span, depth)?; count]
        };
        Ok(self.allocate_array(items))
    }

    pub(super) fn array_length(
        &mut self,
        array: &Expr,
        frame: &mut Frame,
        depth: usize,
    ) -> Evaluation<Evaluated> {
        let Evaluated::Array(id) = self.value(array, frame, depth)? else {
            return Err(unsupported(array.span).into());
        };
        Ok(Evaluated::Scalar(ConstValue::Integer(
            self.arrays[id].len() as i64,
            Type::I32,
        )))
    }

    pub(super) fn array_access(
        &mut self,
        access: &ArrayAccess,
        frame: &mut Frame,
        span: Span,
        depth: usize,
    ) -> Evaluation<Option<Evaluated>> {
        let Evaluated::Array(id) = self.value(&access.array, frame, depth)? else {
            return Err(unsupported(access.array.span).into());
        };
        let ConstValue::Integer(index, Type::I32) = self.scalar(&access.index, frame, depth)?
        else {
            return Err(unsupported(access.index.span).into());
        };
        // CLR stelem checks bounds after the value operand has been evaluated.
        let value = access
            .value
            .as_ref()
            .map(|value| self.value(value, frame, depth))
            .transpose()?;
        let slot = usize::try_from(index)
            .ok()
            .and_then(|index| self.arrays[id].get_mut(index))
            .ok_or_else(|| {
                Diagnostic::new(
                    "E0503",
                    span,
                    "array index is out of bounds in constant evaluation",
                )
            })?;
        if let Some(value) = value {
            *slot = value;
            Ok(None)
        } else {
            Ok(Some(slot.clone()))
        }
    }

    fn zero_value(&mut self, ty: Type, span: Span, depth: usize) -> Evaluation<Evaluated> {
        self.enter(span, depth)?;
        let value = match ty {
            Type::NullableString => ConstValue::Null,
            Type::Bool => ConstValue::Bool(false),
            Type::F32 => ConstValue::F32(0.0),
            Type::F64 => ConstValue::F64(0.0),
            Type::Char | Type::ClrEnum(_) => ConstValue::Integer(0, ty),
            ty if ty.integer() => ConstValue::Integer(0, ty),
            _ => {
                let id = self
                    .types
                    .definition_of(ty)
                    .ok_or_else(|| unsupported(span))?;
                if self.types.definitions[id]
                    .union
                    .as_ref()
                    .is_some_and(|union| union.zero_variant.is_some())
                {
                    let tag = Evaluated::Scalar(ConstValue::Integer(0, Type::I32));
                    return Self::aggregate(vec![(0, tag)], span);
                }
                let fields = &self.types.definitions[id].fields;
                self.charge(fields.len(), span)?;
                let mut data = Vec::with_capacity(fields.len());
                for (index, field) in fields.iter().enumerate() {
                    let field = self.types.field_type(ty, field.ty, span)?;
                    data.push((index, self.zero_value(field, span, depth + 1)?));
                }
                return Self::aggregate(data, span);
            }
        };
        Ok(Evaluated::Scalar(value))
    }
}
