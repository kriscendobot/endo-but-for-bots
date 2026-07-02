//! The `match`-dispatch interpreter over the stage-1 opcode subset
//! (design § Interpreter and dispatch): arithmetic, logic, bitwise,
//! comparison, unary, branch, and the stack/result opcodes. It
//! executes the exact bytecode the C-XS compiler emits (captured by the
//! oracle), so during stages 1 through 4 the byte stream is identical
//! and any computron divergence has one suspect: the interpreter.
//!
//! Semantics are ported case-by-case from `xs/sources/xsRun.c` at the
//! `c/moddable` pin: integer fast paths with checked-overflow promotion
//! to `f64`, XS's `-0` handling on multiply/modulo/minus, ToInt32 on
//! the bitwise ops, and NaN-aware relational and equality comparison.
//!
//! No JIT, no code generation, no execution-count-dependent behavior
//! (requirement 4): a straight `match` LLVM lowers to a jump table.

use crate::meter::{Meter, MeterCheck};
use crate::opcode::Opcode;
use crate::value::{number_to_ecma_string, to_int32, ChunkArena, Kind, Payload, Slot, SlotArena};

/// The fixed cost, in computrons, of the top-level program invocation
/// that precedes the captured program bytecode. C-XS dispatches the
/// program-as-function through its call machinery before the first
/// program opcode; those dispatches are metered but live in the caller
/// frame, not in the bytecode the oracle hands us. It is a constant of
/// the eval harness (identical on both engines), asserted for every
/// corpus entry by the differential harness.
pub const PROGRAM_INVOCATION_COMPUTRONS: u64 = 3;

/// Why a run stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Halt {
    /// Reached RETURN/END: the completion value is in `result`.
    Return,
    /// The meter host refused more computation.
    MeterAbort,
    /// An opcode outside the stage-1 subset was reached. Carries the
    /// mnemonic so the harness can report exactly what to implement
    /// next, rather than papering over it.
    Unsupported(&'static str),
    /// The bytecode was truncated or an opcode byte was invalid.
    Decode(String),
    /// A JS-level throw (only the shapes stage 1 models, e.g. an
    /// explicit `throw`); carries a best-effort string.
    Throw(String),
}

/// The result of running one program's bytecode on endor-vm.
#[derive(Debug, Clone)]
pub struct RunOutcome {
    /// `true` if the program completed normally.
    pub completed: bool,
    /// Completion value rendered with ECMAScript `String()` semantics
    /// (valid when `completed`).
    pub result: String,
    /// Computrons, comparable bit-for-bit with the oracle's run-only
    /// count: dispatched opcodes plus the invocation baseline.
    pub computrons: u64,
    /// Raw dispatched-opcode count, before the invocation baseline
    /// (useful for isolating a metering divergence).
    pub dispatched: u64,
    /// Why the run stopped.
    pub halt: Halt,
}

/// One interpreter activation. Stage 1 runs a single top-level program
/// frame; the slot/chunk arenas are the machine heap (exercised by
/// string and, later, reference values), and the value stack is XS's
/// downward-growing slot stack modeled as a `Vec` whose top is the last
/// element.
pub struct Interp {
    stack: Vec<Slot>,
    result: Slot,
    meter: Meter,
    /// The host metering callback, installed by [`Interp::arm_meter`].
    /// `None` is the default un-metered interpreter the differential
    /// harness uses: the check points then never consult a host and
    /// never abort. When `Some`, each loop-closing check point passes
    /// the current computron count to it and halts with
    /// [`Halt::MeterAbort`] on refusal.
    meter_host: Option<Box<dyn FnMut(u64) -> bool>>,
    /// The machine slot heap (design § Value and heap model).
    pub slots: SlotArena,
    /// The machine chunk heap (CESU-8 strings and later data).
    pub chunks: ChunkArena,
}

impl Default for Interp {
    fn default() -> Self {
        Interp::new()
    }
}

impl Interp {
    pub fn new() -> Interp {
        Interp {
            stack: Vec::with_capacity(64),
            result: Slot::undefined(),
            meter: Meter::new(),
            meter_host: None,
            slots: SlotArena::new(),
            chunks: ChunkArena::new(),
        }
    }

    /// Arm metering (`fxBeginMetering`): install a check `interval` and
    /// the `host` callback the loop-closing check points consult with
    /// `meterIndex >> 16` ("computrons"). This does **not** change the
    /// index accumulation or the un-metered default — a fresh `Interp`
    /// never checks — so the differential harness (which never arms) is
    /// unaffected. On host refusal, the run halts with
    /// [`Halt::MeterAbort`].
    pub fn arm_meter(&mut self, interval: u64, host: Box<dyn FnMut(u64) -> bool>) {
        self.meter.begin(interval);
        self.meter_host = Some(host);
    }

    /// A loop-closing metering check (`mxCheckMeter`). Consults the host
    /// only when metering is armed; otherwise (the default) it is a
    /// no-op that keeps running. Adds nothing to `meterIndex`.
    #[inline]
    fn check_meter(&mut self) -> MeterCheck {
        match self.meter_host.as_mut() {
            Some(host) => self.meter.check(host),
            None => MeterCheck::Continue,
        }
    }

    #[inline]
    fn push(&mut self, s: Slot) {
        self.stack.push(s);
    }
    #[inline]
    fn pop(&mut self) -> Slot {
        self.stack.pop().unwrap_or_else(Slot::undefined)
    }

    /// Run a program bytecode buffer to completion.
    pub fn run(&mut self, code: &[u8]) -> RunOutcome {
        let halt = self.dispatch(code);
        let completed = halt == Halt::Return;
        let result = if completed {
            slot_to_ecma_string(&self.result)
        } else {
            String::new()
        };
        RunOutcome {
            completed,
            result,
            computrons: self.meter.computrons() + PROGRAM_INVOCATION_COMPUTRONS,
            dispatched: self.meter.computrons(),
            halt,
        }
    }

    fn dispatch(&mut self, code: &[u8]) -> Halt {
        let len = code.len();
        let mut pc: usize = 0;

        // Operand readers (little-endian; XS mxRunS1/S2/S4 on our LE
        // target). `off` is relative to `pc`.
        macro_rules! s1 {
            ($off:expr) => {
                code[pc + $off] as i8 as i32
            };
        }

        loop {
            if pc >= len {
                return Halt::Decode(format!("pc {} past end {}", pc, len));
            }
            let byte = code[pc];
            let op = match Opcode::from_u8(byte) {
                Some(o) => o,
                None => return Halt::Decode(format!("invalid opcode byte {:#04x} at {}", byte, pc)),
            };
            // Every dispatched opcode meters one code unit (mxBreak /
            // the switch-path `meterIndex += XS_CODE_METERING`).
            self.meter.tick_code();

            let size = op.size();
            // Bounds-check the fixed-size operands before reading.
            if size > 0 && pc + (size as usize) > len {
                return Halt::Decode(format!(
                    "opcode {} at {} needs {} bytes, {} left",
                    op.name(),
                    pc,
                    size,
                    len - pc
                ));
            }

            use Opcode::*;
            match op {
                // ---- program prologue / frame -----------------------
                XS_CODE_BEGIN_SLOPPY | XS_CODE_BEGIN_STRICT => {
                    // `this` / strictness setup: no effect on the pure
                    // expression subset. Operand byte is the frame's
                    // scope count.
                    pc += size as usize;
                }
                XS_CODE_EVAL_ENVIRONMENT => {
                    pc += size as usize;
                }

                // ---- literals ---------------------------------------
                XS_CODE_INTEGER_1 => {
                    self.push(Slot::integer(s1!(1)));
                    pc += size as usize;
                }
                XS_CODE_INTEGER_2 => {
                    let v = i16::from_le_bytes([code[pc + 1], code[pc + 2]]) as i32;
                    self.push(Slot::integer(v));
                    pc += size as usize;
                }
                XS_CODE_INTEGER_4 => {
                    let v = i32::from_le_bytes([
                        code[pc + 1],
                        code[pc + 2],
                        code[pc + 3],
                        code[pc + 4],
                    ]);
                    self.push(Slot::integer(v));
                    pc += size as usize;
                }
                XS_CODE_NUMBER => {
                    let mut b = [0u8; 8];
                    b.copy_from_slice(&code[pc + 1..pc + 9]);
                    self.push(Slot::number(f64::from_le_bytes(b)));
                    pc += size as usize;
                }
                XS_CODE_TRUE => {
                    self.push(Slot::boolean(true));
                    pc += size as usize;
                }
                XS_CODE_FALSE => {
                    self.push(Slot::boolean(false));
                    pc += size as usize;
                }
                XS_CODE_NULL => {
                    self.push(Slot::null());
                    pc += size as usize;
                }
                XS_CODE_UNDEFINED => {
                    self.push(Slot::undefined());
                    pc += size as usize;
                }

                // ---- arithmetic -------------------------------------
                XS_CODE_ADD => {
                    self.binary_arith(ArithOp::Add);
                    pc += size as usize;
                }
                XS_CODE_SUBTRACT => {
                    self.binary_arith(ArithOp::Sub);
                    pc += size as usize;
                }
                XS_CODE_MULTIPLY => {
                    self.binary_arith(ArithOp::Mul);
                    pc += size as usize;
                }
                XS_CODE_DIVIDE => {
                    self.binary_arith(ArithOp::Div);
                    pc += size as usize;
                }
                XS_CODE_MODULO => {
                    self.binary_arith(ArithOp::Mod);
                    pc += size as usize;
                }

                // ---- bitwise ----------------------------------------
                XS_CODE_BIT_AND => {
                    self.binary_bit(BitOp::And);
                    pc += size as usize;
                }
                XS_CODE_BIT_OR => {
                    self.binary_bit(BitOp::Or);
                    pc += size as usize;
                }
                XS_CODE_BIT_XOR => {
                    self.binary_bit(BitOp::Xor);
                    pc += size as usize;
                }
                XS_CODE_LEFT_SHIFT => {
                    self.binary_bit(BitOp::Shl);
                    pc += size as usize;
                }
                XS_CODE_SIGNED_RIGHT_SHIFT => {
                    self.binary_bit(BitOp::Sar);
                    pc += size as usize;
                }
                XS_CODE_UNSIGNED_RIGHT_SHIFT => {
                    self.binary_bit(BitOp::Shr);
                    pc += size as usize;
                }
                XS_CODE_BIT_NOT => {
                    let a = self.pop();
                    self.push(Slot::integer(!to_int32(to_number(&a))));
                    pc += size as usize;
                }

                // ---- comparison -------------------------------------
                XS_CODE_LESS => {
                    self.relational(RelOp::Less);
                    pc += size as usize;
                }
                XS_CODE_LESS_EQUAL => {
                    self.relational(RelOp::LessEqual);
                    pc += size as usize;
                }
                XS_CODE_MORE => {
                    self.relational(RelOp::More);
                    pc += size as usize;
                }
                XS_CODE_MORE_EQUAL => {
                    self.relational(RelOp::MoreEqual);
                    pc += size as usize;
                }
                XS_CODE_STRICT_EQUAL => {
                    self.equality(true, false);
                    pc += size as usize;
                }
                XS_CODE_STRICT_NOT_EQUAL => {
                    self.equality(true, true);
                    pc += size as usize;
                }
                XS_CODE_EQUAL => {
                    self.equality(false, false);
                    pc += size as usize;
                }
                XS_CODE_NOT_EQUAL => {
                    self.equality(false, true);
                    pc += size as usize;
                }

                // ---- unary ------------------------------------------
                XS_CODE_MINUS => {
                    let a = self.pop();
                    self.push(unary_minus(&a));
                    pc += size as usize;
                }
                XS_CODE_PLUS => {
                    let a = self.pop();
                    // ToNumber; an integer stays an integer.
                    match a.kind {
                        Kind::Integer => self.push(a),
                        _ => self.push(Slot::number(to_number(&a))),
                    }
                    pc += size as usize;
                }
                XS_CODE_NOT => {
                    let a = self.pop();
                    self.push(Slot::boolean(!to_boolean(&a)));
                    pc += size as usize;
                }
                XS_CODE_VOID => {
                    let _ = self.pop();
                    self.push(Slot::undefined());
                    pc += size as usize;
                }

                // ---- stack ------------------------------------------
                XS_CODE_DUB => {
                    let top = self.stack.last().copied().unwrap_or_else(Slot::undefined);
                    self.push(top);
                    pc += size as usize;
                }
                XS_CODE_POP => {
                    let _ = self.pop();
                    pc += size as usize;
                }

                // ---- branches ---------------------------------------
                // mxBranch: target = pc + INDEX(size) + OFFSET(operand).
                // C-XS runs `mxCheckMeter` only when the taken offset is
                // negative (a backward branch — the loop-closing point);
                // an armed host refusal aborts with `Halt::MeterAbort`.
                XS_CODE_BRANCH_1 => {
                    let off = s1!(1);
                    if off < 0 && self.check_meter() == MeterCheck::Abort {
                        return Halt::MeterAbort;
                    }
                    pc = branch_target(pc, size, off);
                }
                XS_CODE_BRANCH_2 => {
                    let off = i16::from_le_bytes([code[pc + 1], code[pc + 2]]) as i32;
                    if off < 0 && self.check_meter() == MeterCheck::Abort {
                        return Halt::MeterAbort;
                    }
                    pc = branch_target(pc, size, off);
                }
                XS_CODE_BRANCH_4 => {
                    let off = i32::from_le_bytes([
                        code[pc + 1],
                        code[pc + 2],
                        code[pc + 3],
                        code[pc + 4],
                    ]);
                    if off < 0 && self.check_meter() == MeterCheck::Abort {
                        return Halt::MeterAbort;
                    }
                    pc = branch_target(pc, size, off);
                }
                // mxBranchElse: the fall-through (cond true) takes INDEX
                // with no check; only the branch-taken (cond false) path
                // is an `mxBranch`, so it checks when its offset < 0.
                XS_CODE_BRANCH_ELSE_1 => {
                    let off = s1!(1);
                    let cond = to_boolean(&self.pop());
                    if cond {
                        pc += size as usize;
                    } else {
                        if off < 0 && self.check_meter() == MeterCheck::Abort {
                            return Halt::MeterAbort;
                        }
                        pc = branch_target(pc, size, off);
                    }
                }
                XS_CODE_BRANCH_ELSE_2 => {
                    let off = i16::from_le_bytes([code[pc + 1], code[pc + 2]]) as i32;
                    let cond = to_boolean(&self.pop());
                    if cond {
                        pc += size as usize;
                    } else {
                        if off < 0 && self.check_meter() == MeterCheck::Abort {
                            return Halt::MeterAbort;
                        }
                        pc = branch_target(pc, size, off);
                    }
                }
                // mxBranchIf: the branch-taken (cond true) path is the
                // `mxBranch`, so it checks when its offset < 0; the
                // fall-through takes INDEX with no check.
                XS_CODE_BRANCH_IF_1 => {
                    let off = s1!(1);
                    let cond = to_boolean(&self.pop());
                    if cond {
                        if off < 0 && self.check_meter() == MeterCheck::Abort {
                            return Halt::MeterAbort;
                        }
                        pc = branch_target(pc, size, off);
                    } else {
                        pc += size as usize;
                    }
                }
                XS_CODE_BRANCH_IF_2 => {
                    let off = i16::from_le_bytes([code[pc + 1], code[pc + 2]]) as i32;
                    let cond = to_boolean(&self.pop());
                    if cond {
                        if off < 0 && self.check_meter() == MeterCheck::Abort {
                            return Halt::MeterAbort;
                        }
                        pc = branch_target(pc, size, off);
                    } else {
                        pc += size as usize;
                    }
                }

                // ---- result / return --------------------------------
                XS_CODE_SET_RESULT => {
                    self.result = self.pop();
                    pc += size as usize;
                }
                XS_CODE_GET_RESULT => {
                    let r = self.result;
                    self.push(r);
                    pc += size as usize;
                }
                XS_CODE_RETURN | XS_CODE_END => {
                    // C-XS checks the meter at returns (the frame-pop
                    // reaches its caller's `mxFirstCode`/`mxCheckMeter`).
                    if self.check_meter() == MeterCheck::Abort {
                        return Halt::MeterAbort;
                    }
                    return Halt::Return;
                }

                // ---- explicit throw (stage-1 shape) -----------------
                XS_CODE_THROW => {
                    let v = self.pop();
                    return Halt::Throw(slot_to_ecma_string(&v));
                }

                other => {
                    return Halt::Unsupported(other.name());
                }
            }
        }
    }

    // Binary numeric arithmetic, ported from the xsRun.c integer fast
    // paths with checked-overflow promotion to f64.
    fn binary_arith(&mut self, op: ArithOp) {
        let b = self.pop();
        let a = self.pop();
        self.push(apply_arith(op, &a, &b));
    }

    fn binary_bit(&mut self, op: BitOp) {
        let b = self.pop();
        let a = self.pop();
        let ai = to_int32(to_number(&a));
        let bi = to_int32(to_number(&b));
        let r = match op {
            BitOp::And => ai & bi,
            BitOp::Or => ai | bi,
            BitOp::Xor => ai ^ bi,
            BitOp::Shl => ((ai as u32) << (bi & 0x1f)) as i32,
            BitOp::Sar => ai >> (bi & 0x1f),
            BitOp::Shr => ((ai as u32) >> (bi & 0x1f)) as i32,
        };
        // Unsigned shift can exceed i32 range; XS keeps it a number
        // when the high bit is set.
        if let BitOp::Shr = op {
            let u = (ai as u32) >> (bi & 0x1f);
            if u > i32::MAX as u32 {
                self.push(Slot::number(u as f64));
                return;
            }
        }
        self.push(Slot::integer(r));
    }

    fn relational(&mut self, op: RelOp) {
        let b = self.pop();
        let a = self.pop();
        let x = to_number(&a);
        let y = to_number(&b);
        let r = if x.is_nan() || y.is_nan() {
            false
        } else {
            match op {
                RelOp::Less => x < y,
                RelOp::LessEqual => x <= y,
                RelOp::More => x > y,
                RelOp::MoreEqual => x >= y,
            }
        };
        self.push(Slot::boolean(r));
    }

    fn equality(&mut self, strict: bool, negate: bool) {
        let b = self.pop();
        let a = self.pop();
        let eq = if strict {
            strict_equals(&a, &b)
        } else {
            loose_equals(&a, &b)
        };
        self.push(Slot::boolean(eq ^ negate));
    }
}

#[derive(Copy, Clone)]
enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}
#[derive(Copy, Clone)]
enum BitOp {
    And,
    Or,
    Xor,
    Shl,
    Sar,
    Shr,
}
#[derive(Copy, Clone)]
enum RelOp {
    Less,
    LessEqual,
    More,
    MoreEqual,
}

#[inline]
fn branch_target(pc: usize, size: i8, offset: i32) -> usize {
    // pc + INDEX(size) + OFFSET, in XS's signed arithmetic.
    (pc as isize + size as isize + offset as isize) as usize
}

// ToBoolean (ECMAScript 7.1.2) for the stage-1 value kinds.
fn to_boolean(s: &Slot) -> bool {
    match s.value {
        Payload::None => false, // undefined and null are both falsy
        Payload::Boolean(b) => b,
        Payload::Integer(i) => i != 0,
        Payload::Number(n) => !(n == 0.0 || n.is_nan()),
        Payload::String(_) => true, // non-empty; stage-1 strings are results only
        Payload::Reference(_) => true,
    }
}

// ToNumber (ECMAScript 7.1.4) for stage-1 value kinds.
fn to_number(s: &Slot) -> f64 {
    match s.value {
        Payload::None => match s.kind {
            Kind::Null => 0.0,
            _ => f64::NAN, // undefined
        },
        Payload::Boolean(b) => {
            if b {
                1.0
            } else {
                0.0
            }
        }
        Payload::Integer(i) => i as f64,
        Payload::Number(n) => n,
        _ => f64::NAN,
    }
}

// XS_CODE_ADD/SUBTRACT/MULTIPLY/DIVIDE/MODULO integer fast paths.
fn apply_arith(op: ArithOp, a: &Slot, b: &Slot) -> Slot {
    if let (Payload::Integer(x), Payload::Integer(y)) = (a.value, b.value) {
        match op {
            ArithOp::Add => match x.checked_add(y) {
                Some(v) => return Slot::integer(v),
                None => return Slot::number(x as f64 + y as f64),
            },
            ArithOp::Sub => match x.checked_sub(y) {
                Some(v) => return Slot::integer(v),
                None => return Slot::number(x as f64 - y as f64),
            },
            ArithOp::Mul => {
                // XS mxMinusZero: 0 * negative and negative * 0 -> -0.
                if x == 0 {
                    if y < 0 {
                        return Slot::number(-0.0);
                    }
                    return Slot::integer(0);
                }
                if y == 0 {
                    if x < 0 {
                        return Slot::number(-0.0);
                    }
                    return Slot::integer(0);
                }
                match x.checked_mul(y) {
                    Some(v) => return Slot::integer(v),
                    None => return Slot::number(x as f64 * y as f64),
                }
            }
            ArithOp::Div => {
                // JS `/` is always floating; XS produces a number.
                return Slot::number(x as f64 / y as f64);
            }
            ArithOp::Mod => {
                if y == 0 {
                    return Slot::number(f64::NAN);
                }
                if x < 0 {
                    let r = x.wrapping_rem(y);
                    if r == 0 {
                        return Slot::number(-0.0);
                    }
                    return Slot::integer(r);
                }
                return Slot::integer(x.wrapping_rem(y));
            }
        }
    }
    // At least one operand is a number: XS does f64 arithmetic.
    let x = to_number(a);
    let y = to_number(b);
    let r = match op {
        ArithOp::Add => x + y,
        ArithOp::Sub => x - y,
        ArithOp::Mul => x * y,
        ArithOp::Div => x / y,
        ArithOp::Mod => x % y, // Rust f64 % is C fmod semantics
    };
    Slot::number(r)
}

// XS_CODE_MINUS: negate, with -0 and INT_MIN promotion to number.
fn unary_minus(a: &Slot) -> Slot {
    match a.value {
        Payload::Integer(i) => {
            // XS: `if (integer & 0x7FFFFFFF)` negate as integer, else
            // promote (covers 0 -> -0.0 and INT_MIN).
            if (i & 0x7FFF_FFFFu32 as i32) != 0 {
                Slot::integer(i.wrapping_neg())
            } else {
                Slot::number(-(i as f64))
            }
        }
        Payload::Number(n) => Slot::number(-n),
        _ => Slot::number(-to_number(a)),
    }
}

// Strict equality (===) for stage-1 value kinds.
fn strict_equals(a: &Slot, b: &Slot) -> bool {
    match (a.value, b.value) {
        (Payload::None, Payload::None) => a.kind == b.kind, // undefined===undefined, null===null
        (Payload::Boolean(x), Payload::Boolean(y)) => x == y,
        (Payload::Integer(x), Payload::Integer(y)) => x == y,
        (Payload::Number(x), Payload::Number(y)) => x == y, // NaN handled by IEEE
        (Payload::Integer(x), Payload::Number(y)) => (x as f64) == y,
        (Payload::Number(x), Payload::Integer(y)) => x == (y as f64),
        _ => false,
    }
}

// Loose equality (==) for stage-1 value kinds (number/int/bool/null/
// undefined). String and reference coercions arrive with the object
// model.
fn loose_equals(a: &Slot, b: &Slot) -> bool {
    match (a.kind, b.kind) {
        (Kind::Undefined | Kind::Null, Kind::Undefined | Kind::Null) => true,
        _ => {
            // Numeric coercion for number/int/boolean.
            let numeric = |s: &Slot| {
                matches!(
                    s.kind,
                    Kind::Integer | Kind::Number | Kind::Boolean
                )
            };
            if numeric(a) && numeric(b) {
                let x = to_number(a);
                let y = to_number(b);
                !x.is_nan() && !y.is_nan() && x == y
            } else {
                strict_equals(a, b)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opcode::Opcode;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn b(op: Opcode) -> u8 {
        op as u8
    }

    #[test]
    fn armed_meter_aborts_at_threshold() {
        // A tight infinite backward-`BRANCH_1` self-loop: `BRANCH_1 -2`
        // jumps to itself (target = pc + size(2) + (-2) = pc). It only
        // terminates because the armed meter refuses more computation.
        let code = [b(Opcode::XS_CODE_BRANCH_1), 0xFE]; // -2

        // Record every computron value the host is shown; refuse at 5.
        let seen = Rc::new(RefCell::new(Vec::new()));
        let seen_cb = Rc::clone(&seen);
        let mut interp = Interp::new();
        // interval 1 (raw fixed-point): the check fires every iteration,
        // so the host sees computrons 1, 2, 3, ... one per dispatch.
        interp.arm_meter(
            1,
            Box::new(move |computrons| {
                seen_cb.borrow_mut().push(computrons);
                computrons < 5
            }),
        );
        let out = interp.run(&code);

        assert_eq!(out.halt, Halt::MeterAbort, "armed meter must abort the loop");
        assert!(!out.completed);
        // Aborted on the 5th backward branch: 5 dispatched opcodes.
        assert_eq!(out.dispatched, 5, "abort at the expected computron threshold");
        assert_eq!(out.computrons, 5 + PROGRAM_INVOCATION_COMPUTRONS);
        assert_eq!(
            *seen.borrow(),
            vec![1, 2, 3, 4, 5],
            "host consulted once per backward branch until refusal"
        );
    }

    #[test]
    fn unarmed_meter_accumulates_without_checking() {
        // A finite path that still exercises a backward branch, so we can
        // observe the index accumulating with no check on the default
        // (un-armed) interpreter the differential harness uses:
        //   pc0: BRANCH_1 +1  -> forward to pc3   (offset >= 0, never checks)
        //   pc2: END                              (halt)
        //   pc3: BRANCH_1 -3  -> backward to pc2  (offset < 0, would check)
        let code = [
            b(Opcode::XS_CODE_BRANCH_1),
            0x01, // +1 -> pc3
            b(Opcode::XS_CODE_END),
            b(Opcode::XS_CODE_BRANCH_1),
            0xFD, // -3 -> pc2 (END)
        ];

        let out = Interp::new().run(&code);
        assert_eq!(out.halt, Halt::Return, "un-armed meter never aborts");
        assert!(out.completed);
        // Three dispatched opcodes: the forward branch, the backward
        // branch, and END — the index accumulated, no host was consulted.
        assert_eq!(out.dispatched, 3, "meter accumulates without checking");
    }

    #[test]
    fn armed_but_permissive_meter_runs_to_return() {
        // Same finite path, armed with a host that always allows more:
        // the backward-branch and END check points fire but never abort.
        let code = [
            b(Opcode::XS_CODE_BRANCH_1),
            0x01,
            b(Opcode::XS_CODE_END),
            b(Opcode::XS_CODE_BRANCH_1),
            0xFD,
        ];
        let mut interp = Interp::new();
        interp.arm_meter(1, Box::new(|_| true));
        let out = interp.run(&code);
        assert_eq!(out.halt, Halt::Return);
        assert_eq!(out.dispatched, 3);
    }
}

/// Render a completion value the way JS `String()` does.
pub fn slot_to_ecma_string(s: &Slot) -> String {
    match s.value {
        Payload::None => match s.kind {
            Kind::Null => "null".to_string(),
            _ => "undefined".to_string(),
        },
        Payload::Boolean(b) => if b { "true" } else { "false" }.to_string(),
        Payload::Integer(i) => i.to_string(),
        Payload::Number(n) => number_to_ecma_string(n),
        Payload::String(_) => String::new(), // stage-1 strings not produced
        Payload::Reference(_) => "[object Object]".to_string(),
    }
}
