//! Resolves the framework types and members used by async method lowering.

use super::{Checked, Checker, Expr, ExprKind, InstanceKind, Member, Structs, Target, Type};
use crate::{Diagnostic, Span, syntax};

#[derive(Clone, Copy)]
pub(crate) struct Plan {
    pub frame: Type,
    pub output: Type,
    pub completion: Type,
    pub constructor: Member,
    pub get_task: Member,
    pub set_result: Member,
    pub set_exception: Member,
    pub cancellation: Type,
    pub cancellation_token: Member,
    pub set_canceled: Member,
    pub action: Type,
    pub action_definition: usize,
    pub context: Type,
    pub capture_context: Member,
    pub run_context: Member,
    pub context_callback: Type,
    pub context_callback_definition: usize,
}

pub(crate) struct Suspension {
    pub awaiter: Expr,
    pub completed: Member,
    pub subscribe: Member,
    pub result: Member,
}

pub(super) fn error(span: Span, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new("E0351", span, message)
}

fn framework_definition(types: &Structs<'_, '_>, name: &str, span: Span) -> Checked<usize> {
    types
        .references
        .type_candidates(name, false)
        .find(|&id| types.references.is_framework_type(id))
        .ok_or_else(|| {
            error(
                span,
                format!("async code requires --framework and CLR type '{name}'"),
            )
        })
}

fn framework_generic(
    types: &Structs<'_, '_>,
    name: &str,
    result: Type,
    span: Span,
) -> Checked<Type> {
    let (name, arguments) = if result == Type::Void {
        (name.to_owned(), Vec::new())
    } else {
        (format!("{name}`1"), vec![result])
    };
    let id = framework_definition(types, &name, span)?;
    types.constructed_type(InstanceKind::External(id), arguments, span)
}

pub(super) fn task_type(types: &Structs<'_, '_>, result: Type, span: Span) -> Checked<Type> {
    framework_generic(types, "System.Threading.Tasks.Task", result, span)
}

pub(super) fn output(types: &Structs<'_, '_>, task: Type) -> Type {
    types
        .owner_arguments(Some(task))
        .map_or(Type::Void, |arguments| arguments[0])
}

fn member(
    types: &Structs<'_, '_>,
    owner: Type,
    name: &str,
    parameters: &[Type],
    span: Span,
) -> Checked<(Member, Type)> {
    let candidates = if name == ".ctor" {
        let id = types.ancestor_id(owner).expect("async runtime type");
        types
            .references
            .members(id, name)
            .iter()
            .map(|&id| Member::declared(id, Some(owner)))
            .collect()
    } else {
        types.instance_candidates(owner, name)
    };
    for candidate in candidates {
        let signature = types.method_signature(candidate, span)?;
        if signature.type_arity == 0 && signature.parameters == parameters {
            return Ok((candidate, signature.result));
        }
    }
    Err(error(
        span,
        format!(
            "CLR async runtime is missing '{name}' on {}",
            types.describe(owner)
        ),
    ))
}

fn context_method(
    types: &Structs<'_, '_>,
    name: &str,
    parameters: &[Type],
    span: Span,
) -> Checked<Member> {
    for &id in types.references.candidates(name) {
        if !types
            .references
            .is_framework_type(types.references.method(id).owner)
        {
            continue;
        }
        let candidate = Member::declared(id, None);
        let signature = types.method_signature(candidate, span)?;
        if signature.type_arity == 0 && signature.parameters == parameters {
            return Ok(candidate);
        }
    }
    Err(error(
        span,
        format!("CLR async runtime is missing '{name}'"),
    ))
}

impl Plan {
    pub(super) fn resolve(types: &Structs<'_, '_>, output: Type, span: Span) -> Checked<Self> {
        let completion = framework_generic(
            types,
            "System.Threading.Tasks.TaskCompletionSource",
            output,
            span,
        )?;
        let result_parameter = if output == Type::Void {
            &[][..]
        } else {
            std::slice::from_ref(&output)
        };
        let exception = types
            .references
            .exception
            .ok_or_else(|| error(span, "async functions require System.Exception"))?;
        let cancellation = Type::External(framework_definition(
            types,
            "System.OperationCanceledException",
            span,
        )?);
        let (cancellation_token, token_type) =
            member(types, cancellation, "get_CancellationToken", &[], span)?;
        let action_definition = framework_definition(types, "System.Action", span)?;
        let context = Type::External(framework_definition(
            types,
            "System.Threading.ExecutionContext",
            span,
        )?);
        let context_callback_definition =
            framework_definition(types, "System.Threading.ContextCallback", span)?;
        let context_callback = Type::External(context_callback_definition);
        Ok(Self {
            frame: types.array(Type::NullableObject, span)?,
            output,
            completion,
            constructor: member(types, completion, ".ctor", &[], span)?.0,
            get_task: member(types, completion, "get_Task", &[], span)?.0,
            set_result: member(types, completion, "SetResult", result_parameter, span)?.0,
            set_exception: member(
                types,
                completion,
                "SetException",
                &[Type::External(exception)],
                span,
            )?
            .0,
            cancellation,
            cancellation_token,
            set_canceled: member(types, completion, "SetCanceled", &[token_type], span)?.0,
            action: Type::External(action_definition),
            action_definition,
            context,
            capture_context: context_method(
                types,
                "System.Threading.ExecutionContext.Capture",
                &[],
                span,
            )?,
            run_context: context_method(
                types,
                "System.Threading.ExecutionContext.Run",
                &[context, context_callback, Type::NullableObject],
                span,
            )?,
            context_callback,
            context_callback_definition,
        })
    }
}

impl<'src> Checker<'_, 'src> {
    pub(super) fn await_expression(
        &mut self,
        operand: &syntax::Expr<'src>,
        span: Span,
    ) -> Checked<(ExprKind, Type)> {
        if !self.async_fn {
            return Err(error(span, "'.await' requires an async function"));
        }
        let receiver = self.expression(operand, None)?;
        if receiver.ty == Type::Never {
            return Ok((receiver.kind, Type::Never));
        }
        if receiver.ty.is_nullable()
            || !self
                .types
                .ancestor_id(receiver.ty)
                .is_some_and(|id| self.references.is_entry_task(id))
        {
            return Err(error(
                operand.span,
                "'.await' requires a non-null CLR Task or ValueTask",
            ));
        }
        let (get_awaiter, awaiter) = member(self.types, receiver.ty, "GetAwaiter", &[], span)?;
        if !awaiter.clr_struct() || self.types.stack_only(awaiter) {
            return Err(error(span, "'.await' requires a CLR value-type awaiter"));
        }
        let (completed, completed_type) =
            member(self.types, awaiter, "get_IsCompleted", &[], span)?;
        let action = Type::External(framework_definition(self.types, "System.Action", span)?);
        let (subscribe, subscribe_type) =
            member(self.types, awaiter, "OnCompleted", &[action], span)?;
        if completed_type != Type::Bool || subscribe_type != Type::Void {
            return Err(error(span, "incompatible CLR awaiter signature"));
        }
        let (result, ty) = member(self.types, awaiter, "GetResult", &[], span)?;
        let (kind, ty_awaiter) = if receiver.ty.clr_struct() {
            self.constrained_call(
                get_awaiter,
                Box::new([]),
                receiver,
                Vec::new(),
                awaiter,
                span,
            )?
        } else {
            (
                ExprKind::Call(
                    Target::External {
                        method: get_awaiter.id,
                        arguments: Box::new([]),
                        owner: get_awaiter.owner,
                    },
                    vec![receiver],
                ),
                awaiter,
            )
        };
        Ok((
            ExprKind::Await(Box::new(Suspension {
                awaiter: Expr {
                    kind,
                    ty: ty_awaiter,
                    span,
                },
                completed,
                subscribe,
                result,
            })),
            ty,
        ))
    }
}
