//! Lowers typed HIR directly to ECMA-335 method bodies.

mod assembly;
mod asynchronous;

use clrmeta::writer::Writer;
use std::{borrow::Cow, collections::HashSet};

use crate::{
    AssemblyKind, Diagnostic, References, Span,
    clr::Invocation,
    hir::{
        ArrayAccess, Block, Destination, ExceptionRegion, Expr, ExprKind, IndexUpdate,
        LiteralCases, Match, Member, PatternLiteral, Program, RangeLoop, Statement, Target,
        TryBlock, Type, UsingRegion, Value,
    },
    syntax::{Binary, LoopControl, Unary},
};

const TYPE_DEF: u32 = 0x0200_0000;
const FIELD_DEF: u32 = 0x0400_0000;
const METHOD_DEF: u32 = 0x0600_0000;
const MEMBER_REF: u32 = 0x0a00_0000;
const TYPE_SPEC: u32 = 0x1b00_0000;
const METHOD_SPEC: u32 = 0x2b00_0000;
const STAND_ALONE_SIG: u32 = 0x1100_0000;

const WRITE_LINE_OVERLOADS: [Option<Type>; 11] = [
    None,
    Some(Type::Bool),
    Some(Type::I32),
    Some(Type::I64),
    Some(Type::F32),
    Some(Type::F64),
    Some(Type::String),
    Some(Type::Object),
    Some(Type::Char),
    Some(Type::U32),
    Some(Type::U64),
];
const STRING_OPERATIONS: [(&str, Type); 2] =
    [("Concat", Type::String), ("op_Equality", Type::Bool)];
const STRING_CONCAT: u32 = MEMBER_REF | (WRITE_LINE_OVERLOADS.len() as u32 + 1);
const STRING_EQUALITY: u32 = MEMBER_REF | (WRITE_LINE_OVERLOADS.len() as u32 + 2);
const INVALID_OPERATION_CTOR: u32 = MEMBER_REF | (WRITE_LINE_OVERLOADS.len() as u32 + 3);
const STRING_LENGTH: u32 = INVALID_OPERATION_CTOR + 1;
const ASSERTION_EXCEPTION_CTOR: u32 = INVALID_OPERATION_CTOR + 3;
const BASE_EXCEPTION: u32 = 0x0100_0006;
const STRING_INDEX: u32 = STRING_LENGTH + 1;
const USER_STRING: u32 = 0x7000_0000;

fn literal_switch(
    ty: Type,
    literals: &LiteralCases,
    targets: &[Destination],
    fallback: Destination,
) -> Option<(i32, Vec<Destination>)> {
    if literals.keys.len() < 4 || (ty != Type::Char && ty.shift_mask().is_none_or(|mask| mask > 31))
    {
        return None;
    }
    let keys = literals.keys.iter().map(|key| match key {
        PatternLiteral::Integer(value) => *value,
        PatternLiteral::Character(value) => i64::from(*value),
        PatternLiteral::String(_) => unreachable!("numeric switch key"),
    });
    let (first, last) = keys
        .clone()
        .fold((i64::MAX, i64::MIN), |(first, last), key| {
            (first.min(key), last.max(key))
        });
    let width = last - first + 1;
    // Bound the IL size and avoid spending more than half the table on holes.
    if width > 4096 || width > 2 * literals.keys.len() as i64 {
        return None;
    }
    let mut table = vec![fallback; width as usize];
    for (key, &target) in keys.zip(targets) {
        table[(key - first) as usize] = target;
    }
    Some((first as i32, table))
}
pub(super) fn emit(
    program: &Program<'_>,
    sources: &[&str],
    name: &str,
    references: &References,
    tests: Option<crate::TestOptions<'_>>,
) -> Result<Vec<u8>, Diagnostic> {
    let kind = if program.entry.is_some() {
        AssemblyKind::Executable
    } else {
        AssemblyKind::Library
    };
    if !kind.valid_name(name) || references.contains(name) {
        return Err(Diagnostic::new(
            "E0401",
            Span::default(),
            "invalid assembly name or name conflicts with a reference",
        ));
    }
    assembly::write(program, sources, name, references, tests)
}

#[derive(Clone, Copy)]
struct FlowTarget {
    base: usize,
    depth: usize,
}

enum Transfer {
    Direct(usize),
    Restore(usize, Vec<Option<u16>>),
}

struct LoopFrame {
    target: FlowTarget,
    exits: Vec<Transfer>,
    continues: Vec<Transfer>,
}

struct TryFrame {
    target: FlowTarget,
    error_local: Option<u16>,
    failures: Vec<Transfer>,
}

#[derive(Clone, Copy)]
enum StackSlot {
    Value(Type),
    Address(Type),
    Vector(Type),
}

impl StackSlot {
    fn of(expr: &Expr) -> Self {
        if matches!(expr.kind, ExprKind::LocalAddress(..)) {
            Self::Address(expr.ty)
        } else {
            Self::Value(expr.ty)
        }
    }
}

struct ExceptionClause {
    protected: std::ops::Range<usize>,
    handler: std::ops::Range<usize>,
    class: Option<u32>,
}

struct Body<'a, 'refs> {
    code: Vec<u8>,
    max_stack: usize,
    metadata: &'a mut assembly::Tables<'refs>,
    loops: Vec<LoopFrame>,
    tries: Vec<TryFrame>,
    lambda_base: usize,
    locals: &'a [Type],
    captured_cells: &'a HashSet<u16>,
    has_exception_regions: bool,
    pending: Vec<StackSlot>,
    spill_locals: Vec<StackSlot>,
    spill_overflow: bool,
    clauses: Vec<ExceptionClause>,
    region_spills: Vec<Vec<Option<u16>>>,
    return_jumps: Vec<usize>,
    return_slot: Option<u16>,
    async_state: Option<asynchronous::State>,
}

impl<'a, 'refs> Body<'a, 'refs> {
    fn new(
        metadata: &'a mut assembly::Tables<'refs>,
        lambda_base: usize,
        locals: &'a [Type],
        captured_cells: &'a HashSet<u16>,
        has_exception_regions: bool,
    ) -> Self {
        Self {
            code: Vec::new(),
            max_stack: 0,
            metadata,
            loops: Vec::new(),
            tries: Vec::new(),
            lambda_base,
            locals,
            captured_cells,
            has_exception_regions,
            pending: Vec::new(),
            spill_locals: Vec::new(),
            spill_overflow: false,
            clauses: Vec::new(),
            region_spills: Vec::new(),
            return_jumps: Vec::new(),
            return_slot: None,
            async_state: None,
        }
    }

    fn emit(&mut self, block: &Block) {
        self.block(block, 0);
        if block.ty != Type::Never {
            self.op(0x2a); // ret
        }
        if !self.return_jumps.is_empty() {
            for jump in std::mem::take(&mut self.return_jumps) {
                self.patch(jump);
            }
            if let Some(slot) = self.return_slot {
                self.indexed(0x11, 0x0c, slot); // ldloc
            }
            self.op(0x2a); // ret
        }
    }

    fn write(&mut self, output: &mut Writer, span: Span) -> Result<(), Diagnostic> {
        if self.spill_overflow || self.clauses.len() > (0xff_ffff - 4) / 24 {
            return Err(Diagnostic::new(
                "E0402",
                span,
                "method exceeds CLR exception handling limits",
            ));
        }
        let max_stack = u16::try_from(self.max_stack).map_err(|_| {
            Diagnostic::new(
                "E0402",
                span,
                "method exceeds the CLR evaluation stack limit",
            )
        })?;
        let code_size = u32::try_from(self.code.len()).map_err(|_| {
            Diagnostic::new("E0402", span, "method exceeds the CLR body size limit")
        })?;
        let locals_token =
            self.metadata
                .add_locals(self.locals, self.captured_cells, &self.spill_locals);
        // Fat header with InitLocals and MoreSects when applicable.
        let mut flags = if locals_token == 0 { 0x3003 } else { 0x3013 };
        if !self.clauses.is_empty() {
            flags |= 0x0008;
        }
        output.write_u16(flags);
        output.write_u16(max_stack);
        output.write_u32(code_size);
        output.write_u32(locals_token);
        output.write_bytes(&self.code);
        if !self.clauses.is_empty() {
            output.align(4);
            let size = 4 + 24 * self.clauses.len() as u32;
            output.write_u32(0x41 | size << 8); // fat exception table
            for clause in &self.clauses {
                output.write_u32(if clause.class.is_some() { 0 } else { 2 }); // catch or finally
                output.write_u32(clause.protected.start as u32);
                output.write_u32(clause.protected.len() as u32);
                output.write_u32(clause.handler.start as u32);
                output.write_u32(clause.handler.len() as u32);
                output.write_u32(clause.class.unwrap_or(0));
            }
        }
        Ok(())
    }

    fn op(&mut self, opcode: u8) {
        self.code.push(opcode);
    }

    fn extended(&mut self, opcode: u8) {
        self.code.extend_from_slice(&[0xfe, opcode]);
    }

    fn indexed(&mut self, short: u8, long: u8, index: u16) {
        if let Ok(index) = u8::try_from(index) {
            self.code.extend_from_slice(&[short, index]);
        } else {
            self.extended(long);
            self.code.extend_from_slice(&index.to_le_bytes());
        }
    }

    fn branch(&mut self, opcode: u8) -> usize {
        self.op(opcode);
        let operand = self.code.len();
        self.code.extend_from_slice(&[0; 4]);
        operand
    }

    fn patch(&mut self, operand: usize) {
        self.patch_to(operand, self.code.len());
    }

    fn patch_to(&mut self, operand: usize, target: usize) {
        let distance = target as i64 - (operand + 4) as i64;
        self.code[operand..operand + 4].copy_from_slice(&(distance as i32).to_le_bytes());
    }

    fn discard_above(&mut self, base: usize, keep: usize) {
        for _ in keep..base {
            self.op(0x26); // pop
        }
    }

    fn while_loop(&mut self, condition: &Expr, body: &Block, base: usize) {
        let header = self.code.len();
        self.expr(condition, base);
        if condition.ty == Type::Never {
            return;
        }
        let condition_exit = self.branch(0x39); // brfalse
        self.loops.push(LoopFrame {
            target: self.flow_target(base),
            exits: vec![Transfer::Direct(condition_exit)],
            continues: Vec::new(),
        });
        self.block(body, base);
        if body.ty != Type::Never {
            let repeat = self.branch(0x38); // br
            self.patch_to(repeat, header);
        }
        let frame = self.loops.pop().expect("emitted loop body");
        self.patch_loop(frame, header);
    }

    fn range_loop(&mut self, range: &RangeLoop, base: usize) {
        for (value, local) in [(&range.start, range.index), (&range.end, range.limit)] {
            self.expr(value, base);
            if value.ty == Type::Never {
                return;
            }
            self.indexed(0x13, 0x0e, local); // stloc
        }
        self.max_stack = self.max_stack.max(base + 2);
        let header = self.code.len();
        self.indexed(0x11, 0x0c, range.index);
        self.indexed(0x11, 0x0c, range.limit);
        let empty = self.branch(match (range.start.ty.unsigned(), range.inclusive) {
            (true, true) => 0x42,   // bgt.un
            (true, false) => 0x41,  // bge.un
            (false, true) => 0x3d,  // bgt
            (false, false) => 0x3c, // bge
        });
        let body_start = self.code.len();
        self.loops.push(LoopFrame {
            target: self.flow_target(base),
            exits: vec![Transfer::Direct(empty)],
            continues: Vec::new(),
        });
        self.block(&range.body, base);
        let step = self.code.len();
        let mut frame = self.loops.pop().expect("emitted loop body");
        if range.body.ty != Type::Never || !frame.continues.is_empty() {
            if range.inclusive {
                // Skip incrementing the inclusive endpoint, even at the type's maximum.
                self.indexed(0x11, 0x0c, range.index);
                self.indexed(0x11, 0x0c, range.limit);
                frame.exits.push(Transfer::Direct(self.branch(0x3b))); // beq
            }
            self.indexed(0x11, 0x0c, range.index);
            self.op(0x17); // ldc.i4.1
            if range.start.ty.wide_integer() {
                self.op(0x6a); // conv.i8
            }
            self.op(0x58); // add
            self.indexed(0x13, 0x0e, range.index);
            let repeat = self.branch(0x38); // br
            self.patch_to(repeat, if range.inclusive { body_start } else { header });
        }
        self.patch_loop(frame, step);
    }

    fn flow_target(&self, base: usize) -> FlowTarget {
        FlowTarget {
            base,
            depth: self.region_spills.len(),
        }
    }

    fn transfer(&mut self, target: FlowTarget, base: usize) -> Transfer {
        if self.region_spills.len() == target.depth {
            self.discard_above(base, target.base);
            return Transfer::Direct(self.branch(0x38)); // br
        }

        // leave clears the stack. The first exited region saved the operands
        // that were already live in the destination's scope.
        let jump = self.branch(0xdd); // leave
        if target.base == 0 {
            Transfer::Direct(jump)
        } else {
            Transfer::Restore(
                jump,
                self.region_spills[target.depth][..target.base].to_vec(),
            )
        }
    }

    fn patch_transfers(&mut self, mut transfers: Vec<Transfer>, target: Option<usize>) {
        if transfers
            .iter()
            .any(|jump| matches!(jump, Transfer::Restore(..)))
        {
            let skip = self.branch(0x38); // skip out-of-line stack restoration
            for transfer in &mut transfers {
                if let Transfer::Restore(jump, spills) = transfer {
                    self.patch(*jump);
                    self.reload_operands(std::mem::take(spills));
                    *transfer = Transfer::Direct(self.branch(0x38)); // br
                }
            }
            self.patch(skip);
        }
        let target = target.unwrap_or(self.code.len());
        for transfer in transfers {
            let Transfer::Direct(jump) = transfer else {
                unreachable!("restored control transfer");
            };
            self.patch_to(jump, target);
        }
    }

    fn patch_loop(&mut self, frame: LoopFrame, continuation: usize) {
        self.patch_transfers(frame.continues, Some(continuation));
        self.patch_transfers(frame.exits, None);
    }

    fn loop_control(&mut self, kind: LoopControl, base: usize) {
        let target = self.loops.last().expect("checked loop control").target;
        let jump = self.transfer(target, base);
        let frame = self.loops.last_mut().expect("checked loop control");
        match kind {
            LoopControl::Break => frame.exits.push(jump),
            LoopControl::Continue => frame.continues.push(jump),
        }
    }

    fn block(&mut self, block: &Block, base: usize) {
        for statement in &block.statements {
            match statement {
                Statement::DeclareLocal { index, value } => {
                    if let Some(element) = self.cell_element(Value::Local(*index)) {
                        // Allocate a fresh cell each time the declaration executes.
                        self.max_stack = self.max_stack.max(base + 1);
                        self.load_i32(1);
                        let token = self.metadata.runtime_type_token(element);
                        self.token_instruction(0x8d, token); // newarr
                        self.indexed(0x13, 0x0e, *index); // stloc
                    }
                    if let Some(value) = value
                        && !self.store(Value::Local(*index), value, base)
                    {
                        return;
                    }
                }
                Statement::Store { target, value, .. } => {
                    if !self.store(*target, value, base) {
                        return;
                    }
                }
                Statement::Return(value) => {
                    self.return_value(value.as_ref(), base);
                    return;
                }
                Statement::TryFailure(error) => {
                    let frame = self.tries.last().expect("checked try block");
                    let (target, local) = (frame.target, frame.error_local);
                    if let Some(error) = error {
                        self.expr(error, base);
                        self.indexed(0x13, 0x0e, local.expect("try error slot")); // stloc
                    }
                    let failure = self.transfer(target, base);
                    self.tries
                        .last_mut()
                        .expect("checked try block")
                        .failures
                        .push(failure);
                    return;
                }
                Statement::While { condition, body } => {
                    self.while_loop(condition, body, base);
                    if condition.ty == Type::Never {
                        return;
                    }
                }
                Statement::For(range) => {
                    self.range_loop(range, base);
                    if range.diverges() {
                        return;
                    }
                }
                Statement::LoopControl(kind) => {
                    self.loop_control(*kind, base);
                    return;
                }
                Statement::Discard(value) => {
                    self.expr(value, base);
                    match value.ty {
                        Type::Never => return,
                        Type::Void => {}
                        _ => self.op(0x26), // pop
                    }
                }
            }
        }
        if let Some(tail) = &block.tail {
            self.expr(tail, base);
        }
    }

    fn return_value(&mut self, value: Option<&Expr>, base: usize) {
        if self.async_state.is_some() {
            self.async_return(value, base);
            return;
        }
        let protected = !self.region_spills.is_empty();
        let mut stored = None;
        if let Some(value) = value {
            // The operand can itself transfer control to the surrounding loop.
            self.expr(value, base);
            if value.ty == Type::Never {
                return;
            }
            if protected || base != 0 {
                let slot = match self.return_slot {
                    Some(slot) => slot,
                    None => {
                        let slot = self.reserve_spill(StackSlot::Value(value.ty));
                        self.return_slot = Some(slot);
                        slot
                    }
                };
                self.indexed(0x13, 0x0e, slot); // stloc
                stored = Some(slot);
            }
        }
        self.discard_above(base, 0);
        if protected {
            let jump = self.branch(0xdd); // leave
            self.return_jumps.push(jump);
        } else {
            if let Some(slot) = stored {
                self.indexed(0x11, 0x0c, slot); // ldloc
            }
            self.op(0x2a); // ret
        }
    }

    fn expr(&mut self, expr: &Expr, base: usize) {
        if !matches!(expr.ty, Type::Never | Type::Void) {
            self.max_stack = self.max_stack.max(base + 1);
        }
        match &expr.kind {
            ExprKind::Await(suspension) => self.suspend(suspension, base),
            ExprKind::Integer(number) if self.metadata.wide_integer(expr.ty) => {
                self.op(0x21); // ldc.i8
                self.code.extend_from_slice(&number.to_le_bytes());
            }
            ExprKind::Integer(number) => self.load_i32(*number as i32),
            ExprKind::Float32(value) => {
                self.op(0x22); // ldc.r4
                self.code.extend_from_slice(&value.to_le_bytes());
            }
            ExprKind::Float64(value) => {
                self.op(0x23); // ldc.r8
                self.code.extend_from_slice(&value.to_le_bytes());
            }
            ExprKind::Boolean(value) => self.op(0x16 + u8::from(*value)), // ldc.i4.0/1
            ExprKind::Null => self.op(0x14),                              // ldnull
            ExprKind::DefaultValue(local) => {
                self.indexed(0x12, 0x0d, *local); // ldloca
                self.extended(0x15); // initobj
                let token = self.metadata.runtime_type_token(expr.ty);
                self.code.extend_from_slice(&token.to_le_bytes());
                self.indexed(0x11, 0x0c, *local); // ldloc
            }
            ExprKind::Coalesce(left, right, temporary) => {
                self.expr(left, base);
                self.indexed(0x13, 0x0e, *temporary); // stloc
                self.indexed(0x11, 0x0c, *temporary); // ldloc
                let present = self.branch(0x3a); // brtrue
                self.expr(right, base);
                if right.ty != Type::Never {
                    self.indexed(0x13, 0x0e, *temporary); // stloc
                }
                self.patch(present);
                self.indexed(0x11, 0x0c, *temporary); // ldloc
            }
            ExprKind::String(value) => {
                let offset = self.metadata.user_string(value);
                self.op(0x72); // ldstr
                self.code
                    .extend_from_slice(&(USER_STRING | offset).to_le_bytes());
            }
            ExprKind::TypeQuery { .. } => {
                unreachable!("type queries are evaluated at compile time")
            }
            ExprKind::Interpolation(parts) => self.interpolation(parts, base),
            ExprKind::Load(Value::Parameter(index)) => {
                if let Some(state) = &self.async_state {
                    self.indexed(0x11, 0x0c, state.parameter(*index)); // ldloc
                } else {
                    self.indexed(0x0e, 0x09, *index); // ldarg
                }
            }
            ExprKind::Load(value @ (Value::Local(_) | Value::CapturedCell(..))) => {
                if let Some(element) = self.cell_element(*value) {
                    self.cell_index(*value, element, base);
                    self.array_element(element, false);
                } else if let Value::Local(index) = value {
                    self.indexed(0x11, 0x0c, *index); // ldloc
                }
            }
            ExprKind::Load(Value::Captured(slot, _)) => {
                self.captured_object(*slot, base);
                let token = self.metadata.runtime_type_token(expr.ty);
                self.token_instruction(if expr.ty.reference() { 0x74 } else { 0xa5 }, token); // castclass or unbox.any
            }
            ExprKind::Lambda {
                method,
                definition,
                type_arguments,
                environment,
            } => {
                self.expr(environment, base);
                self.max_stack = self.max_stack.max(base + 2);
                let method = self.metadata.source_method_token(self.lambda_base + method);
                let method = self.metadata.instantiate_method(method, type_arguments);
                self.extended(0x06); // ldftn
                self.code.extend_from_slice(&method.to_le_bytes());
                let ctor = self.metadata.delegate_ctor(*definition, expr.ty);
                self.token_instruction(0x73, ctor); // newobj
            }
            ExprKind::CaptureCell(value) => match value {
                Value::Local(index) => self.indexed(0x11, 0x0c, *index), // ldloc
                Value::CapturedCell(slot, _) => self.captured_object(*slot, base),
                _ => unreachable!("capture cell storage"),
            },
            ExprKind::LocalAddress(value, _) => self.address(*value, base),
            ExprKind::Construct { ty, local, fields } => {
                self.max_stack = self.max_stack.max(base + 1);
                self.indexed(0x12, 0x0d, *local); // ldloca
                self.extended(0x15); // initobj
                let token = self.metadata.struct_token(*ty);
                self.code.extend_from_slice(&token.to_le_bytes());
                for (field, value) in fields {
                    self.indexed(0x12, 0x0d, *local); // ldloca
                    self.pending_expr(value, base, [StackSlot::Address(*ty)]);
                    if value.ty == Type::Never {
                        return;
                    }
                    self.field(0x7d, *ty, *field); // stfld
                }
                self.indexed(0x11, 0x0c, *local); // ldloc
            }
            ExprKind::Field(receiver, field) => {
                self.expr(receiver, base);
                self.field(0x7b, receiver.ty, *field); // ldfld
            }
            ExprKind::BoxValue(value) => {
                self.expr(value, base);
                let token = self.metadata.runtime_type_token(value.ty);
                self.token_instruction(0x8c, token); // box
                if expr.ty == Type::Object && matches!(value.ty, Type::MethodParameter(_)) {
                    // A generic argument can be a CLR Nullable<T>. Boxing an
                    // empty value cannot satisfy a non-null object signature.
                    self.max_stack = self.max_stack.max(base + 2);
                    self.op(0x25); // dup
                    let nonnull = self.branch(0x3a); // brtrue
                    self.discard_above(base + 1, 0);
                    self.token_instruction(0x73, INVALID_OPERATION_CTOR); // newobj
                    self.op(0x7a); // throw
                    self.patch(nonnull);
                }
            }
            ExprKind::NumericCast(value, target) => {
                self.expr(value, base);
                self.numeric_cast(value.ty, *target);
            }
            ExprKind::CheckedCast(value, target) => {
                self.expr(value, base);
                let token = self.metadata.runtime_type_token(*target);
                let opcode = if target.reference() { 0x74 } else { 0xa5 }; // castclass or unbox.any
                self.token_instruction(opcode, token);
            }
            ExprKind::TypeTest(value, target) => {
                self.expr(value, base);
                let token = self.metadata.runtime_type_token(*target);
                self.token_instruction(0x75, token); // isinst
                self.max_stack = self.max_stack.max(base + 2);
                self.op(0x14); // ldnull
                self.extended(0x03); // cgt.un, non-null reference
            }
            ExprKind::ConstrainedCall {
                method,
                type_arguments,
                receiver,
                temporary,
                args,
            } => {
                self.constrained_call(*method, type_arguments, receiver, *temporary, args, base);
            }
            ExprKind::NewArray { element, length } => {
                self.expr(length, base);
                if length.ty != Type::Never {
                    let token = self.metadata.runtime_type_token(*element);
                    self.token_instruction(0x8d, token); // newarr
                }
            }
            ExprKind::ArrayLiteral { element, items } => {
                self.load_i32(items.len() as i32);
                let token = self.metadata.runtime_type_token(*element);
                self.token_instruction(0x8d, token); // newarr
                for (index, item) in items.iter().enumerate() {
                    self.max_stack = self.max_stack.max(base + 3);
                    self.op(0x25); // dup
                    self.load_i32(index as i32);
                    self.pending_expr(
                        item,
                        base,
                        [
                            StackSlot::Vector(*element),
                            StackSlot::Vector(*element),
                            StackSlot::Value(Type::I32),
                        ],
                    );
                    if item.ty == Type::Never {
                        return;
                    }
                    self.array_element(*element, true);
                }
            }
            ExprKind::StringLength(value) => {
                self.expr(value, base);
                if value.ty != Type::Never {
                    self.token_instruction(0x6f, STRING_LENGTH); // callvirt
                }
            }
            ExprKind::StringIndex(value, index) => {
                self.expr(value, base);
                if value.ty == Type::Never {
                    return;
                }
                self.pending_expr(index, base, [StackSlot::of(value)]);
                if index.ty != Type::Never {
                    self.token_instruction(0x6f, STRING_INDEX); // callvirt
                }
            }
            ExprKind::ArrayLength(array) => {
                self.expr(array, base);
                if array.ty != Type::Never {
                    self.op(0x8e); // ldlen
                    self.op(0x69); // conv.i4
                }
            }
            ExprKind::ArrayAccess(access) => self.array_access(access, base),
            ExprKind::IndexUpdate(update) => self.index_update(update, base),
            ExprKind::Call(target, args) => {
                let checkpoint = self.pending.len();
                for (index, arg) in args.iter().enumerate() {
                    if index == 0 && matches!(target, Target::ExternalField { address: true, .. }) {
                        let ExprKind::Load(local) = arg.kind else {
                            unreachable!("checked struct field receiver");
                        };
                        self.address(local, base);
                        self.keep_operand(StackSlot::Address(arg.ty));
                    } else {
                        self.expr(arg, base + index);
                        if arg.ty == Type::Never {
                            self.pending.truncate(checkpoint);
                            return;
                        }
                        self.keep_operand(StackSlot::of(arg));
                    }
                }
                self.pending.truncate(checkpoint);
                let token = match target {
                    Target::Function(index, types) => self
                        .metadata
                        .instantiate_method(self.metadata.source_method_token(*index), types),
                    Target::External {
                        method,
                        arguments,
                        owner,
                    } => {
                        let (invocation, row) = self.metadata.member(*method, *owner);
                        let opcode = match invocation {
                            Invocation::Static => 0x28,      // call
                            Invocation::Instance => 0x6f,    // callvirt
                            Invocation::Constructor => 0x73, // newobj
                        };
                        let token = self
                            .metadata
                            .instantiate_method(MEMBER_REF | row, arguments);
                        self.token_instruction(opcode, token);
                        return;
                    }
                    Target::ExternalField { member, store, .. } => {
                        let (is_static, volatile, row) =
                            self.metadata.external_field(member.id, member.owner);
                        let opcode = match (is_static, store) {
                            (false, false) => 0x7b, // ldfld
                            (false, true) => 0x7d,  // stfld
                            (true, false) => 0x7e,  // ldsfld
                            (true, true) => 0x80,   // stsfld
                        };
                        if volatile {
                            self.extended(0x13); // volatile.
                        }
                        self.token_instruction(opcode, MEMBER_REF | row);
                        return;
                    }
                    Target::DefaultConstructor(ty) => self.metadata.default_constructor(*ty),
                    Target::ErrorWriter => self.metadata.error_writer_members().0,
                    Target::WriteErrorLine => {
                        let token = self.metadata.error_writer_members().1;
                        self.token_instruction(0x6f, token); // callvirt
                        return;
                    }
                    Target::AssertionException => {
                        self.token_instruction(0x73, ASSERTION_EXCEPTION_CTOR); // newobj
                        return;
                    }
                    Target::WriteLine(ty) => {
                        let index = WRITE_LINE_OVERLOADS
                            .iter()
                            .position(|candidate| candidate == ty)
                            .expect("checked WriteLine overload");
                        MEMBER_REF | (index as u32 + 1)
                    }
                };
                self.call(token);
            }
            ExprKind::Unary(op, operand) => {
                self.expr(operand, base);
                if operand.ty == Type::Never {
                    return;
                }
                match op {
                    Unary::Negate => {
                        self.op(0x65); // neg
                        self.normalize_numeric(operand.ty);
                    }
                    Unary::Not => {
                        self.max_stack = self.max_stack.max(base + 2);
                        self.not();
                    }
                    Unary::Complement => {
                        self.op(0x66); // not
                        self.normalize_numeric(self.metadata.integer_storage(operand.ty));
                    }
                }
            }
            ExprKind::Throw(value) => {
                self.expr(value, base);
                if value.ty != Type::Never {
                    self.op(0x7a); // throw
                }
            }
            ExprKind::Binary(op, left, right) => {
                self.expr(left, base);
                if left.ty == Type::Never {
                    return;
                }
                self.pending_expr(right, base, [StackSlot::of(left)]);
                if right.ty != Type::Never {
                    self.binary(*op, left.ty, base);
                }
            }
            ExprKind::Block(block) => self.block(block, base),
            ExprKind::Try(attempt) => self.try_block(attempt, base),
            ExprKind::TryCatch(region) => self.try_catch(region, expr.ty, base),
            ExprKind::Using(region) => self.using_region(region, expr.ty, base),
            ExprKind::Match(matched) => self.match_expression(matched, base),
            ExprKind::If {
                condition,
                yes,
                no,
                result_local,
            } => {
                self.expr(condition, base);
                if condition.ty == Type::Never {
                    return;
                }
                let on_false = self.branch(0x39); // brfalse
                self.block(yes, base);
                if let Some(no) = no {
                    if let Some(local) = result_local {
                        self.indexed(0x13, 0x0e, *local); // stloc
                    }
                    let done = (yes.ty != Type::Never).then(|| self.branch(0x38)); // br
                    self.patch(on_false);
                    self.expr(no, base);
                    if let Some(local) = result_local {
                        self.indexed(0x13, 0x0e, *local); // stloc
                    }
                    if let Some(done) = done {
                        self.patch(done);
                    }
                    if let Some(local) = result_local {
                        self.indexed(0x11, 0x0c, *local); // ldloc
                    }
                } else {
                    self.patch(on_false);
                }
            }
        }
    }

    fn keep_operand(&mut self, slot: StackSlot) {
        if self.has_exception_regions {
            self.pending.push(slot);
        }
    }

    fn pending_expr<const N: usize>(&mut self, expr: &Expr, base: usize, slots: [StackSlot; N]) {
        if !self.has_exception_regions {
            self.expr(expr, base + N);
            return;
        }
        let checkpoint = self.pending.len();
        self.pending.extend(slots);
        self.expr(expr, base + N);
        self.pending.truncate(checkpoint);
    }

    fn spill(&mut self, slot: StackSlot) -> Option<u16> {
        if matches!(slot, StackSlot::Value(Type::Null)) {
            self.op(0x26); // A typeless null can be reloaded without a local.
            return None;
        }
        let local = self.reserve_spill(slot);
        self.indexed(0x13, 0x0e, local); // stloc
        Some(local)
    }

    fn reserve_spill(&mut self, slot: StackSlot) -> u16 {
        let index = self.locals.len() + self.spill_locals.len();
        if index >= usize::from(u16::MAX) {
            self.spill_overflow = true;
            return 0;
        }
        self.spill_locals.push(slot);
        index as u16
    }

    fn begin_region(&mut self, base: usize) -> Vec<StackSlot> {
        debug_assert_eq!(self.pending.len(), base);
        let pending = std::mem::take(&mut self.pending);
        let mut spills: Vec<_> = pending.iter().rev().map(|&slot| self.spill(slot)).collect();
        spills.reverse();
        self.region_spills.push(spills);
        pending
    }

    fn reload_operands(&mut self, spills: Vec<Option<u16>>) {
        for local in spills {
            if let Some(local) = local {
                self.indexed(0x11, 0x0c, local); // ldloc
            } else {
                self.op(0x14); // ldnull
            }
        }
    }

    fn end_region(
        &mut self,
        pending: Vec<StackSlot>,
        ends: impl IntoIterator<Item = usize>,
        result: Option<u16>,
        ty: Type,
    ) {
        let spills = self.region_spills.pop().expect("active CLR region");
        let mut ends = ends.into_iter().peekable();
        if ends.peek().is_some() {
            for branch in ends {
                self.patch(branch);
            }
            self.reload_operands(spills);
            if let Some(local) = result {
                self.indexed(0x11, 0x0c, local); // ldloc
            } else if ty == Type::Null {
                self.op(0x14); // ldnull
            }
        }
        self.pending = pending;
    }

    fn using_region(&mut self, region: &UsingRegion, ty: Type, base: usize) {
        self.expr(&region.resource, base);
        self.indexed(0x13, 0x0e, region.local); // stloc
        let pending = self.begin_region(base);
        self.op(0x00); // nop outside the protected region
        let start = self.code.len();
        self.block(&region.body, 0);
        let end =
            (region.body.ty != Type::Never).then(|| self.leave_exception_region(region.result, ty));
        let handler = self.code.len();
        self.expr(&region.dispose, 0);
        self.op(0xdc); // endfinally
        self.clauses.push(ExceptionClause {
            protected: start..handler,
            handler: handler..self.code.len(),
            class: None,
        });
        self.end_region(pending, end, region.result, ty);
    }

    fn try_catch(&mut self, region: &ExceptionRegion, ty: Type, base: usize) {
        let pending = self.begin_region(base);

        // Branches to this expression land outside the protected region.
        self.op(0x00); // nop
        let start = self.code.len();
        let mut ends = Vec::new();
        self.block(&region.body, 0);
        if region.body.ty != Type::Never {
            ends.push(self.leave_exception_region(region.result, ty));
        }
        let protected = start..self.code.len();
        let mut clauses = Vec::with_capacity(region.catches.len());
        for arm in &region.catches {
            let start = self.code.len();
            self.max_stack = self.max_stack.max(1);
            if let Some(local) = arm.local {
                self.indexed(0x13, 0x0e, local); // stloc the thrown exception
            } else {
                self.op(0x26); // pop
            }
            self.block(&arm.body, 0);
            if arm.body.ty != Type::Never {
                ends.push(self.leave_exception_region(region.result, ty));
            }
            let class = arm
                .ty
                .map_or(BASE_EXCEPTION, |ty| self.metadata.runtime_type_token(ty));
            clauses.push(ExceptionClause {
                protected: protected.clone(),
                handler: start..self.code.len(),
                class: Some(class),
            });
        }
        // Any nested clauses were emitted before their enclosing clauses.
        self.clauses.extend(clauses);
        self.end_region(pending, ends, region.result, ty);
    }

    fn leave_exception_region(&mut self, result: Option<u16>, ty: Type) -> usize {
        if let Some(local) = result {
            self.indexed(0x13, 0x0e, local); // stloc
        } else if ty == Type::Null {
            self.op(0x26); // pop
        }
        self.branch(0xdd) // leave
    }

    fn try_block(&mut self, attempt: &TryBlock, base: usize) {
        self.tries.push(TryFrame {
            target: self.flow_target(base),
            error_local: attempt.error_local,
            failures: Vec::new(),
        });
        self.block(&attempt.body, base);
        let frame = self.tries.pop().expect("checked try block");
        if frame.failures.is_empty() {
            return;
        }
        let skip_failure = (attempt.body.ty != Type::Never).then(|| self.branch(0x38)); // br
        self.patch_transfers(frame.failures, None);
        self.expr(&attempt.failure, base);
        if let Some(skip) = skip_failure {
            self.patch(skip);
        }
    }

    fn interpolation(&mut self, parts: &[Expr], base: usize) {
        self.max_stack = self.max_stack.max(base + 3);
        self.load_i32(parts.len() as i32);
        let string_type = self.metadata.runtime_type_token(Type::String);
        self.token_instruction(0x8d, string_type); // newarr
        for (index, part) in parts.iter().enumerate() {
            self.op(0x25); // dup
            self.load_i32(index as i32);
            self.pending_expr(
                part,
                base,
                [
                    StackSlot::Vector(Type::String),
                    StackSlot::Vector(Type::String),
                    StackSlot::Value(Type::I32),
                ],
            );
            if part.ty == Type::Never {
                return;
            }
            if part.ty.non_nullable() == Type::Object {
                let convert = self.metadata.string_from_object();
                self.token_instruction(0x28, convert); // call
            }
            self.op(0xa2); // stelem.ref
        }
        let concat = self.metadata.string_array_concat();
        self.token_instruction(0x28, concat); // call
    }

    fn match_expression(&mut self, matched: &Match, base: usize) {
        self.expr(&matched.value, base);
        self.indexed(0x13, 0x0e, matched.local); // stloc

        let mut decision_offsets = Vec::with_capacity(matched.decisions.len());
        let mut switches = Vec::with_capacity(matched.decisions.len());
        let mut branches = Vec::new();
        for decision in &matched.decisions {
            decision_offsets.push(self.code.len());
            let fallback = decision
                .literals
                .as_ref()
                .and_then(|cases| cases.other)
                .unwrap_or(Destination::Invalid);
            let targets = if let Some(literals) = &decision.literals
                && let Some((first, targets)) =
                    literal_switch(decision.value.ty, literals, &decision.targets, fallback)
            {
                self.expr(&decision.value, base);
                if first != 0 {
                    // Switch treats its index as unsigned, so modular subtraction
                    // sends values below the first key to the default branch.
                    self.load_i32(first);
                    self.max_stack = self.max_stack.max(base + 2);
                    self.op(0x59); // sub
                }
                Cow::Owned(targets)
            } else if let Some(literals) = &decision.literals {
                for (key, &target) in literals.keys.iter().zip(&decision.targets) {
                    // The projections read the scrutinee already saved in a local.
                    self.expr(&decision.value, base);
                    let kind = match key {
                        PatternLiteral::Integer(bits) => ExprKind::Integer(*bits),
                        PatternLiteral::Character(value) => ExprKind::Integer(i64::from(*value)),
                        PatternLiteral::String(value) => ExprKind::String(value.clone()),
                    };
                    let literal = Expr {
                        kind,
                        ty: decision.value.ty,
                        span: decision.value.span,
                    };
                    self.pending_expr(&literal, base, [StackSlot::of(&decision.value)]);
                    let jump = if matches!(key, PatternLiteral::String(_)) {
                        self.call(STRING_EQUALITY);
                        self.branch(0x3a) // brtrue
                    } else {
                        self.branch(0x3b) // beq
                    };
                    branches.push((jump, target));
                }
                branches.push((self.branch(0x38), fallback));
                continue;
            } else {
                self.expr(&decision.value, base);
                Cow::Borrowed(decision.targets.as_slice())
            };
            let zero_tag = decision.zero_tag_target();
            if zero_tag.is_some() {
                self.field(0x7b, decision.value.ty, 0); // ldfld tag
            }
            let count = targets.len() + usize::from(zero_tag.is_some());
            self.op(0x45); // switch
            self.code.extend_from_slice(&(count as u32).to_le_bytes());
            let offsets = self.code.len();
            self.code.resize(offsets + 4 * count, 0);
            switches.push((targets, zero_tag, offsets, self.code.len()));
            branches.push((self.branch(0x38), fallback));
        }
        let invalid = self.code.len();
        self.discard_above(base, 0);
        self.token_instruction(0x73, INVALID_OPERATION_CTOR); // newobj
        self.op(0x7a); // throw

        let mut setup_jumps = Vec::with_capacity(matched.setups.len());
        for setup in &matched.setups {
            let offset = self.code.len();
            self.block(&setup.initialization, base);
            setup_jumps.push((offset, self.branch(0x38), setup.arm)); // br
        }
        let mut arm_offsets = Vec::with_capacity(matched.arms.len());
        let mut exits = Vec::new();
        let mut guards = matched.guards.iter().peekable();
        for (index, arm) in matched.arms.iter().enumerate() {
            arm_offsets.push(self.code.len());
            if let Some(guard) = guards.next_if(|guard| guard.arm == index) {
                self.block(&guard.condition, base);
                if guard.condition.ty == Type::Never {
                    continue;
                }
                let failure = self.branch(0x39); // brfalse
                self.patch_to(failure, decision_offsets[guard.fallback]);
            }
            self.block(arm, base);
            if arm.ty != Type::Never {
                if let Some(local) = matched.result_local {
                    self.indexed(0x13, 0x0e, local); // stloc
                }
                exits.push(self.branch(0x38)); // br
            }
        }
        for &(_, jump, arm) in &setup_jumps {
            self.patch_to(jump, arm_offsets[arm]);
        }
        let offset_of = |target| match target {
            Destination::Invalid => invalid,
            Destination::Arm(id) => arm_offsets[id],
            Destination::Setup(id) => setup_jumps[id].0,
            Destination::Decision(id) => decision_offsets[id],
        };
        for (targets, zero_tag, offsets, end) in switches {
            for (index, target) in zero_tag
                .into_iter()
                .chain(targets.iter().copied())
                .enumerate()
            {
                let target = offset_of(target);
                let offset = offsets + 4 * index;
                let distance = (target as i64 - end as i64) as i32;
                self.code[offset..offset + 4].copy_from_slice(&distance.to_le_bytes());
            }
        }
        for (jump, target) in branches {
            self.patch_to(jump, offset_of(target));
        }
        for exit in exits {
            self.patch(exit);
        }
        if let Some(local) = matched.result_local {
            self.indexed(0x11, 0x0c, local); // ldloc
        }
    }

    fn constrained_call(
        &mut self,
        method: Member,
        type_arguments: &[Type],
        receiver: &Expr,
        temporary: Option<u16>,
        args: &[Expr],
        base: usize,
    ) {
        let ty = self.constrained_receiver(receiver, temporary, base);
        let checkpoint = self.pending.len();
        self.keep_operand(StackSlot::Address(receiver.ty));
        for (index, arg) in args.iter().enumerate() {
            self.expr(arg, base + 1 + index);
            if arg.ty == Type::Never {
                self.pending.truncate(checkpoint);
                return;
            }
            self.keep_operand(StackSlot::of(arg));
        }
        self.pending.truncate(checkpoint);
        self.call_instance(method, type_arguments, Some(ty));
    }

    fn constrained_receiver(
        &mut self,
        receiver: &Expr,
        temporary: Option<u16>,
        base: usize,
    ) -> u32 {
        let ty = self.metadata.runtime_type_token(receiver.ty);
        self.max_stack = self.max_stack.max(base + 1);
        match (&receiver.kind, temporary) {
            (ExprKind::Load(value), None) => self.address(*value, base),
            (ExprKind::Load(Value::Local(index)), Some(temporary))
                if matches!(receiver.ty, Type::MethodParameter(_)) =>
            {
                // A value-type receiver needs the original address. A reference
                // receiver must be captured before the arguments can reassign it.
                self.indexed(0x12, 0x0d, temporary); // ldloca
                self.extended(0x15); // initobj
                self.code.extend_from_slice(&ty.to_le_bytes());
                self.indexed(0x11, 0x0c, temporary); // ldloc
                self.token_instruction(0x8c, ty); // box default(T)
                let is_value_type = self.branch(0x3a); // brtrue
                self.indexed(0x11, 0x0c, *index); // ldloc
                self.indexed(0x13, 0x0e, temporary); // stloc
                self.indexed(0x12, 0x0d, temporary); // ldloca
                let done = self.branch(0x38); // br
                self.patch(is_value_type);
                self.indexed(0x12, 0x0d, *index); // ldloca
                self.patch(done);
            }
            (_, Some(temporary)) => {
                self.expr(receiver, base);
                self.indexed(0x13, 0x0e, temporary); // stloc
                self.indexed(0x12, 0x0d, temporary); // ldloca
            }
            _ => unreachable!("checked constrained receiver"),
        }
        ty
    }

    fn array_access(&mut self, access: &ArrayAccess, base: usize) {
        self.expr(&access.array, base);
        if access.array.ty == Type::Never {
            return;
        }
        if let Some(slice) = access.slice {
            self.indexed(0x13, 0x0e, slice.temporary); // stloc
            self.indexed(0x12, 0x0d, slice.temporary); // ldloca
            self.pending_expr(&access.index, base, [StackSlot::Address(access.array.ty)]);
            if access.index.ty == Type::Never {
                return;
            }
            let indexer = self.metadata.slice_indexer(access.array.ty, slice.readonly);
            self.call(indexer);
            if let Some(value) = &access.value {
                self.pending_expr(value, base, [StackSlot::Address(access.element)]);
                if value.ty == Type::Never {
                    return;
                }
            }
            let element = self.metadata.runtime_type_token(access.element);
            self.token_instruction(if access.value.is_some() { 0x81 } else { 0x71 }, element); // stobj / ldobj
            return;
        }
        let array = StackSlot::of(&access.array);
        self.pending_expr(&access.index, base, [array]);
        if access.index.ty == Type::Never {
            return;
        }
        let store = access.value.is_some();
        if let Some(value) = &access.value {
            self.pending_expr(value, base, [array, StackSlot::Value(Type::I32)]);
            if value.ty == Type::Never {
                return;
            }
        }
        self.array_element(access.element, store);
    }

    fn captured_object(&mut self, slot: usize, base: usize) {
        self.max_stack = self.max_stack.max(base + 2);
        self.op(0x02); // ldarg.0
        self.load_i32(slot as i32);
        self.op(0x9a); // ldelem.ref
    }

    fn cell_element(&self, value: Value) -> Option<Type> {
        match value {
            Value::Local(index) if self.captured_cells.contains(&index) => {
                Some(self.locals[usize::from(index)])
            }
            Value::CapturedCell(_, element) => Some(element),
            _ => None,
        }
    }

    fn cell_index(&mut self, value: Value, element: Type, base: usize) {
        // A one-element array shares storage with captured delegates and can
        // provide a managed reference to the original value for CLR ref/out.
        self.max_stack = self.max_stack.max(base + 2);
        match value {
            Value::Local(index) => self.indexed(0x11, 0x0c, index), // ldloc
            Value::CapturedCell(slot, _) => {
                self.captured_object(slot, base);
                let token = self.metadata.cell_array_token(element);
                self.token_instruction(0x74, token); // castclass
            }
            _ => unreachable!("capture cell storage"),
        }
        self.load_i32(0);
    }

    fn address(&mut self, value: Value, base: usize) {
        self.max_stack = self.max_stack.max(base + 1);
        if let Some(element) = self.cell_element(value) {
            self.cell_index(value, element, base);
            let token = self.metadata.runtime_type_token(element);
            self.token_instruction(0x8f, token); // ldelema
        } else {
            match value {
                Value::Local(index) => self.indexed(0x12, 0x0d, index), // ldloca
                Value::Parameter(index) => {
                    if let Some(state) = &self.async_state {
                        self.indexed(0x12, 0x0d, state.parameter(index)); // ldloca
                    } else {
                        self.indexed(0x0f, 0x0a, index); // ldarga
                    }
                }
                _ => unreachable!("addressable binding"),
            }
        }
    }

    fn store(&mut self, target: Value, value: &Expr, base: usize) -> bool {
        let element = self.cell_element(target);
        if let Some(element) = element {
            self.cell_index(target, element, base);
        }
        if let Some(element) = element {
            self.pending_expr(
                value,
                base,
                [StackSlot::Vector(element), StackSlot::Value(Type::I32)],
            );
        } else {
            self.expr(value, base);
        }
        if value.ty == Type::Never {
            return false;
        }
        if let Some(element) = element {
            self.array_element(element, true);
        } else if let Value::Local(index) = target {
            self.indexed(0x13, 0x0e, index); // stloc
        } else {
            unreachable!("assignable binding");
        }
        true
    }

    fn array_element(&mut self, element: Type, store: bool) {
        if element.reference() {
            self.op(if store { 0xa2 } else { 0x9a }); // stelem.ref / ldelem.ref
        } else {
            let token = self.metadata.runtime_type_token(element);
            self.token_instruction(if store { 0xa4 } else { 0xa3 }, token); // stelem / ldelem
        }
    }

    fn index_update(&mut self, update: &IndexUpdate, base: usize) {
        let constrained = if update.receiver.ty.clr_struct()
            || matches!(update.receiver.ty, Type::MethodParameter(_))
        {
            Some(self.constrained_receiver(&update.receiver, update.temporary, base))
        } else {
            self.expr(&update.receiver, base);
            if update.receiver.ty == Type::Never {
                return;
            }
            None
        };
        let receiver = if constrained.is_some() {
            StackSlot::Address(update.receiver.ty)
        } else {
            StackSlot::of(&update.receiver)
        };
        let checkpoint = self.pending.len();
        self.keep_operand(receiver);
        self.keep_operand(receiver);
        // Retain the receiver (or managed address) for the setter while the
        // getter uses the duplicate. Save each index when it first executes.
        self.max_stack = self.max_stack.max(base + 2);
        self.op(0x25); // dup
        for (offset, (local, index)) in update.indices.iter().enumerate() {
            self.expr(index, base + 2 + offset);
            if index.ty == Type::Never {
                self.pending.truncate(checkpoint);
                return;
            }
            self.keep_operand(StackSlot::of(index));
            self.max_stack = self.max_stack.max(base + 4 + offset);
            self.op(0x25); // dup
            self.indexed(0x13, 0x0e, *local); // stloc
        }
        self.call_instance(update.getter, &[], constrained);
        self.pending.truncate(checkpoint + 1);
        self.indexed(0x13, 0x0e, update.previous); // stloc
        for (local, index) in &update.indices {
            self.indexed(0x11, 0x0c, *local); // ldloc
            self.keep_operand(StackSlot::of(index));
        }
        self.expr(&update.value, base + 1 + update.indices.len());
        self.pending.truncate(checkpoint);
        if update.value.ty != Type::Never {
            self.call_instance(update.setter, &[], constrained);
        }
    }

    fn call_instance(&mut self, member: Member, arguments: &[Type], constrained: Option<u32>) {
        let (_, row) = self.metadata.member(member.id, member.owner);
        let token = self
            .metadata
            .instantiate_method(MEMBER_REF | row, arguments);
        if constrained.is_some() && self.metadata.value_member(member.id) {
            self.token_instruction(0x28, token); // call with a value-type 'this' address
            return;
        }
        if let Some(ty) = constrained {
            self.extended(0x16); // constrained.
            self.code.extend_from_slice(&ty.to_le_bytes());
        }
        self.token_instruction(0x6f, token); // callvirt
    }

    fn not(&mut self) {
        self.op(0x16); // ldc.i4.0
        self.extended(0x01); // ceq
    }

    fn field(&mut self, opcode: u8, ty: Type, field: usize) {
        self.op(opcode);
        let token = self.metadata.field_token(ty, field);
        self.code.extend_from_slice(&token.to_le_bytes());
    }

    fn call(&mut self, token: u32) {
        self.token_instruction(0x28, token);
    }

    fn load_i32(&mut self, value: i32) {
        if let Ok(short) = i8::try_from(value) {
            self.code.extend_from_slice(&[0x1f, short as u8]); // ldc.i4.s
        } else {
            self.op(0x20); // ldc.i4
            self.code.extend_from_slice(&value.to_le_bytes());
        }
    }

    fn token_instruction(&mut self, opcode: u8, token: u32) {
        self.op(opcode);
        self.code.extend_from_slice(&token.to_le_bytes());
    }

    fn binary(&mut self, binary: Binary, operand: Type, base: usize) {
        use Binary::*;
        if operand == Type::String {
            match binary {
                Add => self.call(STRING_CONCAT),
                Eq | Ne => {
                    self.call(STRING_EQUALITY);
                    if matches!(binary, Ne) {
                        self.not();
                    }
                }
                _ => unreachable!("checked string operation"),
            }
            return;
        }
        if binary.shift() {
            // IL leaves oversized counts unspecified. Narrow types also share
            // the i32 stack, so mask using the source integer width.
            self.max_stack = self.max_stack.max(base + 3);
            self.load_i32(operand.shift_mask().expect("checked integer shift") as i32);
            self.op(0x5f); // and
        }
        match binary {
            Add => self.op(0x58),
            Sub => self.op(0x59),
            Mul => self.op(0x5a),
            Div => self.op(if operand.unsigned() { 0x5c } else { 0x5b }),
            Rem => self.op(if operand.unsigned() { 0x5e } else { 0x5d }),
            BitAnd => self.op(0x5f),
            BitOr => self.op(0x60),
            BitXor => self.op(0x61),
            Shl => self.op(0x62),
            Shr => self.op(if operand.unsigned() { 0x64 } else { 0x63 }),
            Eq | Ne => self.extended(0x01),                  // ceq
            Le if operand.floating() => self.extended(0x03), // cgt.un
            Ge if operand.floating() => self.extended(0x05), // clt.un
            Gt | Le if operand.unsigned() => self.extended(0x03), // cgt.un
            Lt | Ge if operand.unsigned() => self.extended(0x05), // clt.un
            Gt | Le => self.extended(0x02),                  // cgt
            Lt | Ge => self.extended(0x04),                  // clt
            And | Or | Coalesce => unreachable!("short-circuit operators lower as branches"),
        }
        if matches!(binary, Add | Sub | Mul | Div | Rem | Shl | Shr) {
            self.normalize_numeric(operand);
        } else if binary.bitwise() {
            self.normalize_numeric(self.metadata.integer_storage(operand));
        }
        if matches!(binary, Ne | Le | Ge) {
            self.not();
        }
    }

    fn numeric_cast(&mut self, source: Type, target: Type) {
        let source = self.metadata.integer_storage(source);
        let target = self.metadata.integer_storage(target);
        if source == target {
            return;
        }
        if target.floating() {
            if source.unsigned() {
                self.op(0x76); // conv.r.un
            }
            self.normalize_numeric(target);
            return;
        }
        let (signed, unsigned) = match target {
            Type::I8 => (0xb3, 0x82),
            Type::U8 => (0xb4, 0x86),
            Type::I16 => (0xb5, 0x83),
            Type::U16 | Type::Char => (0xb6, 0x87),
            Type::I32 => (0xb7, 0x84),
            Type::U32 => (0xb8, 0x88),
            Type::I64 => (0xb9, 0x85),
            Type::U64 => (0xba, 0x89),
            _ => unreachable!("checked integer cast"),
        };
        self.op(if source.unsigned() { unsigned } else { signed });
    }

    // Small integers use the CLR's int32 stack representation. Force nominal
    // widths at every operation so folds agree with execution.
    fn normalize_numeric(&mut self, ty: Type) {
        match ty {
            Type::I8 => self.op(0x67),  // conv.i1
            Type::I16 => self.op(0x68), // conv.i2
            Type::U8 => self.op(0xd2),  // conv.u1
            Type::U16 => self.op(0xd1), // conv.u2
            Type::F32 => self.op(0x6b), // conv.r4
            Type::F64 => self.op(0x6c), // conv.r8
            _ => {}
        }
    }
}
