use std::collections::{HashMap, HashSet};

use crate::{
    Diagnostic, Span,
    syntax::{Block, Expr, Expression, Pattern, PatternKind, Statement},
};

use super::super::{Checked, ModuleScope};

pub(super) struct Dependencies<'a, 'src> {
    module: &'a ModuleScope<'a, 'src, 'a>,
    constants: &'a HashMap<&'src str, usize>,
    first_function: usize,
}

impl<'a, 'src> Dependencies<'a, 'src> {
    pub(super) fn new(
        module: &'a ModuleScope<'a, 'src, 'a>,
        constants: &'a HashMap<&'src str, usize>,
        first_function: usize,
    ) -> Self {
        Self {
            module,
            constants,
            first_function,
        }
    }

    pub(super) fn initializer(&self, expr: &Expr<'src>) -> Checked<Vec<usize>> {
        let mut walk = Walk {
            context: self,
            scopes: Vec::new(),
            edges: Vec::new(),
            in_function: false,
        };
        walk.expression(expr)?;
        Ok(walk.edges)
    }

    pub(super) fn function(&self, index: usize) -> Checked<Vec<usize>> {
        let function = self.module.functions[index];
        let parameters = function
            .parameters
            .iter()
            .map(|parameter| parameter.name)
            .collect();
        let mut walk = Walk {
            context: self,
            scopes: vec![parameters],
            edges: Vec::new(),
            in_function: true,
        };
        walk.block(function.body.managed().expect("comptime body"))?;
        Ok(walk.edges)
    }
}

struct Walk<'a, 'src> {
    context: &'a Dependencies<'a, 'src>,
    scopes: Vec<HashSet<&'src str>>,
    edges: Vec<usize>,
    in_function: bool,
}

impl<'src> Walk<'_, 'src> {
    fn expression(&mut self, expr: &Expr<'src>) -> Checked<()> {
        match &expr.kind {
            Expression::Int { .. }
            | Expression::Float { .. }
            | Expression::Char(_)
            | Expression::Bool(_)
            | Expression::Null
            | Expression::String(_) => {}
            Expression::Interpolated(parts) | Expression::Array(parts) => {
                for part in parts {
                    self.expression(part)?;
                }
            }
            Expression::NewArray(_, length) => self.expression(length)?,
            Expression::Name(path) => {
                let (name, span) = path.segments[0];
                if !self.scopes.iter().rev().any(|scope| scope.contains(name)) {
                    if let Some(&id) = self.context.constants.get(name) {
                        self.edges.push(id);
                    } else if path.segments.len() == 1 {
                        return Err(Diagnostic::new(
                            "E0203",
                            span,
                            format!("unknown constant '{name}'"),
                        ));
                    }
                }
            }
            Expression::Field(receiver, _, _) => self.expression(receiver)?,
            Expression::Index {
                receiver,
                arguments,
                ..
            } => {
                self.expression(receiver)?;
                for argument in arguments {
                    self.expression(argument)?;
                }
            }
            Expression::StaticMember { call: None, .. } => {}
            Expression::Call { path, .. } if super::TypeQuery::from_path(path).is_some() => {}
            Expression::Construct { fields, base, .. } => {
                for (_, _, value) in fields {
                    self.expression(value)?;
                }
                if let Some(base) = base {
                    self.expression(base)?;
                }
            }
            Expression::Call {
                path, arguments, ..
            } if path.segments.len() == 2
                && self
                    .context
                    .module
                    .types
                    .names
                    .get(path.segments[0].0)
                    .is_some_and(|&id| {
                        self.context.module.types.definitions[id].union.is_some()
                    }) =>
            {
                for argument in arguments {
                    self.expression(argument)?;
                }
            }
            Expression::Call {
                path, arguments, ..
            } if path.segments.len() == 1 => {
                let (name, span) = path.segments[0];
                let module = self.context.module;
                let index = module.names.get(name).copied().ok_or_else(|| {
                    Diagnostic::new("E0203", span, format!("unknown function '{name}'"))
                })?;
                if !module.functions[index].comptime {
                    return Err(Diagnostic::new(
                        "E0501",
                        span,
                        "constant evaluation can only call comptime functions",
                    ));
                }
                self.edges
                    .push(self.context.constants.len() + index - self.context.first_function);
                for argument in arguments {
                    self.expression(argument)?;
                }
            }
            Expression::Unary(_, operand)
            | Expression::Cast(operand, _)
            | Expression::Propagate(operand, _) => self.expression(operand)?,
            Expression::Binary(_, left, right) => {
                self.expression(left)?;
                self.expression(right)?;
            }
            Expression::Block(block) | Expression::Try(block) => self.block(block)?,
            Expression::Match(value, arms) => {
                self.expression(value)?;
                for arm in arms {
                    self.pattern_scope(&arm.pattern, |walk| {
                        if let Some(guard) = &arm.guard {
                            walk.expression(guard)?;
                        }
                        walk.expression(&arm.value)
                    })?;
                }
            }
            Expression::IfLet(binding) if binding.no.is_some() || self.in_function => {
                self.expression(&binding.value)?;
                self.pattern_scope(&binding.pattern, |walk| walk.block(&binding.yes))?;
                if let Some(no) = &binding.no {
                    self.expression(no)?;
                }
            }
            Expression::If { condition, yes, no } if no.is_some() || self.in_function => {
                self.expression(condition)?;
                self.block(yes)?;
                if let Some(no) = no {
                    self.expression(no)?;
                }
            }
            _ => return Err(self.unsupported(expr.span)),
        }
        Ok(())
    }

    fn block(&mut self, block: &Block<'src>) -> Checked<()> {
        if !self.in_function {
            let Some(tail) = block
                .tail
                .as_deref()
                .filter(|_| block.statements.is_empty())
            else {
                return Err(Diagnostic::new(
                    "E0501",
                    block.span,
                    "constant block must contain one result expression",
                ));
            };
            return self.expression(tail);
        }
        self.scopes.push(HashSet::new());
        for statement in &block.statements {
            match statement {
                Statement::Let { name, value, .. } => {
                    if let Some(value) = value {
                        self.expression(value)?;
                    }
                    self.scopes.last_mut().expect("block scope").insert(name);
                }
                Statement::LetPattern(binding) => {
                    self.expression(&binding.value)?;
                    if let Some(fallback) = &binding.fallback {
                        self.block(fallback)?;
                    }
                    collect_bindings(
                        &binding.pattern,
                        true,
                        self.scopes.last_mut().expect("block scope"),
                    );
                }
                Statement::WhileLet(binding) => {
                    self.expression(&binding.value)?;
                    self.pattern_scope(&binding.pattern, |walk| walk.block(&binding.body))?;
                }
                Statement::Assign { target, value, .. } => {
                    self.expression(target)?;
                    self.expression(value)?;
                }
                Statement::Return { value, .. } => {
                    if let Some(value) = value {
                        self.expression(value)?;
                    }
                }
                Statement::While { condition, body } => {
                    self.expression(condition)?;
                    self.block(body)?;
                }
                Statement::For(loop_) => {
                    self.expression(&loop_.source)?;
                    if let Some(range) = &loop_.range {
                        self.expression(&range.end)?;
                    }
                    self.scopes.push(HashSet::from([loop_.binding.0]));
                    self.block(&loop_.body)?;
                    self.scopes.pop();
                }
                Statement::LoopControl { .. } => {}
                Statement::Expression(expr) => self.expression(expr)?,
            }
        }
        if let Some(value) = &block.tail {
            self.expression(value)?;
        }
        self.scopes.pop();
        Ok(())
    }

    fn unsupported(&self, span: Span) -> Diagnostic {
        Diagnostic::new(
            "E0501",
            span,
            if self.in_function {
                "unsupported expression in comptime function"
            } else {
                "unsupported expression in constant initializer"
            },
        )
    }

    fn pattern_scope(
        &mut self,
        pattern: &Pattern<'src>,
        evaluate: impl FnOnce(&mut Self) -> Checked<()>,
    ) -> Checked<()> {
        let mut names = HashSet::new();
        collect_bindings(pattern, true, &mut names);
        self.scopes.push(names);
        let result = evaluate(self);
        self.scopes.pop();
        result
    }
}

fn collect_bindings<'src>(pattern: &Pattern<'src>, root: bool, names: &mut HashSet<&'src str>) {
    match &pattern.kind {
        PatternKind::Name(_, Some(children)) => {
            for child in children {
                collect_bindings(child, false, names);
            }
        }
        PatternKind::Struct { fields, .. } => {
            for (_, _, child) in fields {
                collect_bindings(child, false, names);
            }
        }
        PatternKind::Name(path, None) if !root => {
            if let [(name, _)] = path.segments.as_slice()
                && *name != "_"
            {
                names.insert(name);
            }
        }
        PatternKind::Or(alternatives) => {
            for alternative in alternatives {
                collect_bindings(alternative, root, names);
            }
        }
        _ => {}
    }
}

/// Groups mutually recursive nodes and places each group after its dependencies.
/// Iterative traversals keep long chains of declarations off the call stack.
pub(super) fn groups(edges: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let mut reverse = vec![Vec::new(); edges.len()];
    for (node, dependencies) in edges.iter().enumerate() {
        for &dependency in dependencies {
            reverse[dependency].push(node);
        }
    }
    let mut visited = vec![false; edges.len()];
    let mut finished = Vec::with_capacity(edges.len());
    for root in 0..edges.len() {
        if std::mem::replace(&mut visited[root], true) {
            continue;
        }
        let mut stack = vec![(root, edges[root].iter())];
        while let Some((node, remaining)) = stack.last_mut() {
            if let Some(&next) = remaining.next() {
                if !std::mem::replace(&mut visited[next], true) {
                    stack.push((next, edges[next].iter()));
                }
            } else {
                finished.push(*node);
                stack.pop();
            }
        }
    }

    let mut groups = Vec::new();
    visited.fill(false);
    for root in finished.into_iter().rev() {
        if std::mem::replace(&mut visited[root], true) {
            continue;
        }
        let mut group = Vec::new();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            group.push(node);
            for &next in &reverse[node] {
                if !std::mem::replace(&mut visited[next], true) {
                    stack.push(next);
                }
            }
        }
        groups.push(group);
    }
    groups.reverse();
    groups
}
