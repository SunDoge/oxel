//! Source-level nullability. CLR reference representations are unchanged.

use super::{
    Block, Checked, Checker, Expr, ExprKind, GenericBound, InstanceKind, Member, ParameterKind,
    Statement, Structs, Target, Type, Value,
};
use crate::{
    ArgumentMode, Diagnostic, Span,
    clr::{Invocation, TypeBoundKind},
    syntax::{self, Binary, Expression, Unary},
};

impl Type {
    pub(crate) fn non_nullable(self) -> Self {
        match self {
            Self::NullableObject => Self::Object,
            Self::NullableString => Self::String,
            Self::NullableExternal(id) => Self::External(id),
            Self::NullableInterface(id) => Self::Interface(id),
            Self::NullableConstructedExternal(id) => Self::ConstructedExternal(id),
            Self::NullableImportedInstance(id) => Self::ImportedInstance(id),
            Self::NullableArray(id) => Self::Array(id),
            Self::NullableImportedArray(id) => Self::ImportedArray(id),
            Self::NullableTypeParameter(id) => Self::TypeParameter(id),
            Self::NullableImportedMethodParameter(id) => Self::ImportedMethodParameter(id),
            ty => ty,
        }
    }

    pub(crate) fn is_nullable(self) -> bool {
        self.non_nullable() != self
    }

    pub(crate) fn nullable(self) -> Option<Self> {
        match self.non_nullable() {
            Self::Object => Some(Self::NullableObject),
            Self::String => Some(Self::NullableString),
            Self::External(id) => Some(Self::NullableExternal(id)),
            Self::Interface(id) => Some(Self::NullableInterface(id)),
            Self::ConstructedExternal(id) => Some(Self::NullableConstructedExternal(id)),
            Self::ImportedInstance(id) => Some(Self::NullableImportedInstance(id)),
            Self::Array(id) => Some(Self::NullableArray(id)),
            Self::ImportedArray(id) => Some(Self::NullableImportedArray(id)),
            Self::TypeParameter(id) => Some(Self::NullableTypeParameter(id)),
            Self::ImportedMethodParameter(id) => Some(Self::NullableImportedMethodParameter(id)),
            _ => None,
        }
    }
}

const OPEN_CLR_NULLABILITY: &str = "E0317";

pub(super) fn supported_clr_signature<T>(
    signature: Checked<T>,
    unavailable: &mut Option<Diagnostic>,
) -> Checked<Option<T>> {
    match signature {
        Ok(signature) => Ok(Some(signature)),
        Err(error) if error.code == OPEN_CLR_NULLABILITY => {
            unavailable.get_or_insert(error);
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

pub(super) fn specialize_annotation(pattern: Type, argument: Type, span: Span) -> Checked<Type> {
    if !pattern.is_nullable() {
        return Ok(argument);
    }
    if matches!(
        argument,
        Type::TypeParameter(_) | Type::MethodParameter(_) | Type::ImportedMethodParameter(_)
    ) {
        return Err(Diagnostic::new(
            OPEN_CLR_NULLABILITY,
            span,
            "a CLR nullable parameter annotation requires a concrete type argument",
        ));
    }
    // The annotation applies to reference arguments. A value argument already
    // has the representation specified by the generic instantiation.
    Ok(argument.nullable().unwrap_or(argument))
}

impl Structs<'_, '_> {
    pub(super) fn nullable_payload(&self, ty: Type) -> Option<Type> {
        self.nullable_underlying(ty)
            .or_else(|| ty.is_nullable().then(|| ty.non_nullable()))
    }

    pub(super) fn nullable_type(
        &self,
        ty: Type,
        bounds: &[GenericBound],
        span: Span,
    ) -> Checked<Option<Type>> {
        if let Some(definition) = self.references.nullable_value
            && self.satisfies_type_bound(ty, bounds, TypeBoundKind::Value)
        {
            return self
                .constructed_type(InstanceKind::External(definition), vec![ty], span)
                .map(Some);
        }
        Ok(match ty {
            Type::TypeParameter(_) => None,
            _ => ty.nullable(),
        })
    }

    pub(super) fn nullable_underlying(&self, ty: Type) -> Option<Type> {
        let Type::ConstructedClrStruct(instance) = ty else {
            return None;
        };
        let definition = self.references.nullable_value?;
        let instance = self.instance(instance);
        (instance.kind == InstanceKind::External(definition)).then(|| instance.arguments[0])
    }

    pub(super) fn nullable_member(
        &self,
        owner: Type,
        name: &str,
        arguments: usize,
    ) -> Option<Member> {
        let definition = self.references.nullable_value?;
        self.references
            .members(definition, name)
            .iter()
            .copied()
            .find(|&id| self.references.method(id).signature.parameters.len() == arguments)
            .map(|id| Member::declared(id, Some(owner)))
    }
}

impl<'src> Checker<'_, 'src> {
    pub(super) fn require_nonnull_receiver(&self, ty: Type, span: Span) -> Checked<()> {
        if ty.is_nullable() || ty == Type::Null {
            return Err(Diagnostic::new(
                "E0311",
                span,
                "receiver may be null; check it or provide a fallback with '??'",
            ));
        }
        Ok(())
    }

    pub(super) fn null_refinements(&self, condition: &Expr, when_true: bool) -> Vec<Value> {
        let mut known = Vec::new();
        self.collect_null_refinements(condition, when_true, &mut known);
        known
    }

    pub(super) fn refine_after_statement(&mut self, statement: &Statement) {
        let mut known = Vec::new();
        self.collect_statement_refinements(statement, &mut known);
        for value in known {
            if !self.nonnull.contains(&value) {
                self.nonnull.push(value);
            }
        }
    }

    fn collect_statement_refinements(&self, statement: &Statement, known: &mut Vec<Value>) {
        match statement {
            Statement::Discard(value)
            | Statement::Store { value, .. }
            | Statement::DeclareLocal {
                value: Some(value), ..
            } => {
                self.collect_completion_refinements(value, known);
            }
            _ => {}
        }
    }

    fn collect_block_refinements(&self, block: &Block, known: &mut Vec<Value>) {
        for statement in &block.statements {
            self.collect_statement_refinements(statement, known);
        }
        if let Some(tail) = &block.tail {
            self.collect_completion_refinements(tail, known);
        }
    }

    fn collect_completion_refinements(&self, expr: &Expr, known: &mut Vec<Value>) {
        if expr.ty == Type::Never {
            return;
        }
        match &expr.kind {
            ExprKind::Block(block) => self.collect_block_refinements(block, known),
            ExprKind::Call(..) | ExprKind::ConstrainedCall { .. } => {
                self.collect_call_refinements(expr, None, known);
            }
            ExprKind::If {
                condition, yes, no, ..
            } => {
                self.collect_completion_refinements(condition, known);
                let no_exits = no.as_ref().is_some_and(|no| no.ty == Type::Never);
                if yes.ty == Type::Never {
                    self.collect_null_refinements(condition, false, known);
                    if let Some(no) = no {
                        self.collect_completion_refinements(no, known);
                    }
                } else if no_exits {
                    self.collect_null_refinements(condition, true, known);
                    self.collect_block_refinements(yes, known);
                } else {
                    let mut on_true = self.null_refinements(condition, true);
                    self.collect_block_refinements(yes, &mut on_true);
                    if !on_true.is_empty() {
                        let mut on_false = self.null_refinements(condition, false);
                        if let Some(no) = no {
                            self.collect_completion_refinements(no, &mut on_false);
                        }
                        known.extend(on_true.into_iter().filter(|value| on_false.contains(value)));
                    }
                }
            }
            _ => {}
        }
    }

    fn collect_null_refinements(&self, condition: &Expr, when_true: bool, known: &mut Vec<Value>) {
        if condition.ty != Type::Bool {
            return;
        }
        match &condition.kind {
            ExprKind::Unary(Unary::Not, operand) => {
                self.collect_null_refinements(operand, !when_true, known);
            }
            ExprKind::Binary(op @ (Binary::Eq | Binary::Ne), left, right) => {
                let equal = when_true == (*op == Binary::Eq);
                match (&left.kind, &right.kind) {
                    (ExprKind::Null, _) if !equal => self.refine_argument(right, known),
                    (_, ExprKind::Null) if !equal => self.refine_argument(left, known),
                    (ExprKind::Boolean(value), _) => {
                        self.collect_null_refinements(right, equal == *value, known);
                    }
                    (_, ExprKind::Boolean(value)) => {
                        self.collect_null_refinements(left, equal == *value, known);
                    }
                    _ => {}
                }
            }
            ExprKind::If {
                condition,
                yes,
                no: Some(no),
                ..
            } if yes.statements.is_empty() => {
                if let Some(yes) = &yes.tail {
                    // The lowering of && and || leaves a constant on the path
                    // that doesn't evaluate the right operand.
                    if when_true && matches!(no.kind, ExprKind::Boolean(false)) {
                        self.collect_null_refinements(condition, true, known);
                        self.collect_null_refinements(yes, true, known);
                    } else if !when_true && matches!(yes.kind, ExprKind::Boolean(true)) {
                        self.collect_null_refinements(condition, false, known);
                        self.collect_null_refinements(no, false, known);
                    } else {
                        // The left operand completed on every path, even when the
                        // boolean result cannot tell us whether the right operand ran.
                        self.collect_completion_refinements(condition, known);
                    }
                }
            }
            ExprKind::Call(..) | ExprKind::ConstrainedCall { .. } => {
                self.collect_call_refinements(condition, Some(when_true), known);
            }
            _ => {}
        }
    }

    fn collect_call_refinements(
        &self,
        expr: &Expr,
        when_true: Option<bool>,
        known: &mut Vec<Value>,
    ) {
        let (member, type_arguments, args) = match &expr.kind {
            ExprKind::Call(target, args) => {
                if when_true.is_none() {
                    for arg in args {
                        self.collect_completion_refinements(arg, known);
                    }
                }
                let Target::External {
                    method,
                    owner,
                    arguments,
                } = target
                else {
                    return;
                };
                let first =
                    usize::from(self.references.method(*method).invocation == Invocation::Instance);
                (
                    Member::declared(*method, *owner),
                    arguments.as_ref(),
                    &args[first..],
                )
            }
            ExprKind::ConstrainedCall {
                method,
                type_arguments,
                receiver,
                args,
                ..
            } => {
                if when_true.is_none() {
                    self.collect_completion_refinements(receiver, known);
                    for arg in args {
                        self.collect_completion_refinements(arg, known);
                    }
                }
                (*method, type_arguments.as_ref(), args.as_slice())
            }
            _ => return,
        };
        self.clr_argument_refinements(member, type_arguments, args, when_true, known);
    }

    fn clr_argument_refinements(
        &self,
        member: Member,
        type_arguments: &[Type],
        args: &[Expr],
        when_true: Option<bool>,
        known: &mut Vec<Value>,
    ) {
        let paths = when_true.map_or(0b11, |result| 1 << u8::from(result));
        for &(index, contract) in &self.references.method(member.id).argument_postconditions {
            let index = usize::from(index);
            let Some(arg) = args.get(index) else { continue };
            match contract.does_not_return_if {
                0b01 => self.collect_null_refinements(arg, true, known),
                0b10 => self.collect_null_refinements(arg, false, known),
                _ => {}
            }
            if contract.maybe_null & paths != 0 {
                continue;
            }
            let declared_nonnull_output = || {
                when_true.is_some()
                    && contract.maybe_null != 0
                    && matches!(arg.kind, ExprKind::LocalAddress(_, ArgumentMode::Out))
                    && self
                        .types
                        .method_signature(member, arg.span)
                        .is_ok_and(|signature| {
                            self.types
                                .specialize(
                                    signature.parameters[index],
                                    type_arguments,
                                    ParameterKind::ImportedMethod,
                                    arg.span,
                                )
                                .is_ok_and(|ty| {
                                    ty.reference() && ty != Type::Null && !ty.is_nullable()
                                })
                        })
            };
            if contract.not_null & paths == paths || declared_nonnull_output() {
                self.refine_argument(arg, known);
            }
        }
    }

    fn refine_argument(&self, arg: &Expr, known: &mut Vec<Value>) {
        let value = match arg.kind {
            ExprKind::Load(value) | ExprKind::LocalAddress(value, ArgumentMode::Out) => value,
            _ => return,
        };
        // A mutable binding might be reassigned after the test. An immutable
        // out local is assigned once, as checked by definite assignment.
        if matches!(value, Value::Captured(..))
            || self.scopes.iter().any(|scope| {
                scope.any_binding(|binding| {
                    binding.value == value && !binding.mutable && binding.ty.is_nullable()
                })
            })
        {
            known.push(value);
        }
    }

    pub(super) fn with_nonnull<T>(
        &mut self,
        values: impl IntoIterator<Item = Value>,
        evaluate: impl FnOnce(&mut Self) -> Checked<T>,
    ) -> Checked<T> {
        let before = self.nonnull.len();
        self.nonnull.extend(values);
        let result = evaluate(self);
        self.nonnull.truncate(before);
        result
    }

    pub(super) fn null_test(&mut self, op: Binary, value: Expr) -> Checked<(ExprKind, Type)> {
        if value.ty == Type::Never {
            return Ok((value.kind, Type::Never));
        }
        if self.types.nullable_underlying(value.ty).is_some() {
            let span = value.span;
            let member = self.require_nullable_member(value.ty, "get_HasValue", span)?;
            let (kind, ty) =
                self.constrained_call(member, Box::new([]), value, Vec::new(), Type::Bool, span)?;
            return Ok(if op == Binary::Eq {
                (
                    ExprKind::Unary(Unary::Not, Box::new(Expr { kind, ty, span })),
                    ty,
                )
            } else {
                (kind, ty)
            });
        }
        if !value.ty.reference() {
            return Err(Diagnostic::new(
                "E0311",
                value.span,
                "null can only be compared with a reference or nullable value",
            ));
        }
        let null = Expr {
            kind: ExprKind::Null,
            ty: Type::Null,
            span: value.span,
        };
        // Reference equality to null uses ceq even for strings.
        Ok((
            ExprKind::Binary(op, Box::new(null), Box::new(value)),
            Type::Bool,
        ))
    }

    pub(super) fn coalesce(
        &mut self,
        left: &syntax::Expr<'src>,
        right: &syntax::Expr<'src>,
        expected: Option<Type>,
    ) -> Checked<(ExprKind, Type)> {
        let left = self.expression(left, None)?;
        if left.ty == Type::Never {
            self.expression(right, expected)?;
            return Ok((left.kind, Type::Never));
        }
        if left.ty == Type::Null {
            if expected.is_some_and(|ty| self.types.nullable_underlying(ty).is_some()) {
                let right = self.expression(right, expected)?;
                return Ok((right.kind, right.ty));
            }
            let right = self.expression(right, expected.filter(|ty| ty.reference()))?;
            if right.ty != Type::Never && !right.ty.reference() {
                return Err(Diagnostic::new(
                    "E0311",
                    right.span,
                    "'??' requires a reference fallback",
                ));
            }
            let ty = right.ty;
            return Ok((
                ExprKind::Block(Block {
                    statements: vec![Statement::Discard(left)],
                    tail: Some(Box::new(right)),
                    ty,
                }),
                ty,
            ));
        }
        if let Some(underlying) = self.types.nullable_underlying(left.ty) {
            return self.coalesce_value(left, underlying, right);
        }
        if let Some((layout, payload)) = self.standard_option_payload(left.ty) {
            let fallback = self.expression(right, Some(payload))?;
            let value = self.option_or(left, layout, payload, payload, fallback)?;
            return Ok((value.kind, value.ty));
        }
        if !left.ty.is_nullable() {
            return Err(Diagnostic::new(
                "E0311",
                left.span,
                "the left operand of '??' must be nullable",
            ));
        }
        let array_hint = (matches!(right.kind, Expression::Array(_))
            && self.types.array_element(left.ty).is_some())
        .then_some(left.ty.non_nullable());
        let right = self.expression(right, array_hint)?;
        let ty = if right.ty.is_nullable() || right.ty == Type::Null {
            left.ty
        } else {
            left.ty.non_nullable()
        };
        let temporary = self.local(ty, left.span)?;
        self.require(right.ty, ty, right.span)?;
        let right = self.coerce(right, ty);
        Ok((
            ExprKind::Coalesce(Box::new(left), Box::new(right), temporary),
            ty,
        ))
    }

    pub(super) fn require_nullable_member(
        &self,
        ty: Type,
        name: &str,
        span: Span,
    ) -> Checked<Member> {
        self.types.nullable_member(ty, name, 0).ok_or_else(|| {
            Diagnostic::new(
                "E0211",
                span,
                format!("CLR Nullable<T> metadata is missing '{name}'"),
            )
        })
    }

    fn coalesce_value(
        &mut self,
        left: Expr,
        underlying: Type,
        fallback: &syntax::Expr<'src>,
    ) -> Checked<(ExprKind, Type)> {
        let right = self.expression(
            fallback,
            fallback.is_numeric_literal().then_some(underlying),
        )?;
        let owner = left.ty;
        let span = left.span;
        let ty = if right.ty == owner || right.ty == Type::Null {
            owner
        } else {
            underlying
        };
        self.require(right.ty, ty, right.span)?;
        let slot = self.local(owner, span)?;
        let load = || Expr {
            kind: ExprKind::Load(Value::Local(slot)),
            ty: owner,
            span,
        };
        let call = |method, ty| Expr {
            kind: ExprKind::ConstrainedCall {
                method,
                type_arguments: Box::new([]),
                receiver: Box::new(load()),
                temporary: None,
                args: Vec::new(),
            },
            ty,
            span,
        };
        let condition = call(
            self.require_nullable_member(owner, "get_HasValue", span)?,
            Type::Bool,
        );
        let present = if ty == owner {
            load()
        } else {
            call(
                self.require_nullable_member(owner, "GetValueOrDefault", span)?,
                underlying,
            )
        };
        let choose = Expr {
            kind: ExprKind::If {
                condition: Box::new(condition),
                yes: Block {
                    statements: Vec::new(),
                    tail: Some(Box::new(present)),
                    ty,
                },
                no: Some(Box::new(self.coerce(right, ty))),
                result_local: None,
            },
            ty,
            span,
        };
        Ok((
            ExprKind::Block(Block {
                statements: vec![Statement::Store {
                    target: Value::Local(slot),
                    value: left,
                    span,
                }],
                tail: Some(Box::new(choose)),
                ty,
            }),
            ty,
        ))
    }

    pub(super) fn join_branches(&self, yes: &mut Block, no: Expr) -> Checked<(Type, Expr)> {
        let a = yes.ty;
        let ty = self.join_types(a, no.ty, no.span)?;
        if a != Type::Never && a != ty {
            let tail = yes
                .tail
                .take()
                .expect("a converted value branch has a result");
            yes.tail = Some(Box::new(self.coerce(*tail, ty)));
            yes.ty = ty;
        }
        Ok((ty, self.coerce(no, ty)))
    }

    pub(super) fn else_hint(
        &self,
        yes: Type,
        no: &syntax::Expr<'src>,
        expected: Option<Type>,
    ) -> Option<Type> {
        expected.or_else(|| {
            (!yes.reference()
                && yes != Type::Never
                && !(self.references.nullable_value.is_some() && no.has_null_tail()))
            .then_some(yes)
        })
    }

    pub(super) fn join_types(&self, a: Type, b: Type, span: Span) -> Checked<Type> {
        let fits = |ty, target| ty == target || self.can_upcast(ty, target);
        let nullable = |value: Type, other| {
            if other == Type::Null {
                self.types.nullable_type(value, self.bounds, span)
            } else {
                Ok(value.nullable())
            }
        };
        Ok(if a == Type::Never || fits(a, b) {
            b
        } else if b == Type::Never || fits(b, a) {
            a
        } else if let Some(nullable) = nullable(a, b)?
            && fits(a, nullable)
            && fits(b, nullable)
        {
            nullable
        } else if let Some(nullable) = nullable(b, a)?
            && fits(a, nullable)
            && fits(b, nullable)
        {
            nullable
        } else {
            self.require(b, a, span)?;
            a
        })
    }
}
