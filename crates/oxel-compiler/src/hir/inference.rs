//! Call-site inference for method type parameters.

use super::{
    Checked, Checker, Expr, ExprKind, ParameterKind, Signature, Structs, Type, describe_scoped,
};
use crate::{
    ArgumentMode, Diagnostic, Span,
    syntax::{self, Expression},
};

struct Inference {
    bindings: Vec<Option<Binding>>,
    kind: ParameterKind,
}

#[derive(Clone, Copy)]
struct Binding {
    ty: Type,
    annotated: bool,
}

impl Inference {
    fn new(arity: u16, kind: ParameterKind) -> Self {
        Self {
            bindings: vec![None; usize::from(arity)],
            kind,
        }
    }

    fn get(&self, index: u16) -> Option<Type> {
        self.bindings[usize::from(index)].map(|binding| binding.ty)
    }

    fn bind(&mut self, index: u16, actual: Type, annotated: bool) -> Result<(), (u16, Type, Type)> {
        if matches!(actual, Type::Void | Type::Never | Type::Null) {
            return Ok(());
        }
        let binding = &mut self.bindings[usize::from(index)];
        let ty = if annotated {
            actual.non_nullable()
        } else {
            actual
        };
        if let Some(previous) = *binding {
            let match_underlying = (annotated || previous.annotated)
                && previous.ty.non_nullable() == ty.non_nullable();
            if previous.ty != ty && !match_underlying {
                return Err((index, previous.ty, ty));
            }
            // An unannotated occurrence can establish the argument's nullability.
            if !previous.annotated {
                return Ok(());
            }
        }
        *binding = Some(Binding { ty, annotated });
        Ok(())
    }

    fn observe(
        &mut self,
        types: &Structs<'_, '_>,
        pattern: Type,
        actual: Type,
    ) -> Result<(), (u16, Type, Type)> {
        if let Some(index) = self.kind.index(pattern) {
            return self.bind(index, actual, pattern.is_nullable());
        }
        let Some(pattern) = pattern.instance_id().map(|id| types.instance(id)) else {
            return Ok(());
        };
        let matches_pattern = |instance: &super::TypeInstance| pattern.kind == instance.kind;
        let mut observe_arguments = |actual: &super::TypeInstance| {
            for (&pattern, &actual) in pattern.arguments.iter().zip(actual.arguments.iter()) {
                self.observe(types, pattern, actual)?;
            }
            Ok(())
        };
        if let Some(direct) = actual.instance_id().map(|id| types.instance(id))
            && matches_pattern(&direct)
        {
            return observe_arguments(&direct);
        }
        if matches!(pattern.kind, super::InstanceKind::External(_))
            && (actual.external_reference() || actual.imported_value())
        {
            for ancestor in types.ancestors(actual.non_nullable(), true) {
                if let Some(instance) = ancestor.instance_id().map(|id| types.instance(id))
                    && matches_pattern(&instance)
                {
                    observe_arguments(&instance)?;
                }
            }
        }
        Ok(())
    }

    fn finish(self) -> Option<Vec<Type>> {
        self.bindings
            .into_iter()
            .map(|binding| binding.map(|binding| binding.ty))
            .collect()
    }
}

pub(super) fn argument_order<'a>(
    args: &'a [syntax::Expr<'_>],
    delay_literals: bool,
) -> impl Iterator<Item = usize> + 'a {
    let is_literal = |index: usize| args[index].is_numeric_literal();
    (0..args.len())
        .filter(move |&index| !delay_literals || !is_literal(index))
        .chain((0..args.len()).filter(move |&index| delay_literals && is_literal(index)))
}

pub(super) fn external_argument_hint(
    types: &Structs<'_, '_>,
    signature: &Signature,
    parameter: u16,
    typed: &[Option<Expr>],
) -> Option<Type> {
    let mut inference = Inference::new(signature.type_arity, ParameterKind::ImportedMethod);
    for (&pattern, value) in signature.parameters.iter().zip(typed) {
        if let Some(value) = value {
            inference
                .observe(types, pattern, external_argument_type(value))
                .ok()?;
        }
    }
    inference.get(parameter)
}

pub(super) fn infer_external_arguments(
    types: &Structs<'_, '_>,
    signature: &Signature,
    sources: &[syntax::Expr<'_>],
    values: &[Expr],
) -> Option<Vec<Type>> {
    let mut inference = Inference::new(signature.type_arity, ParameterKind::ImportedMethod);
    for index in argument_order(sources, true) {
        let pattern = signature.parameters[index];
        let actual = external_argument_type(&values[index]);
        if let Some(parameter) = ParameterKind::ImportedMethod.index(pattern)
            && let Some(expected) = inference.get(parameter)
        {
            let contextual = match (&sources[index].kind, &values[index].kind) {
                (Expression::Int { .. }, ExprKind::Integer(value)) => {
                    actual == Type::I32 && expected.contains_integer(i128::from(*value))
                }
                (Expression::Float { .. }, ExprKind::Float64(_)) => expected == Type::F32,
                _ => false,
            };
            if contextual {
                continue;
            }
        }
        inference.observe(types, pattern, actual).ok()?;
    }
    inference.finish()
}

fn external_argument_type(value: &Expr) -> Type {
    if matches!(value.kind, ExprKind::LocalAddress(_, ArgumentMode::Out)) {
        value.ty.non_nullable()
    } else {
        value.ty
    }
}

impl<'src> Checker<'_, 'src> {
    pub(super) fn infer_call_arguments(
        &mut self,
        declaration: &syntax::Function<'src>,
        signature: &Signature,
        args: &[syntax::Expr<'src>],
        expected: Option<Type>,
        span: Span,
    ) -> Checked<(Vec<Type>, Vec<Expr>)> {
        let types = self.types;
        let mut inferred = Inference::new(signature.type_arity, ParameterKind::Method);
        let mut typed: Vec<_> = (0..args.len()).map(|_| None).collect();
        let mut literals = Vec::new();
        let mut contextual = Vec::new();
        let type_parameters = self.type_parameters;
        let observe = |inferred: &mut Inference, pattern, actual, location| {
            inferred
                .observe(types, pattern, actual)
                .map_err(|(index, a, b)| {
                    let parameter = declaration.type_parameters[usize::from(index)].0;
                    let a = describe_scoped(types, type_parameters, a);
                    let b = describe_scoped(types, type_parameters, b);
                    Diagnostic::new(
                        "E0210",
                        location,
                        format!(
                            "conflicting types for '{parameter}' in '{}': {a} and {b}",
                            declaration.name
                        ),
                    )
                })
        };

        // Determine types from self-typed values before contextualizing decimal
        // literals and generic struct literals. Keep the original argument order
        // in HIR so these compile-time passes do not change evaluation order.
        for (index, (source, &pattern)) in args.iter().zip(&signature.parameters).enumerate() {
            match &source.kind {
                Expression::Int { .. } | Expression::Float { .. } => literals.push(index),
                Expression::Construct { path, .. }
                    if path.type_arguments.is_empty()
                        && types
                            .names
                            .get(path.segments[0].0)
                            .is_some_and(|&id| types.arities[id] != 0) =>
                {
                    contextual.push((index, None));
                }
                _ => {
                    // An open formal parameter supplies inference evidence, rather
                    // than coercing an argument to an earlier argument's type.
                    let hint = types.substitute_parameters(
                        pattern,
                        ParameterKind::Method,
                        source.span,
                        |_| None,
                    )?;
                    let locals = self.locals.len();
                    let uninitialized = self.uninitialized.len();
                    let occurrences = self.occurrences.as_ref().map(|items| items.len());
                    let value = match self.expression(source, hint) {
                        Err(error) if hint.is_none() && error.code == "E0210" => {
                            // Recheck a nested call only if later arguments or
                            // the result context can supply a complete hint.
                            self.locals.truncate(locals);
                            self.uninitialized.truncate(uninitialized);
                            if let Some((items, len)) = self.occurrences.as_mut().zip(occurrences) {
                                items.truncate(len);
                            }
                            contextual.push((index, Some(error)));
                            continue;
                        }
                        value => value?,
                    };
                    observe(&mut inferred, pattern, value.ty, source.span)?;
                    typed[index] = Some(value);
                }
            }
        }
        let mut result_context = Inference::new(signature.type_arity, ParameterKind::Method);
        if let Some(expected) = expected
            && result_context
                .observe(types, signature.result, expected.non_nullable())
                .is_err()
        {
            result_context = Inference::new(signature.type_arity, ParameterKind::Method);
        }
        for (index, prior_error) in literals
            .into_iter()
            .map(|index| (index, None))
            .chain(contextual)
        {
            let source = &args[index];
            let pattern = signature.parameters[index];
            let hint = types.substitute_parameters(
                pattern,
                ParameterKind::Method,
                source.span,
                |index| {
                    inferred.get(index).or_else(|| {
                        // An object result can box the inferred numerical result.
                        // Preserve the literal's own type as the inference evidence.
                        result_context
                            .get(index)
                            .filter(|ty| *ty != Type::Object || !source.is_numeric_literal())
                    })
                },
            )?;
            if let Some(error) = prior_error
                && hint.is_none()
            {
                return Err(error);
            }
            let value = self.expression(source, hint)?;
            observe(&mut inferred, pattern, value.ty, source.span)?;
            typed[index] = Some(value);
        }

        for (binding, fallback) in inferred.bindings.iter_mut().zip(result_context.bindings) {
            *binding = binding.or(fallback);
        }

        let arguments = inferred
            .bindings
            .into_iter()
            .zip(&declaration.type_parameters)
            .map(|(ty, &(name, _))| {
                ty.map(|binding| binding.ty).ok_or_else(|| {
                Diagnostic::new("E0210", span, format!(
                    "cannot infer type parameter '{name}' of '{}'; supply explicit type arguments",
                    declaration.name,
                ))
            })
            })
            .collect::<Checked<Vec<_>>>()?;
        let typed = typed
            .into_iter()
            .zip(&signature.parameters)
            .map(|(value, &pattern)| {
                let value = value.expect("all arguments checked");
                let expected =
                    types.specialize(pattern, &arguments, ParameterKind::Method, value.span)?;
                self.require(value.ty, expected, value.span)?;
                Ok(self.coerce(value, expected))
            })
            .collect::<Checked<Vec<_>>>()?;
        Ok((arguments, typed))
    }
}
