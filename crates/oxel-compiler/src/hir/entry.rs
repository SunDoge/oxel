use super::{
    Block, Checked, Decision, Destination, Expr, ExprKind, Function, Match, Member, MethodBody,
    Signature, Statement, Structs, Target, Type, Value, unions::CarrierLayout,
};
use crate::{Diagnostic, Span, clr::Invocation};

pub(super) struct Entry {
    main: usize,
    span: Span,
    task: Option<Task>,
    result: Option<ResultExit>,
}

pub(super) struct Task {
    as_task: Option<AwaitCall>,
    get_awaiter: AwaitCall,
    get_result: AwaitCall,
}

struct AwaitCall {
    member: Member,
    result: Type,
}

struct ResultExit {
    layout: CarrierLayout,
    success: Type,
    error: Type,
    error_field: usize,
}

impl Entry {
    pub(super) fn check(
        main: usize,
        signature: &Signature,
        types: &Structs<'_, '_>,
        span: Span,
    ) -> Checked<Self> {
        let ty = signature.result;
        let task = Task::resolve(types, ty, span)?;
        let output = task.as_ref().map_or(ty, |task| task.get_result.result);
        let result = CarrierLayout::for_type(types, output)
            .and_then(|layout| layout.error_field.map(|error_field| (layout, error_field)))
            .map(|(layout, error_field)| {
                Ok(ResultExit {
                    layout,
                    success: types.field_type(output, Type::TypeParameter(0), span)?,
                    error: types.field_type(output, Type::TypeParameter(1), span)?,
                    error_field,
                })
            })
            .transpose()?;
        if result.is_none() && !matches!(output, Type::Void | Type::I32) {
            return Err(Diagnostic::new(
                "E0206",
                span,
                "main must return void, i32, Result<T, E>, or a CLR Task or ValueTask with one of those results",
            ));
        }
        Ok(Self {
            main,
            span,
            task,
            result,
        })
    }

    pub(super) fn install(&self, functions: &mut Vec<Function<'_>>) -> usize {
        if self.task.is_none() && self.result.is_none() {
            return self.main;
        }
        let span = self.span;
        let mut signature = functions[self.main].signature.clone();
        let args = signature
            .parameters
            .iter()
            .enumerate()
            .map(|(index, &ty)| expr(ExprKind::Load(Value::Parameter(index as u16)), ty, span))
            .collect();
        let mut value = expr(
            ExprKind::Call(Target::Function(self.main, Box::new([])), args),
            signature.result,
            span,
        );
        let mut locals = Vec::new();
        if let Some(task) = &self.task {
            value = task.await_result(value, &mut locals);
        }
        if let Some(result) = &self.result {
            let local = locals.len() as u16;
            locals.push(value.ty);
            value = result.status(value, local, span);
        }
        signature.result = value.ty;
        let entry = functions.len();
        functions.push(Function {
            namespace: None,
            has_exception_regions: false,
            internal: true,
            name: "<oxel_main>",
            span,
            signature,
            type_parameters: Vec::new(),
            locals,
            captured_cells: Default::default(),
            body: MethodBody::Managed(Block {
                statements: Vec::new(),
                ty: value.ty,
                tail: Some(Box::new(value)),
            }),
            lambdas: Vec::new(),
        });
        entry
    }
}

impl Task {
    pub(super) fn resolve(types: &Structs<'_, '_>, ty: Type, span: Span) -> Checked<Option<Self>> {
        if ty.is_nullable()
            || !types
                .ancestor_id(ty)
                .is_some_and(|id| types.references.is_entry_task(id))
        {
            return Ok(None);
        }
        let method = |owner, name| AwaitCall::resolve(types, owner, name, span);
        // A ValueTask backed by an IValueTaskSource need not block in GetResult.
        // Convert it once so the native entry point can safely wait on a Task.
        let as_task = ty.clr_struct().then(|| method(ty, "AsTask")).transpose()?;
        let task = as_task.as_ref().map_or(ty, |call| call.result);
        let get_awaiter = method(task, "GetAwaiter")?;
        if !get_awaiter.result.clr_struct() {
            return Err(Diagnostic::new(
                "E0206",
                span,
                "CLR Task entry point requires a value-type awaiter",
            ));
        }
        let get_result = method(get_awaiter.result, "GetResult")?;
        Ok(Some(Self {
            as_task,
            get_awaiter,
            get_result,
        }))
    }

    pub(super) fn await_result(&self, mut value: Expr, locals: &mut Vec<Type>) -> Expr {
        if let Some(as_task) = &self.as_task {
            value = as_task.lower(value, locals);
        }
        let awaiter = self.get_awaiter.lower(value, locals);
        self.get_result.lower(awaiter, locals)
    }
}

impl AwaitCall {
    fn resolve(types: &Structs<'_, '_>, owner: Type, name: &str, span: Span) -> Checked<Self> {
        let member = types
            .instance_candidates(owner, name)
            .into_iter()
            .find(|member| {
                let method = types.references.method(member.id);
                matches!(method.invocation, Invocation::Instance)
                    && method.signature.type_arity == 0
                    && method.signature.parameters.is_empty()
            })
            .ok_or_else(|| {
                Diagnostic::new(
                    "E0206",
                    span,
                    format!("CLR task entry point requires {name}"),
                )
            })?;
        let result = types.method_signature(member, span)?.result;
        Ok(Self { member, result })
    }

    fn lower(&self, receiver: Expr, locals: &mut Vec<Type>) -> Expr {
        let span = receiver.span;
        let kind = if receiver.ty.clr_struct() {
            let temporary = locals.len() as u16;
            locals.push(receiver.ty);
            ExprKind::ConstrainedCall {
                method: self.member,
                type_arguments: Box::new([]),
                receiver: Box::new(receiver),
                temporary: Some(temporary),
                args: Vec::new(),
            }
        } else {
            ExprKind::Call(
                Target::External {
                    method: self.member.id,
                    owner: self.member.owner,
                    arguments: Box::new([]),
                },
                vec![receiver],
            )
        };
        expr(kind, self.result, span)
    }
}

impl ResultExit {
    fn status(&self, result: Expr, local: u16, span: Span) -> Expr {
        let load = || expr(ExprKind::Load(Value::Local(local)), result.ty, span);
        let field = |index, ty| expr(ExprKind::Field(Box::new(load()), index), ty, span);
        let number = |value| expr(ExprKind::Integer(value), Type::I32, span);
        let ok = if self.success == Type::I32 {
            field(self.layout.success_field, self.success)
        } else {
            number(0)
        };
        let mut error = field(self.error_field, self.error);
        if !error.ty.reference() {
            error = expr(
                ExprKind::BoxValue(Box::new(error)),
                Type::NullableObject,
                span,
            );
        }
        let writer = expr(
            ExprKind::Call(Target::ErrorWriter, Vec::new()),
            Type::Object,
            span,
        );
        let write = expr(
            ExprKind::Call(Target::WriteErrorLine, vec![writer, error]),
            Type::Void,
            span,
        );
        let mut targets = vec![Destination::Arm(0); 2];
        targets[self.layout.failure_case] = Destination::Arm(1);
        let decision = Decision {
            value: load(),
            discriminant: self.layout.discriminant,
            literals: None,
            targets,
        };
        expr(
            ExprKind::Match(Box::new(Match {
                local,
                value: result,
                decisions: vec![decision],
                setups: Vec::new(),
                guards: Vec::new(),
                arms: vec![
                    Block {
                        statements: Vec::new(),
                        tail: Some(Box::new(ok)),
                        ty: Type::I32,
                    },
                    Block {
                        statements: vec![Statement::Discard(write)],
                        tail: Some(Box::new(number(1))),
                        ty: Type::I32,
                    },
                ],
                result_local: None,
            })),
            Type::I32,
            span,
        )
    }
}

fn expr(kind: ExprKind, ty: Type, span: Span) -> Expr {
    Expr { kind, ty, span }
}
