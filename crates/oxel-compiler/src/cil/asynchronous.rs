//! Emits resumable CLR methods using typed IL locals and a heap frame at suspension.

use super::{BASE_EXCEPTION, Body, ExceptionClause, StackSlot};
use crate::{
    Diagnostic,
    hir::{
        Block, Expr, Member, Type,
        asynchronous::{Plan, Suspension},
    },
};

const STATE: usize = 0;
const COMPLETION: usize = 1;
const CONTINUATION: usize = 2;
const PARAMETERS: usize = 3;

pub(super) struct State {
    plan: Plan,
    first_parameter: u16,
    locals_start: usize,
    scratch_start: usize,
    scratch_size: usize,
    resumes: Vec<usize>,
    failure: Option<Diagnostic>,
}

impl State {
    pub(super) fn parameter(&self, index: u16) -> u16 {
        self.first_parameter.saturating_add(index)
    }
}

impl Body<'_, '_> {
    pub(super) fn emit_async_step(
        &mut self,
        block: &Block,
        plan: Plan,
        parameters: &[Type],
    ) -> Result<usize, Diagnostic> {
        self.max_stack = 4;
        let frame = self.reserve_spill(StackSlot::Value(plan.frame));
        self.op(0x02); // ldarg.0
        let frame_type = self.metadata.runtime_type_token(plan.frame);
        self.token_instruction(0x74, frame_type); // castclass
        self.indexed(0x13, 0x0e, frame); // stloc
        let first_parameter = frame.saturating_add(1);
        let locals_start = PARAMETERS + parameters.len();
        self.async_state = Some(State {
            plan,
            first_parameter,
            locals_start,
            scratch_start: locals_start + self.locals.len(),
            scratch_size: 0,
            resumes: Vec::new(),
            failure: None,
        });
        let protected_start = self.code.len();
        for (index, &ty) in parameters.iter().enumerate() {
            let parameter = self.reserve_spill(StackSlot::Value(ty));
            self.restore_async_local(PARAMETERS + index, parameter, StackSlot::Value(ty));
        }
        let dispatch = self.branch(0x38); // br
        let start = self.code.len();
        self.block(block, 0);
        if block.ty != Type::Never {
            self.complete_async(block.ty);
        }

        self.patch(dispatch);
        self.load_async_slot(STATE, StackSlot::Value(Type::I32));
        self.op(0x25); // dup
        let first_call = self.branch(0x39); // brfalse
        for index in 0..self.locals.len() {
            self.restore_async_local(
                locals_start + index,
                index as u16,
                self.async_local_type(index),
            );
        }
        self.patch(first_call);
        let state = self.async_state.take().expect("active async method");
        if let Some(error) = state.failure {
            return Err(error);
        }
        self.op(0x45); // switch
        self.code
            .extend_from_slice(&((state.resumes.len() + 1) as u32).to_le_bytes());
        let table_end = self.code.len() + (state.resumes.len() + 1) * 4;
        for target in std::iter::once(start).chain(state.resumes) {
            self.code
                .extend_from_slice(&((target as i64 - table_end as i64) as i32).to_le_bytes());
        }
        // A CLR awaiter schedules exactly one continuation for each registration.
        // An invalid state must still leave the protected region with an empty stack.
        let invalid = self.branch(0xdd); // leave
        self.return_jumps.push(invalid);
        let protected_end = self.code.len();

        let exception = self.reserve_spill(StackSlot::Value(Type::Object));
        let cancellation = self.reserve_spill(StackSlot::Value(plan.cancellation));
        let canceled = self.code.len();
        self.indexed(0x13, 0x0e, cancellation); // stloc
        self.load_async_slot(COMPLETION, StackSlot::Value(plan.completion));
        self.indexed(0x11, 0x0c, cancellation); // ldloc
        self.async_call(plan.cancellation_token, true);
        self.async_call(plan.set_canceled, true);
        let leave = self.branch(0xdd);
        self.return_jumps.push(leave);
        let canceled_end = self.code.len();
        let cancellation_type = self.metadata.runtime_type_token(plan.cancellation);
        self.clauses.push(ExceptionClause {
            protected: protected_start..protected_end,
            handler: canceled..canceled_end,
            class: Some(cancellation_type),
        });

        self.indexed(0x13, 0x0e, exception); // stloc
        self.load_async_slot(COMPLETION, StackSlot::Value(plan.completion));
        self.indexed(0x11, 0x0c, exception); // ldloc
        self.token_instruction(0x74, BASE_EXCEPTION); // castclass
        self.async_call(plan.set_exception, true);
        let leave = self.branch(0xdd);
        self.return_jumps.push(leave);
        self.clauses.push(ExceptionClause {
            protected: protected_start..protected_end,
            handler: canceled_end..self.code.len(),
            class: Some(BASE_EXCEPTION),
        });
        for jump in std::mem::take(&mut self.return_jumps) {
            self.patch(jump);
        }
        self.op(0x2a); // ret
        Ok(state.scratch_start + state.scratch_size)
    }

    pub(super) fn emit_async_start(
        &mut self,
        frame_size: usize,
        step: u32,
        plan: Plan,
        parameters: &[Type],
    ) {
        self.max_stack = 5;
        let frame = self.reserve_spill(StackSlot::Value(plan.frame));
        self.load_i32(frame_size as i32);
        let object = self.metadata.runtime_type_token(Type::Object);
        self.token_instruction(0x8d, object); // newarr
        self.indexed(0x13, 0x0e, frame); // stloc

        self.frame_slot(frame, STATE);
        self.load_i32(0);
        self.box_async(Type::I32);
        self.op(0xa2); // stelem.ref
        self.frame_slot(frame, COMPLETION);
        let (_, ctor) = self
            .metadata
            .member(plan.constructor.id, plan.constructor.owner);
        self.token_instruction(0x73, super::MEMBER_REF | ctor); // newobj
        self.op(0xa2); // stelem.ref
        self.frame_slot(frame, CONTINUATION);
        self.indexed(0x11, 0x0c, frame);
        self.extended(0x06); // ldftn
        self.code.extend_from_slice(&step.to_le_bytes());
        let delegate = self
            .metadata
            .delegate_ctor(plan.action_definition, plan.action);
        self.token_instruction(0x73, delegate); // newobj
        self.op(0xa2); // stelem.ref
        for (index, &ty) in parameters.iter().enumerate() {
            self.frame_slot(frame, PARAMETERS + index);
            self.indexed(0x0e, 0x09, index as u16); // ldarg
            self.box_async(ty);
            self.op(0xa2); // stelem.ref
        }
        // Run the initial step on the calling thread while keeping any ambient
        // context changes inside the async method.
        let context = self.reserve_spill(StackSlot::Value(plan.context));
        self.async_call(plan.capture_context, false);
        self.indexed(0x13, 0x0e, context);
        self.indexed(0x11, 0x0c, context);
        let suppressed = self.branch(0x39); // brfalse
        self.indexed(0x11, 0x0c, context);
        self.op(0x14); // ldnull, an unbound static callback
        self.extended(0x06); // ldftn
        self.code.extend_from_slice(&step.to_le_bytes());
        let callback = self
            .metadata
            .delegate_ctor(plan.context_callback_definition, plan.context_callback);
        self.token_instruction(0x73, callback); // newobj
        self.indexed(0x11, 0x0c, frame);
        self.async_call(plan.run_context, false);
        let started = self.branch(0x38);
        self.patch(suppressed);
        self.indexed(0x11, 0x0c, frame);
        self.call(step);
        self.patch(started);
        self.frame_slot(frame, COMPLETION);
        self.op(0x9a); // ldelem.ref
        let completion = self.metadata.runtime_type_token(plan.completion);
        self.token_instruction(0x74, completion); // castclass
        self.async_call(plan.get_task, true);
        self.op(0x2a); // ret
    }

    pub(super) fn suspend(&mut self, await_: &Suspension, base: usize) {
        let pending = self.pending.clone();
        if pending.len() != base
            || pending.iter().any(|&slot| match slot {
                StackSlot::Address(_) => true,
                StackSlot::Value(ty) => self.metadata.stack_only(ty),
                StackSlot::Vector(_) => false,
            })
        {
            let state = self.async_state.as_mut().expect("checked async suspension");
            state.failure.get_or_insert_with(|| Diagnostic::new(
                "E0351", await_.awaiter.span,
                "this expression keeps a managed address or stack-only value live across '.await'",
            ));
            return;
        }
        self.expr(&await_.awaiter, base);
        let awaiter_type = StackSlot::Value(await_.awaiter.ty);
        let awaiter = self.spill(awaiter_type).expect("value-type awaiter");
        let mut spills = pending
            .iter()
            .rev()
            .map(|&slot| self.spill(slot))
            .collect::<Vec<_>>();
        spills.reverse();

        self.indexed(0x12, 0x0d, awaiter); // ldloca
        self.async_call(await_.completed, false);
        let ready = self.branch(0x3a); // brtrue
        let frame = self.async_state.as_ref().expect("active suspension");
        let state = frame.resumes.len() + 1;
        let (locals_start, scratch_start, action) =
            (frame.locals_start, frame.scratch_start, frame.plan.action);
        for index in 0..self.locals.len() {
            self.save_async_local(
                locals_start + index,
                index as u16,
                self.async_local_type(index),
            );
        }
        self.save_async_local(scratch_start, awaiter, awaiter_type);
        for (index, (&slot, &local)) in pending.iter().zip(&spills).enumerate() {
            if let Some(local) = local {
                self.save_async_local(scratch_start + 1 + index, local, slot);
            }
        }
        self.load_frame_slot(STATE);
        self.load_i32(state as i32);
        self.box_async(Type::I32);
        self.op(0xa2); // stelem.ref
        self.indexed(0x12, 0x0d, awaiter);
        self.load_async_slot(CONTINUATION, StackSlot::Value(action));
        self.async_call(await_.subscribe, false);
        let leave = self.branch(0xdd);
        self.return_jumps.push(leave);

        let state = self.async_state.as_mut().expect("active suspension");
        state.resumes.push(self.code.len());
        state.scratch_size = state.scratch_size.max(1 + spills.len());
        self.restore_async_local(scratch_start, awaiter, awaiter_type);
        for (index, (&slot, &local)) in pending.iter().zip(&spills).enumerate() {
            if let Some(local) = local {
                self.restore_async_local(scratch_start + 1 + index, local, slot);
            }
        }
        self.patch(ready);
        self.reload_operands(spills);
        self.indexed(0x12, 0x0d, awaiter);
        self.async_call(await_.result, false);
    }

    pub(super) fn async_return(&mut self, value: Option<&Expr>, base: usize) {
        let mut stored = None;
        if let Some(value) = value {
            self.expr(value, base);
            if value.ty == Type::Never {
                return;
            }
            if base != 0 && value.ty != Type::Void {
                stored = self.spill(StackSlot::Value(value.ty));
            }
        }
        self.discard_above(base, 0);
        if let Some(slot) = stored {
            self.indexed(0x11, 0x0c, slot);
        }
        self.complete_async(value.map_or(Type::Void, |value| value.ty));
    }

    fn complete_async(&mut self, output: Type) {
        let plan = self.async_state.as_ref().expect("active async method").plan;
        let result = (output != Type::Void).then(|| {
            self.spill(StackSlot::Value(plan.output))
                .expect("typed async result")
        });
        self.load_async_slot(COMPLETION, StackSlot::Value(plan.completion));
        if let Some(result) = result {
            self.indexed(0x11, 0x0c, result);
        }
        self.async_call(plan.set_result, true);
        let leave = self.branch(0xdd);
        self.return_jumps.push(leave);
    }

    fn async_call(&mut self, member: Member, reference_receiver: bool) {
        let (_, row) = self.metadata.member(member.id, member.owner);
        self.token_instruction(
            if reference_receiver { 0x6f } else { 0x28 },
            super::MEMBER_REF | row,
        );
    }

    fn frame_slot(&mut self, frame: u16, index: usize) {
        self.indexed(0x11, 0x0c, frame);
        self.load_i32(index as i32);
    }

    fn load_frame_slot(&mut self, index: usize) {
        // The step's first synthetic IL local holds the typed frame.
        self.indexed(0x11, 0x0c, self.locals.len() as u16); // ldloc
        self.load_i32(index as i32);
    }

    fn load_async_slot(&mut self, index: usize, slot: StackSlot) {
        self.load_frame_slot(index);
        self.op(0x9a); // ldelem.ref
        let (token, reference) = match slot {
            StackSlot::Value(ty) => (self.metadata.runtime_type_token(ty), ty.reference()),
            StackSlot::Vector(element) => (self.metadata.cell_array_token(element), true),
            StackSlot::Address(_) => unreachable!("addresses cannot be stored in an async frame"),
        };
        self.token_instruction(if reference { 0x74 } else { 0xa5 }, token); // castclass / unbox.any
    }

    fn box_async(&mut self, ty: Type) {
        if !ty.reference() {
            let token = self.metadata.runtime_type_token(ty);
            self.token_instruction(0x8c, token); // box
        }
    }

    fn async_local_type(&self, index: usize) -> StackSlot {
        if self.captured_cells.contains(&(index as u16)) {
            StackSlot::Vector(self.locals[index])
        } else {
            StackSlot::Value(self.locals[index])
        }
    }

    fn save_async_local(&mut self, index: usize, local: u16, slot: StackSlot) {
        self.load_frame_slot(index);
        self.indexed(0x11, 0x0c, local);
        if let StackSlot::Value(ty) = slot {
            self.box_async(ty);
        }
        self.op(0xa2); // stelem.ref
    }

    fn restore_async_local(&mut self, index: usize, local: u16, slot: StackSlot) {
        self.load_async_slot(index, slot);
        self.indexed(0x13, 0x0e, local);
    }
}
