//! Builds the test entry point from the same typed blocks as source methods.

use super::{
    Binary, Block, Catch, Checked, Checker, ExceptionRegion, Expr, ExprKind, Function, MethodBody,
    Signature, Statement, Target, Type, Value,
};
use crate::{Diagnostic, Span, syntax};

pub(super) fn runner<'src>(functions: &[&syntax::Function<'src>], filter: &str) -> Function<'src> {
    let span = Span::default();
    let expr = |kind, ty| Expr { kind, ty, span };
    let integer = |number| expr(ExprKind::Integer(number), Type::I32);
    let string = |text: String| expr(ExprKind::String(text.into()), Type::String);
    let load = |index, ty| expr(ExprKind::Load(Value::Local(index)), ty);
    let print = |value: Expr| {
        Statement::Discard(expr(
            ExprKind::Call(Target::WriteLine(Some(value.ty)), vec![value]),
            Type::Void,
        ))
    };
    let failures = 0;
    let exception = 1;
    let save_failures = |value| Statement::Store {
        target: Value::Local(failures),
        value,
        span,
    };
    let mut statements = vec![save_failures(integer(0))];
    let mut count = 0;
    for (index, test) in functions
        .iter()
        .enumerate()
        .filter(|(_, test)| test.test && test.name.contains(filter))
    {
        count += 1;
        let call = expr(
            ExprKind::Call(Target::Function(index, Box::new([])), Vec::new()),
            Type::Void,
        );
        let increment = expr(
            ExprKind::Binary(
                Binary::Add,
                Box::new(load(failures, Type::I32)),
                Box::new(integer(1)),
            ),
            Type::I32,
        );
        statements.push(Statement::Discard(expr(
            ExprKind::TryCatch(Box::new(ExceptionRegion {
                body: void_block(vec![
                    Statement::Discard(call),
                    print(string(format!("ok {}", test.name))),
                ]),
                catches: vec![Catch {
                    ty: None,
                    local: Some(exception),
                    body: void_block(vec![
                        save_failures(increment),
                        print(string(format!("FAIL {}", test.name))),
                        print(load(exception, Type::Object)),
                    ]),
                }],
                result: None,
            })),
            Type::Void,
        )));
    }
    let tests = if count == 1 { "test" } else { "tests" };
    statements.push(print(expr(
        ExprKind::Interpolation(vec![
            string(format!("{count} {tests}, ")),
            expr(
                ExprKind::BoxValue(Box::new(load(failures, Type::I32))),
                Type::Object,
            ),
            string(" failed".into()),
        ]),
        Type::String,
    )));
    statements.push(Statement::Discard(expr(
        ExprKind::If {
            condition: Box::new(expr(
                ExprKind::Binary(
                    Binary::Ne,
                    Box::new(load(failures, Type::I32)),
                    Box::new(integer(0)),
                ),
                Type::Bool,
            )),
            yes: Block {
                statements: vec![Statement::Return(Some(integer(1)))],
                tail: None,
                ty: Type::Never,
            },
            no: None,
            result_local: None,
        },
        Type::Void,
    )));
    Function {
        has_exception_regions: count != 0,
        internal: true,
        name: "<oxel_test_main>",
        span,
        signature: Signature {
            type_arity: 0,
            bounds: Vec::new(),
            parameters: Vec::new(),
            result: Type::I32,
        },
        type_parameters: Vec::new(),
        locals: vec![Type::I32, Type::Object],
        captured_cells: Default::default(),
        body: MethodBody::Managed(Block {
            statements,
            tail: Some(Box::new(integer(0))),
            ty: Type::I32,
        }),
        lambdas: Vec::new(),
    }
}

fn void_block(statements: Vec<Statement>) -> Block {
    Block {
        statements,
        tail: None,
        ty: Type::Void,
    }
}

impl<'src> Checker<'_, 'src> {
    pub(super) fn assertion(
        &mut self,
        span: Span,
        type_arguments: &[syntax::Path<'src>],
        args: &[syntax::Expr<'src>],
    ) -> Checked<(ExprKind, Type)> {
        if self.comptime {
            return Err(Diagnostic::new(
                "E0501",
                span,
                "assert is available at runtime",
            ));
        }
        let [condition, message @ ..] = args else {
            return Err(assert_signature(span));
        };
        if !type_arguments.is_empty() || message.len() > 1 {
            return Err(assert_signature(span));
        }
        let condition = self.expression(condition, Some(Type::Bool))?;
        let message = if let Some(message) = message.first() {
            self.expression(message, Some(Type::String))?
        } else {
            Expr {
                kind: ExprKind::String("assertion failed".into()),
                ty: Type::String,
                span,
            }
        };
        if condition.ty == Type::Never {
            return Ok((condition.kind, Type::Never));
        }
        let exception = Expr {
            kind: ExprKind::Call(Target::AssertionException, vec![message]),
            ty: Type::Object,
            span,
        };
        Ok((
            ExprKind::If {
                condition: Box::new(condition),
                yes: void_block(Vec::new()),
                no: Some(Box::new(Expr {
                    kind: ExprKind::Throw(Box::new(exception)),
                    ty: Type::Never,
                    span,
                })),
                result_local: None,
            },
            Type::Void,
        ))
    }
}

fn assert_signature(span: Span) -> Diagnostic {
    Diagnostic::new(
        "E0205",
        span,
        "assert expects a boolean and an optional string message",
    )
}
