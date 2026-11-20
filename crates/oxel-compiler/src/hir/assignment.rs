//! Checks delayed initialization over the same evaluation order used by CIL.

use std::collections::{HashMap, HashSet};

use super::{
    Block, Checked, Destination, Expr, ExprKind, Match, Statement, Value, suggest_mutable,
};
use crate::{
    ArgumentMode, Diagnostic, Span,
    syntax::{LoopControl, Unary},
};

pub(super) struct Local<'src> {
    pub index: u16,
    pub name: &'src str,
    pub mutable: bool,
    pub mutability_edit: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    OutsideScope,
    Unassigned,
    Assigned,
    Maybe,
}

// None represents an unreachable path. Only declarations without an initializer
// need slots in the analysis state.
type State = Option<Vec<Status>>;

fn merge(left: State, right: State) -> State {
    match (left, right) {
        (Some(mut left), Some(right)) => {
            for (left, right) in left.iter_mut().zip(right) {
                if *left != right {
                    *left = Status::Maybe;
                }
            }
            Some(left)
        }
        (left, right) => left.or(right),
    }
}

fn keep_scope(entry: &[Status], mut state: State) -> State {
    if let Some(assigned) = &mut state {
        for (before, after) in entry.iter().zip(assigned) {
            if *before == Status::OutsideScope {
                *after = Status::OutsideScope;
            }
        }
    }
    state
}

#[derive(Default)]
struct LoopPaths {
    breaks: State,
    continues: State,
}

pub(super) fn validate(block: &Block, locals: &[Local<'_>]) -> Checked<()> {
    let mut analysis = Analysis {
        slots: locals
            .iter()
            .enumerate()
            .map(|(slot, local)| (local.index, slot))
            .collect(),
        locals,
        loops: Vec::new(),
        try_failures: Vec::new(),
        catch_writes: Vec::new(),
    };
    analysis.block(block, Some(vec![Status::OutsideScope; locals.len()]))?;
    Ok(())
}

struct Analysis<'a, 'src> {
    slots: HashMap<u16, usize>,
    locals: &'a [Local<'src>],
    loops: Vec<LoopPaths>,
    try_failures: Vec<State>,
    catch_writes: Vec<HashSet<usize>>,
}

impl Analysis<'_, '_> {
    fn statements(&mut self, block: &Block, mut state: State) -> Checked<State> {
        for statement in &block.statements {
            let Some(mut assigned) = state else {
                return Ok(None);
            };
            state = match statement {
                Statement::DeclareLocal { index, value } => {
                    if let Some(value) = value {
                        self.expression(value, Some(assigned))?
                    } else {
                        assigned[self.slots[index]] = Status::Unassigned;
                        Some(assigned)
                    }
                }
                Statement::Store {
                    target,
                    value,
                    span,
                } => {
                    let mut next = self.expression(value, Some(assigned))?;
                    if let Value::Local(index) = target {
                        self.store(*index, *span, &mut next)?;
                    }
                    next
                }
                Statement::Return(value) => {
                    if let Some(value) = value {
                        self.expression(value, Some(assigned))?;
                    }
                    None
                }
                Statement::TryFailure(error) => {
                    let state = if let Some(error) = error {
                        self.expression(error, Some(assigned))?
                    } else {
                        Some(assigned)
                    };
                    let failures = self.try_failures.last_mut().expect("checked try block");
                    *failures = merge(failures.take(), state);
                    None
                }
                Statement::Discard(expr) => self.expression(expr, Some(assigned))?,
                Statement::While { condition, body } => {
                    self.loop_paths(body, assigned, |analysis, state| {
                        analysis.condition(condition, state)
                    })?
                }
                Statement::For(range) => {
                    let entry = self.expression(&range.start, Some(assigned))?;
                    let entry = self.expression(&range.end, entry)?;
                    if let Some(entry) = entry {
                        self.loop_paths(&range.body, entry, |_, state| Ok((state.clone(), state)))?
                    } else {
                        None
                    }
                }
                Statement::LoopControl(control) => {
                    let frame = self.loops.last_mut().expect("checked loop control");
                    let paths = match control {
                        LoopControl::Break => &mut frame.breaks,
                        LoopControl::Continue => &mut frame.continues,
                    };
                    *paths = merge(paths.take(), Some(assigned));
                    None
                }
            };
        }
        Ok(state)
    }

    fn store(&mut self, index: u16, span: Span, state: &mut State) -> Checked<()> {
        if let Some(assigned) = state
            && let Some(&slot) = self.slots.get(&index)
        {
            let local = &self.locals[slot];
            if !local.mutable && assigned[slot] != Status::Unassigned {
                let error = Diagnostic::new(
                    "E0322",
                    span,
                    format!("immutable binding '{}' may already be assigned", local.name),
                );
                return Err(match local.mutability_edit {
                    Some(offset) => suggest_mutable(error, offset),
                    None => error,
                });
            }
            for writes in &mut self.catch_writes {
                writes.insert(slot);
            }
            assigned[slot] = Status::Assigned;
        }
        Ok(())
    }

    fn leave_block(&self, block: &Block, mut state: State) -> State {
        if let Some(assigned) = &mut state {
            for statement in &block.statements {
                if let Statement::DeclareLocal { index, value: None } = statement {
                    assigned[self.slots[index]] = Status::OutsideScope;
                }
            }
        }
        state
    }

    fn block(&mut self, block: &Block, state: State) -> Checked<State> {
        let state = self.statements(block, state)?;
        let state = if let Some(tail) = &block.tail {
            self.expression(tail, state)?
        } else {
            state
        };
        Ok(self.leave_block(block, state))
    }

    fn loop_paths(
        &mut self,
        body: &Block,
        entry: Vec<Status>,
        condition: impl Fn(&mut Self, State) -> Checked<(State, State)>,
    ) -> Checked<State> {
        let mut header = entry.clone();
        loop {
            // A transfer in the condition still targets the enclosing loop body.
            let (on_true, on_false) = condition(self, Some(header.clone()))?;
            self.loops.push(LoopPaths::default());
            let body_end = self.block(body, on_true);
            let paths = self.loops.pop().expect("loop body frame");
            let back_edge = keep_scope(&entry, merge(body_end?, paths.continues));
            let next = merge(Some(entry.clone()), back_edge).expect("reachable loop entry");
            if next == header {
                return Ok(keep_scope(&entry, merge(on_false, paths.breaks)));
            }
            header = next;
        }
    }

    // Preserve the result-dependent paths of boolean blocks and conditionals.
    // The latter include short-circuit operators after lowering.
    fn condition(&mut self, expr: &Expr, state: State) -> Checked<(State, State)> {
        if state.is_none() {
            return Ok((None, None));
        }
        match &expr.kind {
            ExprKind::Boolean(true) => Ok((state, None)),
            ExprKind::Boolean(false) => Ok((None, state)),
            ExprKind::Unary(Unary::Not, value) => {
                let (yes, no) = self.condition(value, state)?;
                Ok((no, yes))
            }
            ExprKind::Block(block) => self.block_condition(block, state),
            ExprKind::Match(matched) => {
                let (mut yes, mut no) = (None, None);
                for (arm, entry) in matched.arms.iter().zip(self.match_entries(matched, state)?) {
                    let (arm_yes, arm_no) = self.block_condition(arm, entry)?;
                    yes = merge(yes, arm_yes);
                    no = merge(no, arm_no);
                }
                Ok((yes, no))
            }
            ExprKind::If {
                condition,
                yes,
                no: Some(no),
                ..
            } => {
                let (on_true, on_false) = self.condition(condition, state)?;
                let (true_yes, true_no) = self.block_condition(yes, on_true)?;
                let (false_yes, false_no) = self.condition(no, on_false)?;
                Ok((merge(true_yes, false_yes), merge(true_no, false_no)))
            }
            _ => {
                let state = self.expression(expr, state)?;
                Ok((state.clone(), state))
            }
        }
    }

    fn block_condition(&mut self, block: &Block, state: State) -> Checked<(State, State)> {
        let state = self.statements(block, state)?;
        let (yes, no) = if let Some(tail) = &block.tail {
            self.condition(tail, state)?
        } else {
            // A reachable boolean block always has a result.
            (state.clone(), state)
        };
        Ok((self.leave_block(block, yes), self.leave_block(block, no)))
    }

    fn expression(&mut self, expr: &Expr, state: State) -> Checked<State> {
        let Some(assigned) = state else {
            return Ok(None);
        };
        let state = Some(assigned);
        match &expr.kind {
            ExprKind::Load(Value::Local(index))
            | ExprKind::CaptureCell(Value::Local(index))
            | ExprKind::LocalAddress(Value::Local(index), ArgumentMode::Ref | ArgumentMode::In) => {
                if let Some(&slot) = self.slots.get(index)
                    && state.as_ref().expect("reachable expression")[slot] != Status::Assigned
                {
                    return Err(Diagnostic::new(
                        "E0321",
                        expr.span,
                        format!("binding '{}' may be unassigned", self.locals[slot].name),
                    ));
                }
                Ok(state)
            }
            ExprKind::If {
                condition, yes, no, ..
            } => {
                let (on_true, on_false) = self.condition(condition, state)?;
                let yes = self.block(yes, on_true)?;
                let no = if let Some(no) = no {
                    self.expression(no, on_false)?
                } else {
                    on_false
                };
                Ok(merge(yes, no))
            }
            ExprKind::Match(matched) => {
                let mut end = None;
                for (arm, entry) in matched.arms.iter().zip(self.match_entries(matched, state)?) {
                    end = merge(end, self.block(arm, entry)?);
                }
                Ok(end)
            }
            ExprKind::Coalesce(left, right, _) => {
                let state = self.expression(left, state)?;
                match &left.kind {
                    ExprKind::String(_) => Ok(state),
                    ExprKind::Null => self.expression(right, state),
                    _ => {
                        let fallback = self.expression(right, state.clone())?;
                        Ok(merge(state, fallback))
                    }
                }
            }
            ExprKind::Block(block) => self.block(block, state),
            ExprKind::Lambda { environment, .. } => self.expression(environment, state),
            ExprKind::Try(attempt) => {
                self.try_failures.push(None);
                let success = self.block(&attempt.body, state.clone())?;
                let failures = self.try_failures.pop().expect("checked try block");
                let failure = self.expression(
                    &attempt.failure,
                    keep_scope(state.as_deref().expect("reachable try block"), failures),
                )?;
                Ok(merge(success, failure))
            }
            ExprKind::Using(region) => {
                let state = self.expression(&region.resource, state)?;
                let state = self.block(&region.body, state)?;
                self.expression(&region.dispose, state)
            }
            ExprKind::TryCatch(region) => {
                let Some(mut entry) = state else {
                    return Ok(None);
                };
                self.catch_writes.push(HashSet::new());
                let success = self.block(&region.body, Some(entry.clone()));
                let writes = self.catch_writes.pop().expect("active protected region");
                let mut end = success?;
                for slot in writes {
                    if entry[slot] == Status::Unassigned {
                        entry[slot] = Status::Maybe;
                    }
                }
                for arm in &region.catches {
                    end = merge(end, self.block(&arm.body, Some(entry.clone()))?);
                }
                Ok(end)
            }
            ExprKind::Throw(value) => {
                self.expression(value, state)?;
                Ok(None)
            }
            ExprKind::Binary(_, left, right) | ExprKind::StringIndex(left, right) => {
                let state = self.expression(left, state)?;
                self.expression(right, state)
            }
            ExprKind::Construct { fields, .. } => {
                self.expressions(fields.iter().map(|(_, value)| value), state)
            }
            ExprKind::Call(_, args) => self.call_arguments(args, state),
            ExprKind::Interpolation(args) | ExprKind::ArrayLiteral { items: args, .. } => {
                self.expressions(args, state)
            }
            ExprKind::ArrayAccess(access) => {
                let state = self.expression(&access.array, state)?;
                let state = self.expression(&access.index, state)?;
                if let Some(value) = &access.value {
                    self.expression(value, state)
                } else {
                    Ok(state)
                }
            }
            ExprKind::NewArray { length, .. }
            | ExprKind::ArrayLength(length)
            | ExprKind::StringLength(length) => self.expression(length, state),
            ExprKind::IndexUpdate(update) => {
                let state = self.expression(&update.receiver, state)?;
                let state =
                    self.expressions(update.indices.iter().map(|(_, value)| value), state)?;
                self.expression(&update.value, state)
            }
            ExprKind::Await(suspension) => self.expression(&suspension.awaiter, state),
            ExprKind::ConstrainedCall { receiver, args, .. } => {
                let state = self.expression(receiver, state)?;
                self.call_arguments(args, state)
            }
            ExprKind::Field(value, _)
            | ExprKind::BoxValue(value)
            | ExprKind::NumericCast(value, _)
            | ExprKind::CheckedCast(value, _)
            | ExprKind::TypeTest(value, _)
            | ExprKind::Unary(_, value) => self.expression(value, state),
            ExprKind::Load(Value::Parameter(_) | Value::Captured(..) | Value::CapturedCell(..))
            | ExprKind::CaptureCell(_)
            | ExprKind::LocalAddress(..)
            | ExprKind::Null
            | ExprKind::DefaultValue(_)
            | ExprKind::Integer(_)
            | ExprKind::Float32(_)
            | ExprKind::Float64(_)
            | ExprKind::Boolean(_)
            | ExprKind::String(_)
            | ExprKind::TypeQuery { .. } => Ok(state),
        }
    }

    fn match_entries(&mut self, matched: &Match, state: State) -> Checked<Vec<State>> {
        let entry = self.expression(&matched.value, state)?;
        if matched.guards.is_empty() {
            return Ok(vec![entry; matched.arms.len()]);
        }
        let mut arms = vec![None; matched.arms.len()];
        let mut decisions = vec![None; matched.decisions.len()];
        decisions[0] = entry;
        let mut guards = matched.guards.iter().peekable();
        for (index, decision) in matched.decisions.iter().enumerate() {
            // All entries to a guard precede the decision group it can fall back to.
            if let Some(guard) = guards.next_if(|guard| guard.fallback == index) {
                let (yes, no) = self.block_condition(&guard.condition, arms[guard.arm].take())?;
                arms[guard.arm] = yes;
                decisions[index] = merge(decisions[index].take(), no);
            }
            let state = decisions[index].take();
            if state.is_none() {
                continue;
            }
            for target in decision.destinations() {
                let entry = match *target {
                    Destination::Arm(arm) => &mut arms[arm],
                    // Pattern setup only initializes new bindings from the saved scrutinee.
                    Destination::Setup(id) => &mut arms[matched.setups[id].arm],
                    Destination::Decision(next) => &mut decisions[next],
                    Destination::Invalid => continue,
                };
                *entry = merge(entry.take(), state.clone());
            }
        }
        Ok(arms)
    }

    fn call_arguments(&mut self, args: &[Expr], state: State) -> Checked<State> {
        let mut state = self.expressions(args, state)?;
        for arg in args {
            if let ExprKind::LocalAddress(Value::Local(index), ArgumentMode::Out) = arg.kind {
                self.store(index, arg.span, &mut state)?;
            }
        }
        Ok(state)
    }

    fn expressions<'a>(
        &mut self,
        expressions: impl IntoIterator<Item = &'a Expr>,
        mut state: State,
    ) -> Checked<State> {
        for expression in expressions {
            state = self.expression(expression, state)?;
        }
        Ok(state)
    }
}
