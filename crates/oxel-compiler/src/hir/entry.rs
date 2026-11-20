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

struct Task {
    get_awaiter: Member,
    awaiter: Type,
    get_result: Member,
    output: Type,
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
        let output = task.as_ref().map_or(ty, |task| task.output);
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
                "main must return void, i32, Result<T, E>, Task, Task<i32>, or Task<Result<T, E>>",
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
            locals.push(task.awaiter);
            value = task.await_result(value, span);
        }
        if let Some(result) = &self.result {
            let local = locals.len() as u16;
            locals.push(value.ty);
            value = result.status(value, local, span);
        }
        signature.result = value.ty;
        let entry = functions.len();
        functions.push(Function {
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
    fn resolve(types: &Structs<'_, '_>, ty: Type, span: Span) -> Checked<Option<Self>> {
        if ty.is_nullable()
            || !types
                .ancestor_id(ty)
                .is_some_and(|id| types.references.is_task(id))
        {
            return Ok(None);
        }
        let method = |owner, name| {
            types
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
                        format!("CLR Task entry point requires {name}"),
                    )
                })
        };
        let get_awaiter = method(ty, "GetAwaiter")?;
        let awaiter = types.method_signature(get_awaiter, span)?.result;
        if !awaiter.clr_struct() {
            return Err(Diagnostic::new(
                "E0206",
                span,
                "CLR Task entry point requires a value-type awaiter",
            ));
        }
        let get_result = method(awaiter, "GetResult")?;
        let output = types.method_signature(get_result, span)?.result;
        Ok(Some(Self {
            get_awaiter,
            awaiter,
            get_result,
            output,
        }))
    }

    fn await_result(&self, value: Expr, span: Span) -> Expr {
        let awaiter = expr(
            ExprKind::Call(
                Target::External {
                    method: self.get_awaiter.id,
                    owner: self.get_awaiter.owner,
                    arguments: Box::new([]),
                },
                vec![value],
            ),
            self.awaiter,
            span,
        );
        expr(
            ExprKind::ConstrainedCall {
                method: self.get_result,
                type_arguments: Box::new([]),
                receiver: Box::new(awaiter),
                temporary: Some(0),
                args: Vec::new(),
            },
            self.output,
            span,
        )
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
