//! CLR generic constraint resolution and checks.

use std::{collections::HashSet, sync::atomic::Ordering};

use super::{
    Checked, Checker, Expr, ExprKind, GenericBound, Member, ParameterKind, Signature, Structs,
    Target, Type, Value, describe_scoped,
};
use crate::{
    Diagnostic, Span,
    clr::{TypeBound, TypeBoundKind},
    syntax,
};

fn parameter_index(ty: Type) -> Option<u16> {
    match ty {
        Type::MethodParameter(index) | Type::TypeParameter(index) => Some(index),
        _ => None,
    }
}

fn direct_bounds(
    parameter: u16,
    bounds: &[GenericBound],
) -> impl Iterator<Item = TypeBoundKind> + '_ {
    let start = bounds.partition_point(|bound| bound.parameter < parameter);
    bounds[start..]
        .iter()
        .take_while(move |bound| bound.parameter == parameter)
        .map(|bound| bound.kind)
}

fn bound_parameter(kind: TypeBoundKind) -> Option<u16> {
    match kind {
        TypeBoundKind::Target(ty) => parameter_index(ty),
        _ => None,
    }
}

pub(super) fn reachable_bounds(
    actual: Type,
    bounds: &[GenericBound],
) -> impl Iterator<Item = TypeBoundKind> + '_ {
    let mut current = parameter_index(actual).map(|index| direct_bounds(index, bounds));
    let mut seen = HashSet::new();
    let mut pending = Vec::new();
    std::iter::from_fn(move || {
        loop {
            if let Some(bound) = current.as_mut().and_then(Iterator::next) {
                if let Some(index) = bound_parameter(bound)
                    && seen.insert(index)
                {
                    pending.push(index);
                }
                return Some(bound);
            }
            current = Some(direct_bounds(pending.pop()?, bounds));
        }
    })
}

impl Structs<'_, '_> {
    pub(super) fn check_imported_type_bounds(
        &self,
        path: &syntax::Path<'_>,
        arguments: &[Type],
        available: &[GenericBound],
        required: &[TypeBound],
        type_parameters: &[(&str, Span)],
    ) -> Checked<()> {
        if let Some((bound, _)) =
            self.failing_clr_bound(required, available, arguments, None, path.span)?
        {
            let index = usize::from(bound.parameter);
            let constraint = self.describe_type_bound(bound.kind, type_parameters);
            return Err(Diagnostic::new(
                "E0212",
                path.type_arguments[index].span,
                format!(
                    "type argument {} of '{}' does not satisfy the CLR constraint '{constraint}'",
                    index + 1,
                    path.qualified_name()
                ),
            ));
        }
        Ok(())
    }

    fn failing_clr_bound(
        &self,
        required: &[TypeBound],
        available: &[GenericBound],
        owner: &[Type],
        method: Option<&[Type]>,
        span: Span,
    ) -> Checked<Option<(TypeBound, Type)>> {
        for &bound in required {
            let actual = method.unwrap_or(owner)[usize::from(bound.parameter)];
            let kind = if let TypeBoundKind::Target(target) = bound.kind {
                let target = self.import_signature_type(target, owner, span)?;
                let target = self.specialize(
                    target,
                    method.unwrap_or_default(),
                    ParameterKind::ImportedMethod,
                    span,
                )?;
                TypeBoundKind::Target(target)
            } else {
                bound.kind
            };
            if !self.satisfies_generic_argument(actual, available, kind, span)? {
                return Ok(Some((TypeBound { kind, ..bound }, actual)));
            }
        }
        Ok(None)
    }

    fn failing_source_bound(
        &self,
        required: &[GenericBound],
        available: &[GenericBound],
        arguments: &[Type],
        kind: ParameterKind,
        span: Span,
    ) -> Checked<Option<GenericBound>> {
        for &bound in required {
            let actual = arguments[usize::from(bound.parameter)];
            let requirement = match bound.kind {
                TypeBoundKind::Target(target) => {
                    TypeBoundKind::Target(self.specialize(target, arguments, kind, span)?)
                }
                special => special,
            };
            if !self.satisfies_generic_argument(actual, available, requirement, span)? {
                return Ok(Some(GenericBound {
                    kind: requirement,
                    ..bound
                }));
            }
        }
        Ok(None)
    }

    pub(super) fn check_struct_bounds(
        &self,
        id: usize,
        location: impl Fn(usize) -> Span,
        arguments: &[Type],
        available: &[GenericBound],
        type_parameters: &[(&str, Span)],
    ) -> Checked<()> {
        let definition = &self.definitions[id];
        if let Some(bound) = self.failing_source_bound(
            &definition.bounds,
            available,
            arguments,
            ParameterKind::Struct,
            location(0),
        )? {
            let index = usize::from(bound.parameter);
            let target = self.describe_type_bound(bound.kind, type_parameters);
            let actual = describe_scoped(self, type_parameters, arguments[index]);
            return Err(Diagnostic::new(
                "E0212",
                location(index),
                format!(
                    "'{actual}' does not satisfy the bound '{target}' for '{}' in '{}'",
                    definition.type_parameters[index], definition.name,
                ),
            ));
        }
        Ok(())
    }

    fn describe_type_bound(&self, kind: TypeBoundKind, parameters: &[(&str, Span)]) -> String {
        match kind {
            TypeBoundKind::Reference => "class".into(),
            TypeBoundKind::Value => "struct".into(),
            TypeBoundKind::Constructor => "new()".into(),
            TypeBoundKind::Target(target) => describe_scoped(self, parameters, target),
        }
    }

    pub(super) fn satisfies_type_bound(
        &self,
        actual: Type,
        available: &[GenericBound],
        required: TypeBoundKind,
    ) -> bool {
        let value = actual.scalar()
            || actual.imported_value()
            || matches!(actual, Type::Struct(_) | Type::ConstructedStruct(_));
        let declared = |kind| {
            parameter_index(actual)
                .is_some_and(|index| direct_bounds(index, available).any(|bound| bound == kind))
        };
        let targets = || {
            reachable_bounds(actual, available).filter_map(|bound| match bound {
                TypeBoundKind::Target(ty) => Some(ty),
                _ => None,
            })
        };
        match required {
            TypeBoundKind::Reference => {
                (actual.reference() && actual != Type::Null && !actual.is_nullable())
                    || declared(TypeBoundKind::Reference)
                    || targets().any(|ty| {
                        self.imported_id(ty).is_some_and(|id| {
                            !self.references.imported_type(id).is_interface
                                && !self.references.is_value_base(id)
                        })
                    })
            }
            TypeBoundKind::Value => {
                (value && self.nullable_underlying(actual).is_none())
                    || declared(TypeBoundKind::Value)
            }
            TypeBoundKind::Constructor => {
                actual.scalar()
                    || actual.imported_value()
                    || declared(TypeBoundKind::Constructor)
                    || actual == Type::Object
                    || self.imported_id(actual).is_some_and(|id| {
                        self.references.members(id, ".ctor").iter().any(|&method| {
                            self.references
                                .method(method)
                                .signature
                                .parameters
                                .is_empty()
                        })
                    })
            }
            TypeBoundKind::Target(expected) => {
                let compatible = |ty| ty == expected || self.upcasts(ty, expected);
                expected == Type::Object || compatible(actual) || targets().any(compatible)
            }
        }
    }

    fn satisfies_generic_argument(
        &self,
        actual: Type,
        available: &[GenericBound],
        required: TypeBoundKind,
        span: Span,
    ) -> Checked<bool> {
        if required == TypeBoundKind::Constructor && self.definition_of(actual).is_some() {
            if !self.layouts_ready {
                self.deferred_constructor_checks
                    .store(true, Ordering::Relaxed);
                return Ok(true);
            }
            return self.zero_initializable(actual, span);
        }
        Ok(self.satisfies_type_bound(actual, available, required))
    }
}

pub(super) fn resolve(
    parameters: &[(&str, Span)],
    annotations: &[syntax::TypeBound<'_>],
    types: &Structs<'_, '_>,
    from_struct: bool,
) -> Checked<Vec<GenericBound>> {
    let mut bounds = Vec::with_capacity(annotations.len());
    let mut classes = vec![None; parameters.len()];
    let mut seen = HashSet::new();
    let mut special = vec![0u16; parameters.len()];
    for bound in annotations {
        let span = bound.span;
        let kind = match &bound.constraint {
            syntax::Bound::Reference => TypeBoundKind::Reference,
            syntax::Bound::Value => TypeBoundKind::Value,
            syntax::Bound::Constructor => TypeBoundKind::Constructor,
            syntax::Bound::Type(path) => {
                let ty = types.scoped_type(path, parameters, None, false, from_struct)?;
                let definition = types.imported_id(ty).filter(|_| !ty.is_nullable());
                if definition.is_none() && parameter_index(ty).is_none() {
                    return Err(Diagnostic::new(
                        "E0211",
                        span,
                        "a generic bound must name an imported CLR class, interface, or type parameter",
                    ));
                }
                if definition.is_some_and(|id| !types.references.imported_type(id).is_interface)
                    && classes[bound.parameter].replace(ty).is_some()
                {
                    return Err(Diagnostic::new(
                        "E0211",
                        span,
                        "a type parameter can have at most one class bound",
                    ));
                }
                TypeBoundKind::Target(ty)
            }
        };
        if !seen.insert((bound.parameter, kind)) {
            return Err(Diagnostic::new("E0211", span, "duplicate generic bound"));
        }
        let flags = &mut special[bound.parameter];
        *flags |= kind.special_flag();
        if (*flags & 8 != 0 && *flags & 4 != 0)
            || (*flags & 0x0c != 0 && classes[bound.parameter].is_some())
        {
            return Err(Diagnostic::new(
                "E0211",
                span,
                "incompatible generic constraints",
            ));
        }
        bounds.push(GenericBound {
            parameter: bound.parameter as u16,
            kind,
        });
    }
    validate_dependencies(&bounds, annotations, &mut classes, &special, types)?;
    Ok(bounds)
}

fn validate_dependencies(
    bounds: &[GenericBound],
    annotations: &[syntax::TypeBound<'_>],
    classes: &mut [Option<Type>],
    special: &[u16],
    types: &Structs<'_, '_>,
) -> Checked<()> {
    if !bounds
        .iter()
        .any(|bound| bound_parameter(bound.kind).is_some())
    {
        return Ok(());
    }
    let mut dependents = vec![Vec::new(); classes.len()];
    let mut remaining = vec![0; classes.len()];
    for (bound, annotation) in bounds.iter().zip(annotations) {
        if let Some(index) = bound_parameter(bound.kind) {
            let parameter = usize::from(bound.parameter);
            dependents[usize::from(index)].push((parameter, annotation.span));
            remaining[parameter] += 1;
        }
    }
    let mut ready: Vec<_> = (0..classes.len())
        .filter(|&index| remaining[index] == 0)
        .collect();
    while let Some(index) = ready.pop() {
        for &(dependent, span) in &dependents[index] {
            if special[index] & 8 != 0 || (special[dependent] & 8 != 0 && classes[index].is_some())
            {
                return Err(Diagnostic::new(
                    "E0211",
                    span,
                    "dependent type parameters have incompatible value type bounds",
                ));
            }
            if let Some(inherited) = classes[index] {
                match classes[dependent] {
                    None => classes[dependent] = Some(inherited),
                    Some(current) if current == inherited || types.upcasts(current, inherited) => {}
                    Some(current) if types.upcasts(inherited, current) => {
                        classes[dependent] = Some(inherited);
                    }
                    Some(_) => {
                        return Err(Diagnostic::new(
                            "E0211",
                            span,
                            "dependent type parameters have incompatible class bounds",
                        ));
                    }
                }
            }
            remaining[dependent] -= 1;
            if remaining[dependent] == 0 {
                ready.push(dependent);
            }
        }
    }
    if let Some((_, span)) = dependents
        .iter()
        .enumerate()
        .filter(|&(index, _)| remaining[index] != 0)
        .flat_map(|(_, edges)| edges)
        .find(|&&(dependent, _)| remaining[dependent] != 0)
    {
        return Err(Diagnostic::new(
            "E0211",
            *span,
            "cyclic type parameter bounds",
        ));
    }
    Ok(())
}

pub(super) fn validate_arguments(
    parameters: &[(&str, Span)],
    annotations: &[syntax::TypeBound<'_>],
    bounds: &[GenericBound],
    types: &Structs<'_, '_>,
    from_struct: bool,
) -> Checked<()> {
    // All declared bounds are visible while validating their type arguments.
    // This also permits a bound such as T: ISelf<T> when ISelf requires it.
    for bound in annotations {
        if let syntax::Bound::Type(path) = &bound.constraint
            && !path.type_arguments.is_empty()
        {
            types.scoped_type(path, parameters, Some(bounds), false, from_struct)?;
        }
    }
    Ok(())
}

pub(super) fn describe(
    parameters: &[(&str, Span)],
    bounds: &[GenericBound],
    types: &Structs<'_, '_>,
) -> String {
    if bounds.is_empty() {
        return super::generic_parameters(parameters);
    }
    let mut declarations: Vec<String> = parameters.iter().map(|&(name, _)| name.into()).collect();
    let mut previous = None;
    for bound in bounds {
        let delimiter = if previous == Some(bound.parameter) {
            " + "
        } else {
            ": "
        };
        let declaration = &mut declarations[usize::from(bound.parameter)];
        declaration.push_str(delimiter);
        declaration.push_str(&types.describe_type_bound(bound.kind, parameters));
        previous = Some(bound.parameter);
    }
    format!("<{}>", declarations.join(", "))
}

impl Checker<'_, '_> {
    pub(super) fn unsatisfied_clr_bound(
        &self,
        method: Member,
        arguments: &[Type],
        span: Span,
    ) -> Checked<Option<(TypeBound, Type)>> {
        let required = &self.references.method(method.id).bounds;
        if required.is_empty() {
            return Ok(None);
        }
        let owner = self.types.owner_arguments(method.owner);
        self.types.failing_clr_bound(
            required,
            self.bounds,
            owner.as_deref().unwrap_or_default(),
            Some(arguments),
            span,
        )
    }

    pub(super) fn clr_bound_diagnostic(
        &self,
        name: &str,
        bound: TypeBound,
        actual: Type,
        span: Span,
    ) -> Diagnostic {
        let actual = describe_scoped(self.types, self.type_parameters, actual);
        let constraint = self
            .types
            .describe_type_bound(bound.kind, self.type_parameters);
        Diagnostic::new(
            "E0212",
            span,
            format!(
                "type argument {} ('{actual}') of '{name}' does not satisfy the CLR constraint '{constraint}'",
                usize::from(bound.parameter) + 1,
            ),
        )
    }

    pub(super) fn can_upcast(&self, actual: Type, expected: Type) -> bool {
        if let Some(underlying) = self.types.nullable_underlying(expected) {
            return actual == Type::Null
                || (actual == underlying
                    && self.types.nullable_member(expected, ".ctor", 1).is_some());
        }
        if self.types.nullable_underlying(actual).is_some() {
            return expected == Type::NullableObject;
        }
        if actual == Type::Null {
            return expected.is_nullable();
        }
        if actual.is_nullable() && !expected.is_nullable() {
            return false;
        }
        let (from, to) = (actual.non_nullable(), expected.non_nullable());
        if from == to {
            return actual != expected;
        }
        let (actual, expected) = (from, to);
        if expected == Type::Object {
            return !matches!(actual, Type::Void | Type::Never);
        }
        if matches!(actual, Type::MethodParameter(_))
            && (expected.external_reference() || matches!(expected, Type::MethodParameter(_)))
        {
            reachable_bounds(actual, self.bounds).any(|bound| {
                matches!(bound, TypeBoundKind::Target(target)
                    if target == expected || self.types.upcasts(target, expected))
            })
        } else {
            self.types.upcasts(actual, expected)
        }
    }

    pub(super) fn coerce(&self, mut value: Expr, expected: Type) -> Expr {
        if value.ty != Type::Never
            && value.ty != expected
            && self.types.nullable_underlying(expected).is_some()
        {
            let span = value.span;
            if value.ty == Type::Null {
                return Expr {
                    kind: ExprKind::CheckedCast(Box::new(value), expected),
                    ty: expected,
                    span,
                };
            }
            let ctor = self
                .types
                .nullable_member(expected, ".ctor", 1)
                .expect("checked nullable lift");
            return Expr {
                kind: ExprKind::Call(
                    Target::External {
                        method: ctor.id,
                        owner: ctor.owner,
                        arguments: Box::new([]),
                    },
                    vec![value],
                ),
                ty: expected,
                span,
            };
        }
        let generic_upcast = matches!(value.ty, Type::MethodParameter(_))
            && value.ty != expected
            && (expected.external_reference() || matches!(expected, Type::MethodParameter(_)));
        let reference_box = (expected.non_nullable() == Type::Object
            || (value.ty.scalar() || value.ty.imported_value()) && expected.external_reference())
            && !value.ty.reference()
            && !matches!(value.ty, Type::Void | Type::Never);
        if generic_upcast || reference_box {
            let span = value.span;
            let boxed = Expr {
                kind: ExprKind::BoxValue(Box::new(value)),
                ty: if generic_upcast
                    && (expected.is_nullable() || matches!(expected, Type::MethodParameter(_)))
                {
                    Type::NullableObject
                } else {
                    Type::Object
                },
                span,
            };
            return if generic_upcast {
                Expr {
                    kind: ExprKind::CheckedCast(Box::new(boxed), expected),
                    ty: expected,
                    span,
                }
            } else {
                Expr {
                    ty: expected,
                    ..boxed
                }
            };
        }
        if value.ty != Type::Never {
            value.ty = expected;
        }
        value
    }

    pub(super) fn unsatisfied_bound(
        &self,
        signature: &Signature,
        args: &[Type],
        span: Span,
    ) -> Checked<Option<GenericBound>> {
        self.types.failing_source_bound(
            &signature.bounds,
            self.bounds,
            args,
            ParameterKind::Method,
            span,
        )
    }

    pub(super) fn unsatisfied_bound_diagnostic(
        &self,
        bound: GenericBound,
        actual: Type,
        parameter: &str,
        span: Span,
    ) -> Diagnostic {
        let actual = describe_scoped(self.types, self.type_parameters, actual);
        let target = self
            .types
            .describe_type_bound(bound.kind, self.type_parameters);
        Diagnostic::new(
            "E0212",
            span,
            format!("'{actual}' does not satisfy the bound '{target}' for '{parameter}'"),
        )
    }

    pub(super) fn bound_members<M: PartialEq>(
        &self,
        receiver: Type,
        members: impl Fn(Type) -> Vec<M>,
    ) -> Vec<M> {
        match receiver {
            Type::External(_)
            | Type::ConstructedExternal(_)
            | Type::ClrEnum(_)
            | Type::ClrStruct(_)
            | Type::ConstructedClrStruct(_) => members(receiver),
            Type::MethodParameter(_) => {
                let mut class = None;
                let mut interfaces = Vec::new();
                for bound in reachable_bounds(receiver, self.bounds) {
                    let TypeBoundKind::Target(bound) = bound else {
                        continue;
                    };
                    let Some(definition) = self.types.imported_id(bound) else {
                        continue;
                    };
                    if !self.references.imported_type(definition).is_interface {
                        if class.is_none_or(|previous| self.types.upcasts(bound, previous)) {
                            class = Some(bound);
                        }
                        continue;
                    }
                    for member in members(bound) {
                        if !interfaces.contains(&member) {
                            interfaces.push(member);
                        }
                    }
                }
                if let Some(class) = class {
                    let found = members(class);
                    if !found.is_empty() {
                        return found;
                    }
                }
                interfaces
            }
            _ => Vec::new(),
        }
    }

    pub(super) fn constrained_call(
        &mut self,
        method: Member,
        type_arguments: Box<[Type]>,
        receiver: Expr,
        args: Vec<Expr>,
        result: Type,
        span: Span,
    ) -> Checked<(ExprKind, Type)> {
        let temporary = self.constrained_temporary(&receiver, !args.is_empty(), span)?;
        let ty = if args.iter().any(|arg| arg.ty == Type::Never) {
            Type::Never
        } else {
            result
        };
        Ok((
            ExprKind::ConstrainedCall {
                method,
                type_arguments,
                receiver: Box::new(receiver),
                temporary,
                args,
            },
            ty,
        ))
    }
    pub(super) fn constrained_temporary(
        &mut self,
        receiver: &Expr,
        has_arguments: bool,
        span: Span,
    ) -> Checked<Option<u16>> {
        let mutable = self.mutable_local(receiver);
        let capture = if receiver.ty.clr_struct() {
            // Call on a defensive copy unless the receiver is a mutable local.
            !mutable
        } else {
            !matches!(
                receiver.kind,
                ExprKind::Load(Value::Parameter(_) | Value::Local(_) | Value::CapturedCell(..))
            ) || (has_arguments && mutable)
        };
        capture.then(|| self.local(receiver.ty, span)).transpose()
    }

    fn mutable_local(&self, receiver: &Expr) -> bool {
        if matches!(receiver.kind, ExprKind::Load(Value::CapturedCell(..))) {
            return true;
        }
        let ExprKind::Load(Value::Local(index)) = receiver.kind else {
            return false;
        };
        self.scopes.iter().any(|scope| {
            scope.any_binding(|binding| binding.mutable && binding.value == Value::Local(index))
        })
    }

    pub(super) fn require_mutable_struct_receiver(
        &self,
        receiver: &Expr,
        writes: bool,
        span: Span,
    ) -> Checked<()> {
        if writes && receiver.ty.clr_struct() && !self.mutable_local(receiver) {
            return Err(Diagnostic::new(
                "E0204",
                span,
                "assigning a CLR struct member requires a mutable local receiver",
            ));
        }
        Ok(())
    }
}
