use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};

mod arrays;

use crate::{Diagnostic, Span, syntax::LoopControl};

use super::{
    super::{
        Block, Decision, Destination, Expr, ExprKind, Function, Match, ParameterKind,
        PatternLiteral, RangeLoop, Statement, Structs, Target, Value,
    },
    ConstValue, Type, TypeQuery, ValueContext, binary, check_string_size, numeric_cast,
    supported_value, unary,
};

const STEP_LIMIT: usize = 100_000;
const DEPTH_LIMIT: usize = 192;
const CALL_CACHE_LIMIT: usize = 4096;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Argument {
    Integer(i64),
    Float32(u32),
    Float64(u64),
    Bool(bool),
    Null,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Call {
    function: usize,
    types: Box<[Type]>,
    arguments: Vec<Argument>,
}

impl Call {
    fn key(function: usize, types: &[Type], parameters: &[Evaluated]) -> Option<Self> {
        if parameters.len() > 16 {
            return None;
        }
        let arguments = parameters
            .iter()
            .map(|value| {
                let Evaluated::Scalar(value) = value else {
                    return None;
                };
                Some(match value {
                    ConstValue::Integer(value, _) => Argument::Integer(*value),
                    // Distinguish signed zero and NaN bit patterns from ordinary equality.
                    ConstValue::F32(value) => Argument::Float32(value.to_bits()),
                    ConstValue::F64(value) => Argument::Float64(value.to_bits()),
                    ConstValue::Bool(value) => Argument::Bool(*value),
                    ConstValue::Null => Argument::Null,
                    ConstValue::String(_) => return None,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Self {
            function,
            types: types.into(),
            arguments,
        })
    }
}

enum CallState {
    Active,
    Done(Option<Evaluated>),
}

enum Stop {
    Return(Option<Evaluated>),
    TryFailure(Option<Evaluated>),
    Break,
    Continue,
    Diagnostic(Box<Diagnostic>),
}

impl From<Diagnostic> for Stop {
    fn from(value: Diagnostic) -> Self {
        Self::Diagnostic(Box::new(value))
    }
}

type Evaluation<T> = Result<T, Stop>;

#[derive(Clone)]
pub(super) enum Evaluated {
    Scalar(ConstValue),
    Aggregate(Arc<Aggregate>),
    Array(usize),
}

pub(super) struct Aggregate {
    pub(super) fields: Vec<(usize, Evaluated)>,
    pub(super) fingerprint: OnceLock<blake3::Hash>,
    depth: usize,
}

impl Aggregate {
    fn field(&self, index: usize) -> Option<&Evaluated> {
        let slot = self
            .fields
            .binary_search_by_key(&index, |(id, _)| *id)
            .ok()?;
        Some(&self.fields[slot].1)
    }
}

struct Frame {
    types: Box<[Type]>,
    parameters: Vec<Evaluated>,
    locals: Vec<Option<Evaluated>>,
}

pub(super) fn evaluate(
    initializer: &Expr,
    locals: usize,
    functions: &[Option<Function>],
    types: &Structs<'_, '_>,
) -> Result<Evaluated, Diagnostic> {
    let mut interpreter = Interpreter {
        functions,
        types,
        remaining: STEP_LIMIT,
        calls: HashMap::new(),
        arrays: Vec::new(),
    };
    let mut frame = Frame {
        types: Box::default(),
        parameters: Vec::new(),
        locals: vec![None; locals],
    };
    match interpreter
        .charge(locals, initializer.span)
        .and_then(|()| interpreter.value(initializer, &mut frame, 0))
    {
        Ok(value) => Ok(value),
        Err(Stop::Diagnostic(error)) => Err(*error),
        Err(_) => Err(unsupported(initializer.span)),
    }
}

struct Interpreter<'a, 'src, 'refs> {
    functions: &'a [Option<Function>],
    types: &'a Structs<'src, 'refs>,
    remaining: usize,
    calls: HashMap<Call, CallState>,
    arrays: Vec<Vec<Evaluated>>,
}

impl Interpreter<'_, '_, '_> {
    fn charge(&mut self, count: usize, span: Span) -> Evaluation<()> {
        self.remaining = self.remaining.checked_sub(count).ok_or_else(|| {
            Diagnostic::new("E0504", span, "constant evaluation exceeded its step limit")
        })?;
        Ok(())
    }

    fn enter(&mut self, span: Span, depth: usize) -> Evaluation<()> {
        if depth > DEPTH_LIMIT {
            return Err(Diagnostic::new(
                "E0504",
                span,
                "constant evaluation exceeded its depth limit",
            )
            .into());
        }
        self.charge(1, span)
    }

    fn call(
        &mut self,
        index: usize,
        types: Box<[Type]>,
        parameters: Vec<Evaluated>,
        span: Span,
        depth: usize,
    ) -> Evaluation<Option<Evaluated>> {
        let key = Call::key(index, &types, &parameters);
        if let Some(state) = key.as_ref().and_then(|key| self.calls.get(key)) {
            return match state {
                CallState::Active => Err(Diagnostic::new(
                    "E0502",
                    span,
                    "comptime call repeats the arguments of an active call",
                )
                .into()),
                CallState::Done(value) => Ok(value.clone()),
            };
        }
        let cached = key.filter(|_| self.calls.len() < CALL_CACHE_LIMIT);
        if let Some(key) = &cached {
            self.calls.insert(key.clone(), CallState::Active);
        }
        let function = self.functions[index]
            .as_ref()
            .ok_or_else(|| unsupported(span))?;
        self.charge(function.locals.len(), span)?;
        if !types.is_empty() {
            for &ty in function
                .signature
                .parameters
                .iter()
                .chain(&function.locals)
                .chain(std::iter::once(&function.signature.result))
            {
                let ty = self
                    .types
                    .specialize(ty, &types, ParameterKind::Method, span)?;
                if ty != Type::Void
                    && !supported_value(self.types, ty, ValueContext::Instantiation, span)?
                {
                    return Err(Diagnostic::new(
                        "E0501",
                        span,
                        format!(
                            "comptime instantiation uses unsupported value type '{}'",
                            self.types.describe(ty)
                        ),
                    )
                    .into());
                }
            }
        }
        let mut frame = Frame {
            types,
            parameters,
            locals: vec![None; function.locals.len()],
        };
        let super::super::MethodBody::Managed(body) = &function.body else {
            return Err(unsupported(span).into());
        };
        let value = match self.block(body, &mut frame, span, depth) {
            Err(Stop::Return(value)) => value,
            result => result?,
        };
        if let Some(key) = cached {
            if matches!(
                value,
                Some(
                    Evaluated::Scalar(ConstValue::String(_))
                        | Evaluated::Aggregate(_)
                        | Evaluated::Array(_)
                )
            ) {
                self.calls.remove(&key);
            } else {
                self.calls.insert(key, CallState::Done(value.clone()));
            }
        }
        Ok(value)
    }

    fn block(
        &mut self,
        block: &Block,
        frame: &mut Frame,
        span: Span,
        depth: usize,
    ) -> Evaluation<Option<Evaluated>> {
        self.enter(span, depth)?;
        for statement in &block.statements {
            self.charge(1, span)?;
            match statement {
                Statement::DeclareLocal { index, value } => {
                    frame.locals[usize::from(*index)] = match value {
                        Some(value) => Some(self.value(value, frame, depth + 1)?),
                        None => None,
                    };
                }
                Statement::Store {
                    target: Value::Local(index),
                    value,
                    ..
                } => {
                    let value = self.value(value, frame, depth + 1)?;
                    frame.locals[usize::from(*index)] = Some(value);
                }
                Statement::Store { .. } => return Err(unsupported(span).into()),
                Statement::Return(value) => {
                    let value = value
                        .as_ref()
                        .map(|value| self.expression(value, frame, depth + 1))
                        .transpose()?
                        .flatten();
                    return Err(Stop::Return(value));
                }
                Statement::TryFailure(error) => {
                    let error = error
                        .as_ref()
                        .map(|expr| self.value(expr, frame, depth + 1))
                        .transpose()?;
                    return Err(Stop::TryFailure(error));
                }
                Statement::Discard(expr) => {
                    self.expression(expr, frame, depth + 1)?;
                }
                Statement::While { condition, body } => {
                    while self.condition(condition, frame, depth + 1)? {
                        match self.block(body, frame, condition.span, depth + 1) {
                            Ok(_) | Err(Stop::Continue) => {}
                            Err(Stop::Break) => break,
                            Err(other) => return Err(other),
                        }
                    }
                }
                Statement::For(range) => self.range_loop(range, frame, depth + 1)?,
                Statement::LoopControl(control) => {
                    return Err(match control {
                        LoopControl::Break => Stop::Break,
                        LoopControl::Continue => Stop::Continue,
                    });
                }
            }
        }
        if let Some(tail) = &block.tail {
            return self.expression(tail, frame, depth + 1);
        }
        Ok(None)
    }

    fn range_loop(&mut self, range: &RangeLoop, frame: &mut Frame, depth: usize) -> Evaluation<()> {
        let ConstValue::Integer(start, ty) = self.scalar(&range.start, frame, depth)? else {
            return Err(unsupported(range.start.span).into());
        };
        let ConstValue::Integer(limit, _) = self.scalar(&range.end, frame, depth)? else {
            return Err(unsupported(range.end.span).into());
        };
        let mut index = ty.integer_value(start);
        let limit = ty.integer_value(limit);
        while index < limit || (range.inclusive && index == limit) {
            frame.locals[usize::from(range.index)] =
                Some(Evaluated::Scalar(ConstValue::Integer(index as i64, ty)));
            match self.block(&range.body, frame, range.start.span, depth) {
                Ok(_) | Err(Stop::Continue) => {}
                Err(Stop::Break) => break,
                Err(other) => return Err(other),
            }
            if index == limit {
                break;
            }
            index += 1;
        }
        Ok(())
    }

    fn condition(&mut self, expr: &Expr, frame: &mut Frame, depth: usize) -> Evaluation<bool> {
        match self.scalar(expr, frame, depth)? {
            ConstValue::Bool(value) => Ok(value),
            _ => Err(unsupported(expr.span).into()),
        }
    }

    fn match_expression(
        &mut self,
        matched: &Match,
        frame: &mut Frame,
        span: Span,
        depth: usize,
    ) -> Evaluation<Option<Evaluated>> {
        let value = self.value(&matched.value, frame, depth)?;
        frame.locals[usize::from(matched.local)] = Some(value);
        let mut next = Destination::Decision(0);
        loop {
            self.charge(1, span)?;
            next = match next {
                Destination::Decision(id) => {
                    let decision = &matched.decisions[id];
                    let value = self.value(&decision.value, frame, depth)?;
                    self.match_destination(decision, value)?
                }
                Destination::Arm(id) => {
                    if let Ok(guard) = matched.guards.binary_search_by_key(&id, |guard| guard.arm) {
                        let guard = &matched.guards[guard];
                        match self.block(&guard.condition, frame, span, depth)? {
                            Some(Evaluated::Scalar(ConstValue::Bool(false))) => {
                                next = Destination::Decision(guard.fallback);
                                continue;
                            }
                            Some(Evaluated::Scalar(ConstValue::Bool(true))) => {}
                            _ => return Err(unsupported(span).into()),
                        }
                    }
                    return self.block(&matched.arms[id], frame, span, depth);
                }
                Destination::Setup(id) => {
                    let setup = &matched.setups[id];
                    self.block(&setup.initialization, frame, span, depth)?;
                    Destination::Arm(setup.arm)
                }
                Destination::Invalid => return Err(unsupported(span).into()),
            };
        }
    }

    fn match_destination(
        &mut self,
        decision: &Decision,
        value: Evaluated,
    ) -> Evaluation<Destination> {
        let span = decision.value.span;
        if let Some(zero_tag) = decision.zero_tag_target() {
            let Evaluated::Aggregate(value) = value else {
                return Err(unsupported(span).into());
            };
            let Some(Evaluated::Scalar(ConstValue::Integer(tag, _))) = value.field(0) else {
                return Err(unsupported(span).into());
            };
            if *tag == 0 {
                return Ok(zero_tag);
            }
            return Ok(usize::try_from(*tag)
                .ok()
                .and_then(|tag| tag.checked_sub(1))
                .and_then(|case| decision.targets.get(case).copied())
                .unwrap_or(Destination::Invalid));
        }
        if let Some(literals) = &decision.literals
            && literals.keys.is_empty()
        {
            return Ok(literals.other.unwrap_or(Destination::Invalid));
        }
        let Evaluated::Scalar(value) = value else {
            return Err(unsupported(span).into());
        };
        if let Some(literals) = &decision.literals {
            for (pattern, &target) in literals.keys.iter().zip(&decision.targets) {
                self.charge(1, span)?;
                let equal = match (pattern, &value) {
                    (PatternLiteral::Integer(key), ConstValue::Integer(value, _)) => key == value,
                    (PatternLiteral::Character(key), ConstValue::Integer(value, _)) => {
                        i64::from(*key) == *value
                    }
                    (PatternLiteral::String(key), ConstValue::String(value)) => {
                        self.charge(key.len().min(value.len()) / 1024, span)?;
                        key == value
                    }
                    _ => false,
                };
                if equal {
                    return Ok(target);
                }
            }
            Ok(literals.other.unwrap_or(Destination::Invalid))
        } else if let ConstValue::Bool(value) = value {
            Ok(decision.targets[usize::from(value)])
        } else {
            Err(unsupported(span).into())
        }
    }

    fn value(&mut self, expr: &Expr, frame: &mut Frame, depth: usize) -> Evaluation<Evaluated> {
        self.expression(expr, frame, depth)?.ok_or_else(|| {
            Diagnostic::new("E0501", expr.span, "constant evaluation requires a value").into()
        })
    }

    fn scalar(&mut self, expr: &Expr, frame: &mut Frame, depth: usize) -> Evaluation<ConstValue> {
        let Evaluated::Scalar(value) = self.value(expr, frame, depth)? else {
            return Err(Diagnostic::new(
                "E0501",
                expr.span,
                "constant evaluation requires a primitive value",
            )
            .into());
        };
        Ok(value)
    }

    fn construct(
        &mut self,
        fields: &[(usize, Expr)],
        frame: &mut Frame,
        span: Span,
        depth: usize,
    ) -> Evaluation<Evaluated> {
        self.charge(fields.len(), span)?;
        let mut data = Vec::with_capacity(fields.len());
        for (index, value) in fields {
            data.push((*index, self.value(value, frame, depth)?));
        }
        Self::aggregate(data, span)
    }

    fn aggregate(mut data: Vec<(usize, Evaluated)>, span: Span) -> Evaluation<Evaluated> {
        let field_depth = data
            .iter()
            .filter_map(|(_, value)| match value {
                Evaluated::Aggregate(aggregate) => Some(aggregate.depth),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        // Bound nested stored values in addition to the evaluator's call stack.
        if field_depth >= DEPTH_LIMIT {
            return Err(Diagnostic::new(
                "E0504",
                span,
                "compile-time value exceeded its nesting limit",
            )
            .into());
        }
        data.sort_unstable_by_key(|(index, _)| *index);
        Ok(Evaluated::Aggregate(Arc::new(Aggregate {
            fields: data,
            fingerprint: OnceLock::new(),
            depth: field_depth + 1,
        })))
    }

    fn query(&mut self, query: TypeQuery, ty: Type, span: Span) -> Evaluation<Evaluated> {
        let types = self.types;
        if matches!(query, TypeQuery::TypeName) {
            return self.query_string(&types.describe(ty), span);
        }

        self.charge(1, span)?;
        let mut items = Vec::new();
        query.visit_members::<Stop>(types, ty, span, |name, details| {
            self.charge(1, span)?;
            let name = self.query_string(name, span)?;
            let value = if let Some((ty, readonly)) = details {
                self.charge(3, span)?;
                let type_name = self.query_string(&types.describe(ty), span)?;
                Self::aggregate(
                    vec![
                        (0, name),
                        (1, type_name),
                        (2, Evaluated::Scalar(ConstValue::Bool(readonly))),
                    ],
                    span,
                )?
            } else {
                name
            };
            items.push(value);
            Ok(())
        })?;
        Ok(self.allocate_array(items))
    }

    fn query_string(&mut self, value: &str, span: Span) -> Evaluation<Evaluated> {
        check_string_size(value.encode_utf16().count(), span)?;
        self.charge(value.len() / 1024, span)?;
        Ok(Evaluated::Scalar(ConstValue::String(value.into())))
    }

    fn expression(
        &mut self,
        expr: &Expr,
        frame: &mut Frame,
        depth: usize,
    ) -> Evaluation<Option<Evaluated>> {
        self.enter(expr.span, depth)?;
        let child = depth + 1;
        match &expr.kind {
            ExprKind::Load(Value::Parameter(index)) => {
                Ok(Some(frame.parameters[usize::from(*index)].clone()))
            }
            ExprKind::Load(Value::Local(index)) => frame.locals[usize::from(*index)]
                .clone()
                .map(Some)
                .ok_or_else(|| {
                    Diagnostic::new("E0321", expr.span, "comptime read of an unassigned local")
                        .into()
                }),
            ExprKind::Construct { fields, .. } => {
                self.construct(fields, frame, expr.span, child).map(Some)
            }
            ExprKind::Field(value, field) => {
                let Evaluated::Aggregate(value) = self.value(value, frame, child)? else {
                    return Err(unsupported(expr.span).into());
                };
                value
                    .field(*field)
                    .cloned()
                    .map(Some)
                    .ok_or_else(|| unsupported(expr.span).into())
            }
            ExprKind::Block(block) => self.block(block, frame, expr.span, child),
            ExprKind::Coalesce(left, right, _) => {
                let value = self.value(left, frame, child)?;
                if matches!(value, Evaluated::Scalar(ConstValue::Null)) {
                    self.expression(right, frame, child)
                } else {
                    Ok(Some(value))
                }
            }
            ExprKind::ArrayLiteral { items, .. } => {
                self.array_literal(items, frame, expr.span, child).map(Some)
            }
            ExprKind::NewArray { element, length } => {
                let element = self.types.specialize(
                    *element,
                    &frame.types,
                    ParameterKind::Method,
                    expr.span,
                )?;
                self.new_array(element, length, frame, expr.span, child)
                    .map(Some)
            }
            ExprKind::ArrayLength(array) => self.array_length(array, frame, child).map(Some),
            ExprKind::ArrayAccess(access) => self.array_access(access, frame, expr.span, child),
            ExprKind::Match(matched) => self.match_expression(matched, frame, expr.span, child),
            ExprKind::Try(block) => match self.block(&block.body, frame, expr.span, child) {
                Err(Stop::TryFailure(error)) => {
                    match (block.error_local, error) {
                        (Some(local), Some(value)) => {
                            frame.locals[usize::from(local)] = Some(value)
                        }
                        (None, None) => {}
                        _ => return Err(unsupported(expr.span).into()),
                    }
                    self.expression(&block.failure, frame, child)
                }
                result => result,
            },
            ExprKind::If {
                condition, yes, no, ..
            } => {
                if self.condition(condition, frame, child)? {
                    self.block(yes, frame, expr.span, child)
                } else if let Some(no) = no {
                    self.expression(no, frame, child)
                } else {
                    Ok(None)
                }
            }
            ExprKind::TypeQuery {
                query,
                ty,
                argument,
            } => {
                let ty =
                    self.types
                        .specialize(*ty, &frame.types, ParameterKind::Method, *argument)?;
                self.query(*query, ty, *argument).map(Some)
            }
            ExprKind::Call(Target::Function(index, type_arguments), args) => {
                let types = type_arguments
                    .iter()
                    .map(|&ty| {
                        self.types
                            .specialize(ty, &frame.types, ParameterKind::Method, expr.span)
                    })
                    .collect::<Result<_, _>>()?;
                let values = args
                    .iter()
                    .map(|arg| self.value(arg, frame, child))
                    .collect::<Evaluation<Vec<_>>>()?;
                self.call(*index, types, values, expr.span, child)
            }
            _ => self
                .scalar_expression(expr, frame, child)
                .map(|value| Some(Evaluated::Scalar(value))),
        }
    }

    fn scalar_expression(
        &mut self,
        expr: &Expr,
        frame: &mut Frame,
        child: usize,
    ) -> Evaluation<ConstValue> {
        use ConstValue::{Bool, F32, F64, Integer, Null, String};
        Ok(match &expr.kind {
            ExprKind::Integer(value) => Integer(*value, expr.ty),
            ExprKind::Float32(value) => F32(*value),
            ExprKind::Float64(value) => F64(*value),
            ExprKind::Boolean(value) => Bool(*value),
            ExprKind::Null => Null,
            ExprKind::String(value) => {
                check_string_size(value.encode_utf16().count(), expr.span)?;
                self.charge(value.len() / 1024, expr.span)?;
                String(Arc::clone(value))
            }
            ExprKind::StringLength(value) => {
                let String(value) = self.scalar(value, frame, child)? else {
                    return Err(unsupported(expr.span).into());
                };
                self.charge(value.len() / 1024, expr.span)?;
                Integer(value.encode_utf16().count() as i64, crate::Type::I32)
            }
            ExprKind::StringIndex(value, index) => {
                let String(value) = self.scalar(value, frame, child)? else {
                    return Err(unsupported(expr.span).into());
                };
                let Integer(index, _) = self.scalar(index, frame, child)? else {
                    return Err(unsupported(expr.span).into());
                };
                self.charge(value.len() / 1024, expr.span)?;
                let unit = usize::try_from(index)
                    .ok()
                    .and_then(|index| value.encode_utf16().nth(index))
                    .ok_or_else(|| {
                        Diagnostic::new(
                            "E0503",
                            expr.span,
                            "string index is out of bounds in constant evaluation",
                        )
                    })?;
                Integer(i64::from(unit), crate::Type::Char)
            }
            ExprKind::Interpolation(parts) => {
                let mut result = std::string::String::new();
                let mut units = 0;
                for part in parts {
                    let value = match self.scalar(part, frame, child)? {
                        String(value) => value,
                        Null => continue,
                        _ => return Err(unsupported(part.span).into()),
                    };
                    units += value.encode_utf16().count();
                    check_string_size(units, expr.span)?;
                    self.charge(value.len() / 1024, expr.span)?;
                    result.push_str(&value);
                }
                String(result.into())
            }
            ExprKind::NumericCast(value, target) => numeric_cast(
                self.scalar(value, frame, child)?,
                *target,
                expr.span,
                self.types.references,
            )?,
            ExprKind::Unary(op, operand) => unary(
                *op,
                self.scalar(operand, frame, child)?,
                self.types.references,
            ),
            ExprKind::Binary(op, left, right) => {
                let left = self.scalar(left, frame, child)?;
                let right = self.scalar(right, frame, child)?;
                if let (String(left), String(right)) = (&left, &right) {
                    self.charge((left.len() + right.len()) / 1024, expr.span)?;
                }
                binary(*op, left, right, expr.span, self.types.references)?
            }
            _ => return Err(unsupported(expr.span).into()),
        })
    }
}

fn unsupported(span: Span) -> Diagnostic {
    Diagnostic::new(
        "E0501",
        span,
        "unsupported operation in constant evaluation",
    )
}
