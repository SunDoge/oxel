//! Evaluates constants after type checking with the ordinary HIR checker.

mod cache;
mod dependencies;
mod interpreter;
mod reflection;

pub(crate) use cache::EvaluationCache;
use cache::Fingerprints;
use interpreter::{Aggregate, Evaluated};
pub(crate) use reflection::TypeQuery;
pub(super) use reflection::builtin_field_info;

use super::{
    Block, Checked, Checker, Expr, ExprKind, ModuleScope, Statement, Structs, Type, Value,
};
use crate::{
    Diagnostic, Occurrence, OccurrenceKind, References, Span,
    analysis::BodyCache,
    syntax::{self, Binary, Unary},
};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, OnceLock},
};

pub(super) type Values<'src> = HashMap<std::borrow::Cow<'src, str>, Constant>;
const MATERIALIZATION_LIMIT: usize = 100_000;
const TYPE_SHAPE_LIMIT: usize = 100_000;

pub(super) struct Constant {
    value: Evaluated,
    ty: Type,
    aggregate_name: Option<String>,
    pub definition: Span,
    fingerprint: OnceLock<blake3::Hash>,
}

impl Constant {
    pub fn detail(&self, name: &str) -> String {
        match &self.aggregate_name {
            Some(ty) => format!("const {name}: {ty}"),
            None => format!("const {name}: {}", self.ty),
        }
    }
    pub fn fingerprint(&self) -> blake3::Hash {
        *self.fingerprint.get_or_init(|| {
            let value = self.value.fingerprint();
            let ty = match &self.aggregate_name {
                Some(ty) => ty.as_bytes(),
                None if self.ty == Type::NullableString => b"nullable string",
                None => return value,
            };
            let mut hash = blake3::Hasher::new();
            hash.update(value.as_bytes());
            hash.update(ty);
            hash.finalize()
        })
    }

    pub(super) fn expression(&self, checker: &mut Checker<'_, '_>, span: Span) -> Checked<Expr> {
        if let Evaluated::Scalar(value) = &self.value {
            return Ok(value.expression(self.ty, span));
        }
        checker.track_constant_type(self.ty);
        let mut materialization = Materialization {
            locals: HashMap::new(),
            statements: Vec::new(),
            remaining: MATERIALIZATION_LIMIT,
        };
        let value = materialization.expression(&self.value, self.ty, checker, span)?;
        Ok(Expr {
            kind: ExprKind::Block(Block {
                statements: materialization.statements,
                tail: Some(Box::new(value)),
                ty: self.ty,
            }),
            ty: self.ty,
            span,
        })
    }
}

impl Evaluated {
    fn fingerprint(&self) -> blake3::Hash {
        match self {
            Self::Scalar(value) => value.fingerprint(),
            Self::Aggregate(aggregate) => *aggregate.fingerprint.get_or_init(|| {
                let mut hash = blake3::Hasher::new();
                hash.update(b"aggregate");
                for (index, field) in &aggregate.fields {
                    hash.update(&(*index as u64).to_le_bytes());
                    hash.update(field.fingerprint().as_bytes());
                }
                hash.finalize()
            }),
            Self::Array(_) => unreachable!("mutable array cannot escape constant evaluation"),
        }
    }
}

struct Materialization {
    locals: HashMap<(*const Aggregate, Type), u16>,
    statements: Vec<Statement>,
    remaining: usize,
}

impl Materialization {
    fn expression(
        &mut self,
        value: &Evaluated,
        ty: Type,
        checker: &mut Checker<'_, '_>,
        span: Span,
    ) -> Checked<Expr> {
        self.remaining = self.remaining.checked_sub(1).ok_or_else(|| {
            Diagnostic::new("E0504", span, "constant value exceeds its expansion limit")
        })?;
        let data = match value {
            Evaluated::Scalar(value) => return Ok(value.expression(ty, span)),
            Evaluated::Aggregate(data) => data,
            Evaluated::Array(_) => unreachable!("mutable array cannot escape constant evaluation"),
        };
        let key = (Arc::as_ptr(data), ty);
        let local = if let Some(&local) = self.locals.get(&key) {
            local
        } else {
            let types = checker.types;
            let id = types.definition_of(ty).expect("typed constant aggregate");
            let mut fields = Vec::with_capacity(data.fields.len());
            for (index, value) in &data.fields {
                let field = types.field_type(ty, types.definitions[id].fields[*index].ty, span)?;
                fields.push((*index, self.expression(value, field, checker, span)?));
            }
            let local = checker.local(ty, span)?;
            self.statements.push(Statement::Store {
                target: Value::Local(local),
                value: Expr {
                    kind: ExprKind::Construct { ty, local, fields },
                    ty,
                    span,
                },
                span,
            });
            self.locals.insert(key, local);
            local
        };
        Ok(Expr {
            kind: ExprKind::Load(Value::Local(local)),
            ty,
            span,
        })
    }
}

#[derive(Clone)]
pub(super) enum ConstValue {
    Integer(i64, Type),
    F32(f32),
    F64(f64),
    Bool(bool),
    Null,
    String(Arc<str>),
}

impl ConstValue {
    fn ty(&self) -> Type {
        match self {
            Self::Integer(_, ty) => *ty,
            Self::F32(_) => Type::F32,
            Self::F64(_) => Type::F64,
            Self::Bool(_) => Type::Bool,
            Self::Null => Type::Null,
            Self::String(_) => Type::String,
        }
    }

    fn fingerprint(&self) -> blake3::Hash {
        let mut hash = blake3::Hasher::new();
        // The resolved type matters even for numerically equal values.
        let tag = match self.ty() {
            Type::I8 => 0,
            Type::U8 => 1,
            Type::I16 => 2,
            Type::I32 => 3,
            Type::I64 => 4,
            Type::F32 => 5,
            Type::F64 => 6,
            Type::Bool => 7,
            Type::String => 8,
            Type::Char => 9,
            Type::ClrEnum(_) => 10,
            Type::U16 => 11,
            Type::U32 => 12,
            Type::U64 => 13,
            Type::Null => 14,
            _ => unreachable!("primitive constant type"),
        };
        hash.update(&[tag]);
        if let Type::ClrEnum(id) = self.ty() {
            hash.update(&(id as u64).to_le_bytes());
        }
        match self {
            Self::Integer(value, _) => hash.update(&value.to_le_bytes()),
            Self::F32(value) => hash.update(&value.to_bits().to_le_bytes()),
            Self::F64(value) => hash.update(&value.to_bits().to_le_bytes()),
            Self::Bool(value) => hash.update(&[u8::from(*value)]),
            Self::String(value) => hash.update(value.as_bytes()),
            Self::Null => return hash.finalize(),
        };
        hash.finalize()
    }

    fn expression(&self, ty: Type, span: Span) -> Expr {
        let kind = match self {
            Self::Integer(value, _) => ExprKind::Integer(*value),
            // NaN payloads from host arithmetic need not match across targets.
            Self::F32(value) => ExprKind::Float32(if value.is_nan() { f32::NAN } else { *value }),
            Self::F64(value) => ExprKind::Float64(if value.is_nan() { f64::NAN } else { *value }),
            Self::Bool(value) => ExprKind::Boolean(*value),
            Self::Null => ExprKind::Null,
            Self::String(value) => ExprKind::String(Arc::clone(value)),
        };
        Expr { kind, ty, span }
    }
}

fn supported_scalar(ty: Type) -> bool {
    ty.scalar() || matches!(ty, Type::String | Type::NullableString | Type::ClrEnum(_))
}

#[derive(Clone, Copy)]
pub(super) enum ValueContext {
    Constant,
    Function,
    Instantiation,
}

pub(super) fn supported_value(
    types: &Structs<'_, '_>,
    ty: Type,
    context: ValueContext,
    span: Span,
) -> Checked<bool> {
    if supported_scalar(ty) {
        return Ok(true);
    }
    let mut pending = vec![ty];
    let mut seen = HashSet::new();
    while let Some(ty) = pending.pop() {
        if supported_scalar(ty)
            || matches!(context, ValueContext::Function) && matches!(ty, Type::MethodParameter(_))
            || !seen.insert(ty)
        {
            continue;
        }
        if seen.len() > TYPE_SHAPE_LIMIT {
            return Err(Diagnostic::new(
                "E0504",
                span,
                "compile-time type exceeds its shape limit",
            ));
        }
        if !matches!(context, ValueContext::Constant)
            && let Type::Array(_) = ty
        {
            pending.push(types.array_element(ty).expect("CLR vector"));
            continue;
        }
        let Some(id) = types.definition_of(ty) else {
            return Ok(false);
        };
        for field in &types.definitions[id].fields {
            pending.push(types.field_type(ty, field.ty, span)?);
        }
    }
    Ok(true)
}

pub(super) fn evaluate<'src>(
    declarations: &[&syntax::ConstDefinition<'src>],
    module: &mut ModuleScope<'_, 'src, '_>,
    mut occurrences: Option<&mut Vec<Occurrence>>,
    mut cache: Option<&mut BodyCache>,
) -> Checked<()> {
    let first_function = module.runtime_functions().len();
    if declarations.is_empty() && first_function == module.functions.len() {
        return Ok(());
    }
    let mut names = HashMap::new();
    for (id, declaration) in declarations.iter().enumerate() {
        let key = module
            .types
            .scopes
            .declaration(declaration.name, declaration.name_span);
        let duplicate = names
            .insert(key.clone(), id)
            .map(|previous: usize| declarations[previous].name_span)
            .or_else(|| {
                module
                    .names
                    .get(key.as_ref())
                    .map(|&index| module.functions[index].name_span)
            });
        if let Some(previous) = duplicate {
            let span = if previous.start > declaration.name_span.start {
                previous
            } else {
                declaration.name_span
            };
            return Err(Diagnostic::new(
                "E0201",
                span,
                format!("duplicate value '{}'", declaration.name),
            ));
        }
    }

    let dependencies = dependencies::Dependencies::new(module, &names, first_function);
    let mut edges = Vec::new();
    for declaration in declarations {
        edges.push(dependencies.initializer(&declaration.value)?);
    }
    for index in first_function..module.functions.len() {
        edges.push(dependencies.function(index)?);
    }
    let groups = dependencies::groups(&edges);
    let cycle = groups
        .iter()
        .filter(|group| group.len() > 1 || edges[group[0]].contains(&group[0]))
        .flatten()
        .copied()
        .filter(|&id| id < declarations.len())
        .min();
    if let Some(id) = cycle {
        let declaration = declarations[id];
        return Err(Diagnostic::new(
            "E0502",
            declaration.name_span,
            format!("constant '{}' depends on a cycle", declaration.name),
        ));
    }

    let mut checked = Vec::with_capacity(module.functions.len());
    checked.resize_with(module.functions.len(), || None);
    let mut fingerprints = cache.as_ref().map(|_| Fingerprints::new(edges.len()));
    for group in groups {
        if let [node] = group.as_slice()
            && let Some(declaration) = declarations.get(*node)
        {
            let evaluation = cache
                .as_deref_mut()
                .zip(fingerprints.as_ref())
                .map(|(cache, fingerprints)| (cache, fingerprints, edges[*node].as_slice()));
            let constant = initializer(
                module,
                declaration,
                &checked,
                occurrences.as_deref_mut(),
                evaluation,
            )?;
            if let Some(fingerprints) = &mut fingerprints {
                fingerprints.record(*node, constant.fingerprint());
            }
            let key = module
                .types
                .scopes
                .declaration(declaration.name, declaration.name_span);
            module.constants.insert(key, constant);
        } else {
            let mut lookups = Vec::new();
            for &node in &group {
                let index = node - declarations.len() + first_function;
                checked[index] = Some(module.check(
                    index,
                    occurrences.as_deref_mut(),
                    fingerprints.is_some().then_some(&mut lookups),
                )?);
            }
            if let Some((cache, fingerprints)) = cache.as_deref().zip(fingerprints.as_mut()) {
                fingerprints.functions(
                    cache,
                    &group,
                    &edges,
                    &module.functions[first_function..],
                    declarations.len(),
                    &lookups,
                );
            }
        }
    }
    Ok(())
}

fn initializer<'src>(
    module: &ModuleScope<'_, 'src, '_>,
    declaration: &syntax::ConstDefinition<'src>,
    functions: &[Option<super::Function>],
    occurrences: Option<&mut Vec<Occurrence>>,
    evaluation: Option<(&mut BodyCache, &Fingerprints, &[usize])>,
) -> Checked<Constant> {
    let types = &module.types;
    let mut lookups = Vec::new();
    let mut checker = Checker {
        async_fn: false,
        in_test: false,
        comptime: true,
        names: &module.names,
        functions: &module.functions,
        signatures: &module.signatures,
        constants: &module.constants,
        references: module.references,
        types,
        result: Type::Void,
        type_parameters: &[],
        bounds: &[],
        scopes: Vec::new(),
        locals: Vec::new(),
        uninitialized: Vec::new(),
        loop_depth: 0,
        try_context: None,
        has_exception_regions: false,
        nonnull: Vec::new(),
        capture: None,
        captured_cells: Default::default(),
        lambdas: Vec::new(),
        lookups: evaluation.is_some().then_some(&mut lookups),
        occurrences,
    };
    let annotation = declaration.annotation.as_ref();
    let expected = annotation
        .map(|path| checker.scoped_value_type(path))
        .transpose()?;
    if let Some((path, ty)) = annotation.zip(expected) {
        if !supported_value(types, ty, ValueContext::Constant, path.span)? {
            return Err(Diagnostic::new(
                "E0501",
                path.span,
                "constants require supported value types",
            ));
        }
        checker.record_annotation(path, ty);
    }
    let typed = checker.expression(&declaration.value, expected)?;
    if !matches!(typed.ty, Type::Void | Type::Never)
        && !supported_value(types, typed.ty, ValueContext::Constant, typed.span)?
    {
        return Err(Diagnostic::new(
            "E0501",
            typed.span,
            "constants require supported value types",
        ));
    }
    let aggregate_name = types
        .definition_of(typed.ty)
        .map(|_| types.describe(typed.ty));
    if aggregate_name.is_some() {
        checker.track_constant_type(typed.ty);
    }
    let local_count = checker.locals.len();
    let run = || interpreter::evaluate(&typed, local_count, functions, types);
    let value = if let Some((cache, fingerprints, dependencies)) = evaluation {
        let lookups = checker
            .lookups
            .as_deref()
            .expect("cached constant tracks lookups");
        let signature = fingerprints.initializer(cache, declaration, dependencies, lookups);
        cache.evaluation.value(declaration.name, signature, run)?
    } else {
        run()?
    };
    let constant = Constant {
        value,
        ty: typed.ty,
        aggregate_name,
        definition: declaration.name_span,
        fingerprint: OnceLock::new(),
    };
    checker.record_declaration(declaration.name_span, OccurrenceKind::Constant, || {
        constant.detail(declaration.name)
    });
    Ok(constant)
}

fn unary(op: Unary, operand: ConstValue, references: &References) -> ConstValue {
    use ConstValue::{Bool, F32, F64, Integer};
    match (op, operand) {
        (Unary::Negate, Integer(value, ty)) => Integer(wrap_integer(value.wrapping_neg(), ty), ty),
        (Unary::Negate, F32(value)) => F32(-value),
        (Unary::Negate, F64(value)) => F64(-value),
        (Unary::Not, Bool(value)) => Bool(!value),
        (Unary::Complement, Integer(value, ty)) => {
            Integer(wrap_integer(!value, references.integer_storage(ty)), ty)
        }
        _ => unreachable!("checked constant unary operand"),
    }
}

fn numeric_cast(
    value: ConstValue,
    target: Type,
    span: Span,
    references: &References,
) -> Checked<ConstValue> {
    use ConstValue::{F32, F64, Integer};
    let result = match (value, references.integer_storage(target)) {
        (Integer(value, ty), Type::F32) => {
            F32(references.integer_storage(ty).integer_value(value) as f32)
        }
        (Integer(value, ty), Type::F64) => {
            F64(references.integer_storage(ty).integer_value(value) as f64)
        }
        (F32(value), Type::F64) => F64(f64::from(value)),
        (F64(value), Type::F32) => F32(value as f32),
        (value, ty) if ty.integer() || ty == Type::Char => {
            let integer = match value {
                Integer(integer, source) => {
                    Some(references.integer_storage(source).integer_value(integer))
                }
                F32(float) => checked_float_integer(f64::from(float)),
                F64(float) => checked_float_integer(float),
                _ => unreachable!("checked numeric operand"),
            };
            let integer = integer
                .filter(|&value| {
                    if ty == Type::Char {
                        u16::try_from(value).is_ok()
                    } else {
                        ty.contains_integer(value)
                    }
                })
                .ok_or_else(|| {
                    let name = match target {
                        Type::ClrEnum(id) => &references.imported_type(id).qualified_name,
                        _ => &target.to_string(),
                    };
                    Diagnostic::new(
                        "E0503",
                        span,
                        format!("numeric cast to {name} overflows in constant evaluation"),
                    )
                })?;
            Integer(integer as i64, target)
        }
        _ => unreachable!("checked numeric cast"),
    };
    Ok(result)
}

fn checked_float_integer(value: f64) -> Option<i128> {
    let integer = value.trunc();
    // u64::MAX rounds up as f64. Every supported integer fits in this interval.
    (i64::MIN as f64..u64::MAX as f64)
        .contains(&integer)
        .then_some(integer as i128)
}

fn binary(
    op: Binary,
    left: ConstValue,
    right: ConstValue,
    span: Span,
    references: &References,
) -> Checked<ConstValue> {
    use ConstValue::{Bool, F32, F64, Integer, Null, String};
    Ok(match (left, right) {
        (Integer(left, ty), Integer(right, _)) => integer(op, left, right, ty, span, references)?,
        (F32(left), F32(right)) => floating(op, left, right, F32),
        (F64(left), F64(right)) => floating(op, left, right, F64),
        (Bool(left), Bool(right)) => Bool(match op {
            Binary::BitAnd => left & right,
            Binary::BitOr => left | right,
            Binary::BitXor => left ^ right,
            _ => equality(op, left, right),
        }),
        (String(left), String(right)) if matches!(op, Binary::Add) => {
            let units = left.encode_utf16().count() + right.encode_utf16().count();
            check_string_size(units, span)?;
            let mut text = std::string::String::with_capacity(left.len() + right.len());
            text.push_str(&left);
            text.push_str(&right);
            String(Arc::from(text))
        }
        (String(left), String(right)) => Bool(equality(op, left, right)),
        (Null, Null) => Bool(matches!(op, Binary::Eq)),
        (Null, String(_)) | (String(_), Null) => Bool(matches!(op, Binary::Ne)),
        _ => unreachable!("checked constant binary operands"),
    })
}

fn equality<T: PartialEq>(op: Binary, left: T, right: T) -> bool {
    match op {
        Binary::Eq => left == right,
        Binary::Ne => left != right,
        _ => unreachable!("checked constant equality operator"),
    }
}

fn floating<T>(op: Binary, left: T, right: T, wrap: fn(T) -> ConstValue) -> ConstValue
where
    T: PartialOrd
        + std::ops::Add<Output = T>
        + std::ops::Sub<Output = T>
        + std::ops::Mul<Output = T>
        + std::ops::Div<Output = T>
        + std::ops::Rem<Output = T>,
{
    use Binary::*;
    use ConstValue::Bool;
    match op {
        Add => wrap(left + right),
        Sub => wrap(left - right),
        Mul => wrap(left * right),
        Div => wrap(left / right),
        Rem => wrap(left % right),
        Eq | Ne => Bool(equality(op, left, right)),
        Lt => Bool(left < right),
        Le => Bool(left <= right),
        Gt => Bool(left > right),
        Ge => Bool(left >= right),
        And | Or | Coalesce => unreachable!("logical operators lower as branches"),
        BitAnd | BitOr | BitXor | Shl | Shr => unreachable!("checked floating-point operation"),
    }
}

fn integer(
    op: Binary,
    left: i64,
    right: i64,
    ty: Type,
    span: Span,
    references: &References,
) -> Checked<ConstValue> {
    use Binary::*;
    use ConstValue::Bool;
    if matches!(op, Div | Rem) {
        if right == 0 {
            return Err(Diagnostic::new(
                "E0503",
                span,
                "division or remainder by zero in constant",
            ));
        }
        if right == -1
            && ((ty == Type::I32 && left == i64::from(i32::MIN))
                || (ty == Type::I64 && left == i64::MIN))
        {
            return Err(Diagnostic::new(
                "E0503",
                span,
                "signed division or remainder overflows in constant",
            ));
        }
    }
    let value = match op {
        Add => left.wrapping_add(right),
        Sub => left.wrapping_sub(right),
        Mul => left.wrapping_mul(right),
        Div => (ty.integer_value(left) / ty.integer_value(right)) as i64,
        Rem => (ty.integer_value(left) % ty.integer_value(right)) as i64,
        BitAnd => left & right,
        BitOr => left | right,
        BitXor => left ^ right,
        Shl | Shr => {
            let count = right as u32 & ty.shift_mask().expect("checked integer shift");
            match op {
                Shl => left.wrapping_shl(count),
                Shr if ty.unsigned() => ((left as u64) >> count) as i64,
                _ => left >> count,
            }
        }
        Eq | Ne => return Ok(Bool(equality(op, left, right))),
        Lt => return Ok(Bool(ty.integer_value(left) < ty.integer_value(right))),
        Le => return Ok(Bool(ty.integer_value(left) <= ty.integer_value(right))),
        Gt => return Ok(Bool(ty.integer_value(left) > ty.integer_value(right))),
        Ge => return Ok(Bool(ty.integer_value(left) >= ty.integer_value(right))),
        And | Or | Coalesce => unreachable!("logical operators lower as branches"),
    };
    Ok(ConstValue::Integer(
        wrap_integer(value, references.integer_storage(ty)),
        ty,
    ))
}

fn wrap_integer(value: i64, ty: Type) -> i64 {
    match ty {
        Type::I8 => i64::from(value as i8),
        Type::U8 => i64::from(value as u8),
        Type::I16 => i64::from(value as i16),
        Type::U16 => i64::from(value as u16),
        Type::I32 => i64::from(value as i32),
        Type::U32 => i64::from(value as u32),
        Type::I64 | Type::U64 => value,
        _ => unreachable!("checked integer result"),
    }
}

fn check_string_size(utf16_units: usize, span: Span) -> Checked<()> {
    // One byte for the initial heap entry, four for the compressed length,
    // and one for the terminal flag must fit alongside the UTF-16 contents.
    if utf16_units > (1 << 23) - 3 {
        return Err(Diagnostic::new(
            "E0503",
            span,
            "constant string exceeds the CLR user string heap limit",
        ));
    }
    Ok(())
}
