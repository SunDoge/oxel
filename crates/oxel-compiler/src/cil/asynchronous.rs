//! Emits resumable CLR methods using typed IL locals and a heap frame at suspension.

use super::{BASE_EXCEPTION, Body, ExceptionClause, StackSlot};
use crate::{
    Diagnostic,
    hir::{
        Block, ExceptionRegion, Expr, Member, Type, UsingRegion,
        asynchronous::{Completion, Plan, Suspension},
    },
};

const STATE: usize = 0;
const COMPLETION: usize = 1;
const CONTINUATION: usize = 2;
const PARAMETERS: usize = 3;

pub(super) struct State {
    plan: Plan,
    first_parameter: u16,
    resume_state: u16,
    locals_start: usize,
    scratch_start: usize,
    scratch_size: usize,
    resumes: Vec<usize>,
    failure: Option<Diagnostic>,
    completion_jumps: Vec<usize>,
    suspend_flag: Option<u16>,
    cleanup_depth: usize,
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
        let protected_start = self.code.len();
        for (index, &ty) in parameters.iter().enumerate() {
            let parameter = self.reserve_spill(StackSlot::Value(ty));
            self.restore_async_local(PARAMETERS + index, parameter, StackSlot::Value(ty));
        }
        let resume_state = self.reserve_spill(StackSlot::Value(Type::I32));
        self.async_state = Some(State {
            plan,
            first_parameter,
            resume_state,
            locals_start,
            scratch_start: locals_start + self.locals.len(),
            scratch_size: 0,
            resumes: Vec::new(),
            failure: None,
            completion_jumps: Vec::new(),
            suspend_flag: None,
            cleanup_depth: 0,
        });
        let dispatch = self.branch(0x38); // br
        let start = self.code.len();
        self.block(block, 0);
        if block.ty != Type::Never {
            self.complete_async(block.ty);
        }

        let completion_jumps = std::mem::take(
            &mut self
                .async_state
                .as_mut()
                .expect("active async method")
                .completion_jumps,
        );
        if !completion_jumps.is_empty() {
            for jump in completion_jumps {
                self.patch(jump);
            }
            if let Some(slot) = self.return_slot {
                self.indexed(0x11, 0x0c, slot);
            }
            self.complete_async(plan.output);
        }

        self.patch(dispatch);
        self.load_async_slot(STATE, StackSlot::Value(Type::I32));
        self.indexed(0x13, 0x0e, resume_state); // stloc
        self.indexed(0x11, 0x0c, resume_state); // ldloc
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
        self.indexed(0x11, 0x0c, resume_state); // ldloc
        let mut targets = vec![start];
        targets.extend(state.resumes);
        self.async_switch(&targets);
        // A CLR awaiter schedules exactly one continuation for each registration.
        // An invalid state must still leave the protected region with an empty stack.
        let invalid = self.branch(0xdd); // leave
        self.return_jumps.push(invalid);
        let protected_end = self.code.len();

        let exception = self.reserve_spill(StackSlot::Value(Type::Object));
        if let Completion::Task { canceled, .. } = plan.completion {
            let cancellation = self.reserve_spill(StackSlot::Value(canceled.exception));
            let start = self.code.len();
            self.indexed(0x13, 0x0e, cancellation); // stloc
            self.load_completion(plan);
            self.indexed(0x11, 0x0c, cancellation); // ldloc
            self.async_call(canceled.token, true);
            self.async_call(canceled.complete, true);
            let leave = self.branch(0xdd);
            self.return_jumps.push(leave);
            let cancellation_type = self.metadata.runtime_type_token(canceled.exception);
            self.clauses.push(ExceptionClause {
                protected: protected_start..protected_end,
                handler: start..self.code.len(),
                class: Some(cancellation_type),
            });
        }

        let failed = self.code.len();
        self.indexed(0x13, 0x0e, exception); // stloc
        self.load_completion(plan);
        self.indexed(0x11, 0x0c, exception); // ldloc
        self.token_instruction(0x74, BASE_EXCEPTION); // castclass
        self.async_call(plan.set_exception, plan.completion_type.reference());
        let leave = self.branch(0xdd);
        self.return_jumps.push(leave);
        self.clauses.push(ExceptionClause {
            protected: protected_start..protected_end,
            handler: failed..self.code.len(),
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
        match plan.completion {
            Completion::Task { constructor, .. } => self.new_async_object(constructor),
            Completion::Void { create } => {
                self.async_call(create, false);
                self.box_async(plan.completion_type);
            }
        }
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
        if let Completion::Task { get_task, wrap, .. } = plan.completion {
            self.frame_slot(frame, COMPLETION);
            self.op(0x9a); // ldelem.ref
            let completion = self.metadata.runtime_type_token(plan.completion_type);
            self.token_instruction(0x74, completion); // castclass
            self.async_call(get_task, true);
            if let Some(wrap) = wrap {
                self.new_async_object(wrap);
            }
        }
        self.op(0x2a); // ret
    }

    pub(super) fn suspend(&mut self, await_: &Suspension, base: usize) {
        if self.spill_overflow {
            return;
        }
        let pending = self.pending.clone();
        // A try region has already moved its enclosing operands into locals.
        // Keep those values alive even though its evaluation stack is empty.
        let mut region_locals: Vec<_> = self
            .region_spills
            .iter()
            .flatten()
            .flatten()
            .map(|&local| {
                (
                    local,
                    self.spill_locals[usize::from(local) - self.locals.len()],
                )
            })
            .collect();
        if pending.len() != base
            || pending
                .iter()
                .copied()
                .chain(region_locals.iter().map(|&(_, slot)| slot))
                .any(|slot| match slot {
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
        region_locals.extend(
            spills
                .iter()
                .zip(&pending)
                .filter_map(|(&local, &slot)| local.map(|local| (local, slot))),
        );

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
        for (index, &(local, slot)) in region_locals.iter().enumerate() {
            self.save_async_local(scratch_start + 1 + index, local, slot);
        }
        self.load_frame_slot(STATE);
        self.load_i32(state as i32);
        self.box_async(Type::I32);
        self.op(0xa2); // stelem.ref
        self.indexed(0x12, 0x0d, awaiter);
        self.load_async_slot(CONTINUATION, StackSlot::Value(action));
        self.async_call(await_.subscribe, false);
        let state = self.async_state.as_ref().expect("active suspension");
        if state.cleanup_depth != 0 {
            let flag = state.suspend_flag.expect("active cleanup");
            self.op(0x17); // ldc.i4.1
            self.indexed(0x13, 0x0e, flag); // stloc
        }
        let leave = self.branch(0xdd);
        self.return_jumps.push(leave);

        let state = self.async_state.as_mut().expect("active suspension");
        state.resumes.push(self.code.len());
        state.scratch_size = state.scratch_size.max(1 + region_locals.len());
        let resume_state = state.resume_state;
        self.restore_async_local(scratch_start, awaiter, awaiter_type);
        for (index, (local, slot)) in region_locals.into_iter().enumerate() {
            self.restore_async_local(scratch_start + 1 + index, local, slot);
        }
        self.op(0x16); // ldc.i4.0, later regions use their normal entry
        self.indexed(0x13, 0x0e, resume_state); // stloc
        self.patch(ready);
        self.reload_operands(spills);
        self.indexed(0x12, 0x0d, awaiter);
        self.async_call(await_.result, false);
    }

    pub(super) fn async_try_catch(&mut self, region: &ExceptionRegion, ty: Type, base: usize) {
        let pending = self.begin_region(base);
        let (protected, end) = self.async_protected_body(&region.body, region.result, ty);
        let mut ends: Vec<_> = end.into_iter().collect();
        let mut handlers = Vec::with_capacity(region.catches.len());
        for arm in &region.catches {
            let start = self.code.len();
            if let Some(local) = arm.local {
                self.indexed(0x13, 0x0e, local); // stloc the thrown exception
            } else {
                self.op(0x26); // pop
            }
            handlers.push(self.branch(0xdd)); // leave
            let class = arm
                .ty
                .map_or(BASE_EXCEPTION, |ty| self.metadata.runtime_type_token(ty));
            self.clauses.push(ExceptionClause {
                protected: protected.clone(),
                handler: start..self.code.len(),
                class: Some(class),
            });
        }
        // Run the source catch blocks outside the CLR handlers so an await
        // can resume there, and a new exception only reaches enclosing catches.
        for (arm, handler) in region.catches.iter().zip(handlers) {
            self.patch(handler);
            self.block(&arm.body, 0);
            if arm.body.ty != Type::Never {
                ends.push(self.leave_exception_region(region.result, ty));
            }
        }
        self.end_region(pending, ends, region.result, ty);
    }

    pub(super) fn async_using(&mut self, region: &UsingRegion, ty: Type, base: usize) {
        self.expr(&region.resource, base);
        self.indexed(0x13, 0x0e, region.local); // stloc
        let pending = self.begin_region(base);
        let flag = self
            .async_state
            .as_ref()
            .expect("async using")
            .suspend_flag
            .unwrap_or_else(|| self.reserve_spill(StackSlot::Value(Type::Bool)));
        let state = self.async_state.as_mut().expect("async using");
        state.suspend_flag = Some(flag);
        state.cleanup_depth += 1;
        let (protected, end) = self.async_protected_body(&region.body, region.result, ty);
        self.async_state
            .as_mut()
            .expect("async using")
            .cleanup_depth -= 1;

        let handler = self.code.len();
        // A suspension leaves all CLR regions. Keep source resources live in
        // the frame until a resumed invocation actually exits their scope.
        self.indexed(0x11, 0x0c, flag); // ldloc
        let suspended = self.branch(0x3a); // brtrue
        self.expr(&region.dispose, 0);
        self.patch(suspended);
        self.op(0xdc); // endfinally
        self.clauses.push(ExceptionClause {
            protected,
            handler: handler..self.code.len(),
            class: None,
        });
        self.end_region(pending, end, region.result, ty);
    }

    fn async_protected_body(
        &mut self,
        body: &Block,
        result: Option<u16>,
        ty: Type,
    ) -> (std::ops::Range<usize>, Option<usize>) {
        let entry = self.code.len();
        self.op(0x00); // resume dispatch reaches this point outside the protected body
        let start = self.code.len();
        let dispatch = self.branch(0x38); // br inside the protected region
        let body_start = self.code.len();
        let first_resume = self.async_state.as_ref().expect("async try").resumes.len();
        self.block(body, 0);
        let end = (body.ty != Type::Never).then(|| self.leave_exception_region(result, ty));

        let state = self.async_state.as_mut().expect("async try");
        let resumes = &mut state.resumes[first_resume..];
        if resumes.is_empty() {
            self.patch_to(dispatch, body_start);
        } else {
            // Each outer dispatcher targets the region entrance, then the
            // dispatcher inside the region can reach its own resume addresses.
            let targets = resumes.to_vec();
            resumes.fill(entry);
            let resume_state = state.resume_state;
            self.patch(dispatch);
            self.indexed(0x11, 0x0c, resume_state); // ldloc
            self.load_i32((first_resume + 1) as i32);
            self.op(0x59); // sub
            self.async_switch(&targets);
            let normal = self.branch(0x38);
            self.patch_to(normal, body_start);
        }

        (start..self.code.len(), end)
    }

    fn async_switch(&mut self, targets: &[usize]) {
        self.op(0x45); // switch
        self.code
            .extend_from_slice(&(targets.len() as u32).to_le_bytes());
        let end = self.code.len() + targets.len() * 4;
        for &target in targets {
            self.code
                .extend_from_slice(&((target as i64 - end as i64) as i32).to_le_bytes());
        }
    }

    pub(super) fn async_return(&mut self, value: Option<&Expr>, base: usize) {
        if let Some(value) = value {
            self.expr(value, base);
            if value.ty == Type::Never {
                return;
            }
            if value.ty != Type::Void {
                let output = self.async_state.as_ref().expect("async return").plan.output;
                let slot = match self.return_slot {
                    Some(slot) => slot,
                    None => {
                        let slot = self.reserve_spill(StackSlot::Value(output));
                        self.return_slot = Some(slot);
                        slot
                    }
                };
                self.indexed(0x13, 0x0e, slot); // stloc
            }
        }
        self.discard_above(base, 0);
        // Complete the task only after all enclosing cleanup has succeeded.
        let jump = self.branch(if self.region_spills.is_empty() {
            0x38
        } else {
            0xdd
        });
        self.async_state
            .as_mut()
            .expect("async return")
            .completion_jumps
            .push(jump);
    }

    fn complete_async(&mut self, output: Type) {
        let plan = self.async_state.as_ref().expect("active async method").plan;
        let result = (output != Type::Void).then(|| {
            self.spill(StackSlot::Value(plan.output))
                .expect("typed async result")
        });
        self.load_completion(plan);
        if let Some(result) = result {
            self.indexed(0x11, 0x0c, result);
        }
        self.async_call(plan.set_result, plan.completion_type.reference());
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

    fn new_async_object(&mut self, constructor: Member) {
        let (_, row) = self.metadata.member(constructor.id, constructor.owner);
        self.token_instruction(0x73, super::MEMBER_REF | row); // newobj
    }

    fn load_completion(&mut self, plan: Plan) {
        if plan.completion_type.reference() {
            self.load_async_slot(COMPLETION, StackSlot::Value(plan.completion_type));
        } else {
            // Mutate the same boxed builder that captured the synchronization context.
            self.load_frame_slot(COMPLETION);
            self.op(0x9a); // ldelem.ref
            let builder = self.metadata.runtime_type_token(plan.completion_type);
            self.token_instruction(0x79, builder); // unbox
        }
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
