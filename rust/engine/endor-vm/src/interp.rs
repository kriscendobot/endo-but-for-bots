//! The `match`-dispatch interpreter (design § Interpreter and
//! dispatch). It executes the exact bytecode the C-XS compiler emits
//! (captured by the oracle), so during stages 1 through 4 the byte
//! stream is identical and any divergence has one suspect: the
//! interpreter.
//!
//! Stage 1 covered the pure-expression subset (arithmetic, logic,
//! bitwise, comparison, unary, branch, stack/result). Stage 2 adds the
//! program frame and its scope: `RESERVE`/`NEW_LOCAL`/`NEW_TEMPORARY`
//! scope slots, the `*_LOCAL` accessors, the environment/variable
//! opcodes (`EVAL_ENVIRONMENT`/`EVAL_REFERENCE`/`GET_VARIABLE`/
//! `SET_VARIABLE`), and backward-branch control flow (loops) over
//! compiler-emitted bytecode — the call/frame machinery's scope half.
//!
//! Semantics are ported case-by-case from `xs/sources/xsRun.c` at the
//! `c/moddable` pin: integer fast paths with checked-overflow promotion
//! to `f64`, XS's `-0` handling on multiply/modulo/minus, ToInt32 on
//! the bitwise ops, NaN-aware comparison, and the scope-slot addressing
//! `mxEnvironment - index`.
//!
//! **Allocation-faithful metering (stage 2b).** Computrons are bit-exact
//! for programs that allocate at run time, not only for pure
//! expressions. The meter accrues, in raw 16.16 units so they compose
//! through the carry into computrons:
//!  - **per dispatched opcode** `XS_CODE_METERING` (1<<16), as `mxBreak`;
//!  - **the program overhead** once at `BEGIN_*`: the invocation baseline
//!    ([`PROGRAM_INVOCATION_COMPUTRONS`] dispatches in the caller frame)
//!    plus the eval-environment setup aggregate
//!    ([`PROGRAM_ENV_SETUP_METERING`], measured against the pin);
//!  - **per slot allocation** `XS_SLOT_ALLOCATION_METERING` (1<<8) where
//!    `fxNewSlot` runs — a top-level `var` hoisted onto the global object
//!    at `EVAL_ENVIRONMENT`, or a sloppy global created at
//!    `SET_VARIABLE`;
//!  - **per property store** one built-in step
//!    `XS_BUILTIN_METERING` (1<<14, `mxMeterOne`) at `SET_VARIABLE`.
//! The upshot is the "16920 per var" the 2a differential probe measured
//! is now *reproduced* (536 allocation + 16384 store), and the stage-2
//! var/loop corpus graduates into the bit-exact bar. `GET_VARIABLE`
//! reads meter no built-in step, matching the oracle.
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

/// The raw 16.16-fixed-point aggregate C-XS accrues building the
/// program's environment instance and frame during program entry
/// (`fxRunEvalEnvironment` and the frame setup: a bundle of `fxNewSlot`
/// allocations for the program environment). Measured against the pin
/// `48ee02d8cfe0`: every top-level program — even `1` — carries exactly
/// this fractional remainder (`meterIndex & 0xFFFF == 17688` on a
/// pure-expression program, verified via the oracle's raw meter). It is
/// under one computron (< 1<<16), so the stage-1 pure-expression corpus
/// never observes a carry from it (which is why stage 1 was bit-exact
/// without modeling it); a program that also allocates at run time
/// (§ Allocation-faithful metering) accrues on top of it and *does*
/// carry, which is the stage-2b crux. Accrued once, at the `BEGIN_*`
/// program-frame-entry opcode.
pub const PROGRAM_ENV_SETUP_METERING: u64 = 17688;

/// The raw 16.16 cost C-XS accrues materializing one new own property on
/// an object (`mxBehaviorSetProperty`/`fxRunDefine` creating a property:
/// `fxNewSlot` for the property slot plus the property-table growth and
/// interned-key `fxNewSlot`/`fxNewChunk`). Measured against the pin as
/// 536 = one modeled property-slot allocation
/// ([`crate::meter::SLOT_ALLOCATION_METERING`], 1<<8 = 256) plus this
/// [`PROPERTY_CREATE_REMAINDER`]. Accrued wherever a new own property is
/// created — a hoisted `var` or sloppy global at `EVAL_ENVIRONMENT` /
/// `SET_VARIABLE`, an object-literal member at `NEW_PROPERTY`, or a
/// dynamic assignment at `SET_PROPERTY`. Verified per-site against the
/// oracle's raw meter (a `SET_PROPERTY` that creates costs exactly 536;
/// one that overwrites costs nothing; a `NEW_PROPERTY` costs 536 plus one
/// built-in step for `fxRunDefine`).
pub const PROPERTY_CREATE_REMAINDER: u64 = 280;

/// The raw 16.16 cost C-XS accrues in `constructor_function`
/// (`fxNewFunctionInstance` + `fxDefaultFunctionPrototype`): the function
/// instance and its internal CODE/HOME slots, its `length`/`name`
/// properties, and the default `.prototype` object with its `constructor`
/// back-reference — a fixed cluster of `fxNewSlot` allocations plus the
/// built-in steps `fxDefaultFunctionPrototype` runs, independent of the
/// function's body or arity (the body chunk is metered separately at
/// `code`, and the arity-dependent scope slots at the body's
/// `new_local`s). Measured against the pin `48ee02d8cfe0`: a bare
/// `(function(){})()` — whose only unmodeled cost is this cluster plus the
/// 5-byte body chunk — carries exactly [`FUNCTION_DEFINE_METERING`] + 5
/// beyond the program baseline, the per-opcode dispatch metering, and the
/// modeled `new_property`. The nested `(function(){return
/// (function(){return 1})()})()` carries exactly twice this (its raw gap
/// with the constant zeroed was 135670 = 2 × 67835), confirming it as a
/// clean per-definition constant. Verified per-site against the oracle's
/// raw meter. Plain `XS_CODE_FUNCTION` (no default prototype) is a
/// distinct, smaller cluster carried by a later increment. (The
/// body-chunk allocation is *not* part of this constant — it is metered
/// faithfully at `code` via [`crate::meter::Meter::tick_chunk_new`], so a
/// function's arity/body length moves its computrons the way C-XS's does.)
pub const FUNCTION_DEFINE_METERING: u64 = 67816;

/// The raw 16.16 cost C-XS accrues in `function_environment`
/// (`fxNewEnvironmentInstance`): the closure environment instance the
/// function captures its defining scope through. Accrued once per
/// `function_environment` opcode, at the definition site. Measured
/// against the pin; verified per-site.
pub const FUNCTION_ENVIRONMENT_METERING: u64 = 0;

/// Body-scope allocation metering per declared parameter/local inside a
/// function frame. Each declared parameter/local (`new_local`) of a
/// function carries a fixed definition-time allocation cost — a scope-cell
/// `fxNewSlot` (256) plus a small aligned chunk (24 = the `fxNewChunk`
/// header/alignment of a ≤8-byte block), 280 raw total, measured against
/// the pin. It is a **definition-time** cost (present even when the
/// function is never called and constant across calls/recursion depth, so
/// it does not accumulate per invocation), accrued at `code` in proportion
/// to the count of `new_local` opcodes in the function's immediate body.
/// (A residual ≤8 raw per definition from body-chunk alignment on some
/// arities stays below one computron and does not perturb the bit-exact
/// bar.)
pub const FUNCTION_LOCAL_METERING: u64 = 280;

/// Metadata for a user function instance created by
/// `constructor_function`/`function`: the byte range of its body in the
/// program's code buffer (set by the following `code` opcode) and the
/// closure environment it captured (set by `function_environment`). Kept
/// in a side table keyed by the function's slot index so the function
/// object stays a real arena instance whose own properties (`.prototype`,
/// `.length`, `.name`, and user-defined) are real arena slots the GC
/// traces, while the non-value-slot body/closure metadata rides alongside.
#[derive(Clone, Debug)]
struct FuncInfo {
    /// Start offset of the function body in the program code buffer (the
    /// byte just past the `code` opcode's operand — where `begin_*` sits).
    body_start: usize,
    /// Length of the body chunk (the `code` opcode's operand).
    body_len: usize,
    /// The captured closure environment (a frame-cell owner), or `NULL`
    /// until `function_environment` runs / for a non-capturing function.
    closures: crate::value::SlotIndex,
}

impl Default for FuncInfo {
    fn default() -> Self {
        FuncInfo {
            body_start: 0,
            body_len: 0,
            closures: crate::value::SlotIndex::NULL,
        }
    }
}

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
    /// Raw 16.16 fixed-point meter index (`the->meterIndex`), for
    /// diagnosing fractional (allocation/built-in) metering during
    /// calibration.
    pub meter_raw: u64,
    /// Why the run stopped.
    pub halt: Halt,
}

/// One interpreter activation over a single top-level program frame
/// (design § Interpreter and dispatch). The value stack is XS's
/// downward-growing slot stack modeled as a `Vec` whose top is the last
/// element; the program frame carries its scope slots (`locals`,
/// declared by `NEW_LOCAL`/`NEW_TEMPORARY` and addressed by the
/// `*_LOCAL` opcodes' index), the `id -> local` map the environment
/// opcodes resolve `var` names through, the completion value
/// (`result`), and the global bindings undeclared names fall back to.
///
/// Metering is per dispatched bytecode (§ Metering): the frame and
/// control-flow opcodes each dispatch once, so running the exact C-XS
/// bytecode yields the exact C-XS computron count without any separate
/// per-opcode weight bookkeeping. The program-invocation baseline
/// ([`PROGRAM_INVOCATION_COMPUTRONS`]) accounts for the program-frame
/// entry C-XS meters outside the captured bytecode.
///
/// Call/return frame *switching* (nested user functions) and the object
/// model are the next stage-2 work items; opcodes outside the
/// frame/scope/variable/control-flow/expression subset halt with
/// [`Halt::Unsupported`] naming themselves, so the differential harness
/// reports exactly what to implement next rather than diverging
/// silently.
pub struct Interp {
    stack: Vec<Slot>,
    /// The program frame's scope slots. `NEW_LOCAL`/`NEW_TEMPORARY`
    /// append (XS's `--mxScope`); a `*_LOCAL` opcode's 1-based index `k`
    /// addresses `locals[k - 1]` (XS's `mxEnvironment - index`).
    locals: Vec<Slot>,
    /// `id -> locals index` for the frame's named `var`/`let`/`const`
    /// bindings, so the environment opcodes resolve a name to its scope
    /// slot (XS aliases the frame locals through the environment
    /// instance; this map is the behavioral equivalent).
    id_map: std::collections::HashMap<u16, usize>,
    /// The global object instance in the slot arena (§ Value and heap
    /// model). Its `next` chains its property slots; a top-level `var`
    /// hoists onto it (`fxRunEvalEnvironment` — top-level vars are global
    /// properties), and a sloppy assignment to an undeclared name creates
    /// one. This makes the global object a real arena object whose
    /// properties are real arena slots, so their allocation meters
    /// faithfully and the GC traces them.
    global_obj: crate::value::SlotIndex,
    /// `id -> property slot index` for the global object's own
    /// properties, the fast index into [`Self::global_obj`]'s property
    /// list. Presence marks that the property has been materialized (so
    /// its creation cost is metered exactly once). For a name that is
    /// also a declared frame local, the frame scope slot holds the
    /// working value (XS aliases the two through a closure cell — the
    /// closure-cell unification is child 2 of the stage-2b
    /// orchestration); the global property slot is materialized for
    /// allocation faithfulness and GC shape.
    global_props: std::collections::HashMap<u16, crate::value::SlotIndex>,
    result: Slot,
    /// Whether the frame runs in strict mode (`BEGIN_STRICT*`). Recorded
    /// for the exception/`this` semantics that observe it; the covered
    /// subset does not yet branch on it.
    strict: bool,
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
    /// Count of bytecode opcodes dispatched, before the invocation
    /// baseline — the raw dispatch count the differential harness reports
    /// for isolating a metering divergence. Distinct from the meter's
    /// computron count, which now also folds in the program overhead and
    /// the allocation metering.
    n_dispatched: u64,
    /// Side table of user-function metadata (body range + captured
    /// closures), keyed by the function instance's slot index. See
    /// [`FuncInfo`].
    functions: std::collections::HashMap<crate::value::SlotIndex, FuncInfo>,
    /// The saved caller states of the active call chain (design §
    /// Interpreter and dispatch: "frames are stack slots ... fixed offsets
    /// for result/function/this"). The top-level program is the base
    /// activation whose scope lives in the flat `locals`/`id_map`/`result`
    /// fields; each user-function `run` saves the current activation here
    /// and installs the callee's, and each `end` restores the top of this
    /// stack. Empty ⇒ the program frame is active (its `end`-equivalent is
    /// `return`, which exits to the C caller). XS keeps these frames inline
    /// on the slot stack; endor keeps the scope state per-activation here
    /// and the value stack shared, preserving the observable frame
    /// geometry (arguments below the frame, `result`/`function`/`this` at
    /// fixed offsets) that `run`/`argument`/`end` read.
    call_stack: Vec<CallerState>,
    /// The active frame's positional arguments (`mxFrameArgv`), read by
    /// `XS_CODE_ARGUMENT`. Empty in the program frame.
    args: Vec<Slot>,
    /// The active frame's `this` (`mxFrameThis`). Bound by `begin_*`;
    /// the covered subset does not yet branch on it.
    this_val: Slot,
    /// The active frame's function instance (`mxFrameFunction`), whose
    /// [`FuncInfo`] carries the closure environment closure opcodes resolve
    /// against. `NULL` in the program frame.
    cur_func: crate::value::SlotIndex,
}

/// A suspended activation: the caller's scope and resume point, saved by
/// `run` and restored by `end` (XS's `mxFrame->value.frame.{code,scope}`
/// plus the environment the frame aliases). The value stack is shared and
/// not saved here; `end` resets it to the frame boundary and pushes the
/// callee's result, matching XS's `mxStack = mxFrameEnd; *mxStack = *slot`.
struct CallerState {
    locals: Vec<Slot>,
    id_map: std::collections::HashMap<u16, usize>,
    result: Slot,
    strict: bool,
    args: Vec<Slot>,
    this_val: Slot,
    cur_func: crate::value::SlotIndex,
    /// The caller's code cursor to resume at (just past its `run`).
    ret_pc: usize,
}

impl Default for Interp {
    fn default() -> Self {
        Interp::new()
    }
}

impl Interp {
    pub fn new() -> Interp {
        let mut slots = SlotArena::new();
        // The global object is a real arena instance with a null
        // prototype (the intrinsic %Object.prototype% wiring lands with
        // the intrinsics seam); its allocation predates metering, so it
        // is not metered.
        let global_obj = slots.alloc(Slot::instance(crate::value::SlotIndex::NULL));
        Interp {
            stack: Vec::with_capacity(64),
            locals: Vec::new(),
            id_map: std::collections::HashMap::new(),
            global_obj,
            global_props: std::collections::HashMap::new(),
            result: Slot::undefined(),
            strict: false,
            meter: Meter::new(),
            meter_host: None,
            slots,
            chunks: ChunkArena::new(),
            n_dispatched: 0,
            functions: std::collections::HashMap::new(),
            call_stack: Vec::new(),
            args: Vec::new(),
            this_val: Slot::undefined(),
            cur_func: crate::value::SlotIndex::NULL,
        }
    }

    /// Seed a global binding by id, so a program that reads an
    /// undeclared name (`EVAL_REFERENCE`/`GET_VARIABLE` falling through
    /// to the global object) observes it. Used by
    /// [`crate::compartment::Compartment::evaluate`] to bind the
    /// compartment's own globals before running.
    pub fn define_global_id(&mut self, id: u16, value: Slot) {
        // Seeding a compartment global happens before the run, so it is
        // not metered (it is not a guest allocation the meter counts).
        self.create_global_property(id, (value.kind, value.value));
    }

    /// Allocate a property slot for global key `id`, link it into the
    /// global object's property list, and record it in [`Self::global_props`].
    /// Does **not** meter — callers add the allocation metering at the
    /// faithful opcode site. Returns the property slot index.
    fn create_global_property(&mut self, id: u16, value: (Kind, Payload)) -> crate::value::SlotIndex {
        let mut prop = Slot::property(id, value.1);
        prop.kind = value.0;
        // Insert at the head of the global object's property list.
        let head = self.slots.get(self.global_obj).next;
        prop.next = head;
        let idx = self.slots.alloc(prop);
        self.slots.get_mut(self.global_obj).next = idx;
        self.global_props.insert(id, idx);
        idx
    }

    /// Materialize a new own global property at run time (a hoisted
    /// `var`, or a sloppy assignment creating a global), metering the
    /// allocation exactly where `fxNewSlot`/`fxNewChunk` run:
    /// [`crate::meter::SLOT_ALLOCATION_METERING`] for the property slot
    /// plus the measured [`PROPERTY_CREATE_REMAINDER`] (the property-table
    /// growth and interned-key allocation not yet modeled as individual
    /// slots) — 536 raw total against the pin. Initialized undefined; a
    /// following `SET_VARIABLE` assigns and meters its own built-in step.
    fn materialize_global_property(&mut self, id: u16) -> crate::value::SlotIndex {
        self.tick_property_create();
        self.create_global_property(id, (Kind::Undefined, Payload::None))
    }

    /// Meter one new own-property allocation: the property `fxNewSlot`
    /// ([`crate::meter::SLOT_ALLOCATION_METERING`]) plus the measured
    /// [`PROPERTY_CREATE_REMAINDER`], 536 raw total against the pin.
    #[inline]
    fn tick_property_create(&mut self) {
        self.meter.tick_slot_alloc();
        self.meter.tick_raw(PROPERTY_CREATE_REMAINDER);
    }

    /// Arm metering (`fxBeginMetering`): install a check `interval` — a
    /// **computron** count, as the xsnap embedder passes — and the `host`
    /// callback the loop-closing check points consult with
    /// `meterIndex >> 16` ("computrons"). Per `fxBeginMetering` this
    /// scales `interval <<16` and resets the index to 0 (finding 2), so
    /// arm before running. The un-metered default is unchanged — a fresh
    /// `Interp` never arms and never checks, so the differential harness
    /// is unaffected. On host refusal, the run halts with
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

    /// Accrue the program-frame + eval-environment setup overhead, once,
    /// at the `BEGIN_*` program-entry opcode: the invocation baseline
    /// ([`PROGRAM_INVOCATION_COMPUTRONS`] dispatches C-XS meters in the
    /// caller frame before the captured bytecode) plus the measured
    /// environment-setup aggregate ([`PROGRAM_ENV_SETUP_METERING`]). Both
    /// are raw 16.16 units so they compose with the allocation metering
    /// through the carry into computrons. Synthetic bytecode that never
    /// executes a `BEGIN_*` (the meter unit tests) never accrues it.
    #[inline]
    fn tick_program_overhead(&mut self) {
        self.meter
            .tick_raw(PROGRAM_INVOCATION_COMPUTRONS * crate::meter::CODE_METERING);
        self.meter.tick_raw(PROGRAM_ENV_SETUP_METERING);
    }

    /// `fxRunEvalEnvironment`'s global-hoist branch: each declared
    /// top-level `var` (a `NEW_LOCAL` name) becomes an own property of
    /// the global object. Materialize each not-yet-present name's global
    /// property in declaration order, metering the allocation. Idempotent
    /// across a re-declared name (its property is created once).
    fn hoist_vars_to_global(&mut self) {
        // Declaration order = the `locals` index the name maps to.
        let mut names: Vec<(usize, u16)> =
            self.id_map.iter().map(|(&id, &i)| (i, id)).collect();
        names.sort_unstable();
        for (_, id) in names {
            if !self.global_props.contains_key(&id) {
                self.materialize_global_property(id);
            }
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
            // The meter now accrues everything C-XS's `meterIndex` does:
            // the per-opcode dispatch metering, the program-frame +
            // eval-environment setup overhead (at `BEGIN_*`, folding in
            // the invocation baseline), and the run-time allocation
            // metering (§ Allocation-faithful metering). Computrons are
            // `meterIndex >> 16`, directly comparable with the oracle.
            computrons: self.meter.computrons(),
            dispatched: self.n_dispatched,
            meter_raw: self.meter.raw(),
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
        // Unsigned 1-byte operand (a scope index; XS mxRunU1).
        macro_rules! u1 {
            ($off:expr) => {
                code[pc + $off] as usize
            };
        }
        // 2-byte little-endian ID operand (XS mxRunID == mxRunS2 on the
        // endor build). Used by the environment/variable opcodes.
        macro_rules! id {
            ($off:expr) => {
                u16::from_le_bytes([code[pc + $off], code[pc + $off + 1]])
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
            self.n_dispatched += 1;

            let size = op.size();
            // The resolved instruction length (fixed size, or the
            // 1+ID_SIZE / length-prefixed length for the variable
            // opcodes). ID-operand opcodes have `size == 0`, so they
            // must advance by `ilen`, never by `size` (a zero-advance
            // infinite loop).
            let ilen = match crate::opcode::instruction_len(code, pc) {
                Some(l) if l > 0 => l,
                _ => {
                    return Halt::Decode(format!(
                        "opcode {} at {} has unresolvable length",
                        op.name(),
                        pc
                    ))
                }
            };
            // Bounds-check the operands before reading.
            if pc + ilen > len {
                return Halt::Decode(format!(
                    "opcode {} at {} needs {} bytes, {} left",
                    op.name(),
                    pc,
                    ilen,
                    len - pc
                ));
            }

            use Opcode::*;
            match op {
                // ---- program prologue / frame -----------------------
                XS_CODE_BEGIN_SLOPPY => {
                    // `this` setup (`XS_CODE_BEGIN_SLOPPY` in `xsRun.c`):
                    // an `undefined`/`null` `this` in a sloppy frame binds
                    // to the realm global. The program-frame + eval-env
                    // setup overhead C-XS meters outside the captured
                    // bytecode is a property of the *program* invocation
                    // (`fxRunProgram`), so it accrues only on the top-level
                    // program's `begin` — a function frame's `begin` (a
                    // stack-based `run` set it up, dispatch-only) does not.
                    if self.call_stack.is_empty() {
                        self.tick_program_overhead();
                    } else {
                        self.bind_this_sloppy();
                    }
                    pc += size as usize;
                }
                XS_CODE_BEGIN_STRICT
                | XS_CODE_BEGIN_STRICT_BASE
                | XS_CODE_BEGIN_STRICT_DERIVED
                | XS_CODE_BEGIN_STRICT_FIELD => {
                    self.strict = true;
                    if self.call_stack.is_empty() {
                        self.tick_program_overhead();
                    }
                    pc += size as usize;
                }
                // The environment opcodes establish/refer to the frame's
                // variable environment. `EVAL_ENVIRONMENT` /
                // `PROGRAM_ENVIRONMENT` build it (a no-op here: the
                // frame's `locals` + `id_map` are the environment);
                // `EVAL_REFERENCE` / `PROGRAM_REFERENCE` push the
                // reference `GET_VARIABLE`/`SET_VARIABLE` resolve a name
                // against — the frame scope when the id is a declared
                // local, else the global object.
                XS_CODE_EVAL_ENVIRONMENT | XS_CODE_PROGRAM_ENVIRONMENT => {
                    // `fxRunEvalEnvironment`: a top-level program's `var`
                    // bindings hoist onto the global object as own
                    // properties (varEnvironment is null, so the global
                    // branch runs). Materialize each declared name's
                    // global property here — that is where XS allocates
                    // it — metering the allocation faithfully. The frame
                    // property slots hold the working value from here on.
                    self.hoist_vars_to_global();
                    // `fxRunEvalEnvironment` ends `the->scope = top + 1`,
                    // resetting the scope region: the hoisted vars now live
                    // in the global object, and their scope slots are freed
                    // and reused (a following `RESERVE`/`NEW_TEMPORARY`
                    // reuses scope index 1 — this is why an object-literal
                    // temporary and a hoisted var can both address `#1`).
                    // Reads/writes of a top-level var resolve to its global
                    // property from here (`resolve_get`/`resolve_set`).
                    self.locals.clear();
                    self.id_map.clear();
                    pc += size as usize;
                }
                XS_CODE_EVAL_REFERENCE | XS_CODE_PROGRAM_REFERENCE => {
                    let name = id!(1);
                    let env = if self.id_map.contains_key(&name) {
                        // Frame scope: NULL sentinel reference.
                        Slot::of(Kind::EnvReference, Payload::Reference(crate::value::SlotIndex::NULL))
                    } else {
                        // Global object: a distinct non-null sentinel.
                        Slot::of(Kind::EnvReference, Payload::Reference(crate::value::SlotIndex(0)))
                    };
                    self.push(env);
                    pc += ilen;
                }

                // ---- scope slots ------------------------------------
                // XS reserves the scope region (RESERVE) and fills it
                // downward with NEW_LOCAL/NEW_TEMPORARY (`--mxScope`); a
                // 1-based scope index `k` addresses the k-th declared
                // slot. Here the frame's `locals` vector is that region.
                XS_CODE_RESERVE_1 | XS_CODE_RESERVE_2 => {
                    // Space is grown lazily as NEW_LOCAL/NEW_TEMPORARY
                    // append; nothing to pre-allocate.
                    pc += size as usize;
                }
                XS_CODE_NEW_LOCAL => {
                    let name = id!(1);
                    self.locals.push(Slot::uninitialized());
                    self.id_map.insert(name, self.locals.len() - 1);
                    pc += ilen;
                }
                XS_CODE_NEW_TEMPORARY => {
                    self.locals.push(Slot::undefined());
                    pc += size as usize;
                }
                // Initialize/assign a scope slot from the stack top,
                // WITHOUT popping (the compiler emits an explicit POP
                // when the value is not wanted). `PULL_LOCAL` is the
                // popping variant.
                XS_CODE_VAR_LOCAL_1 | XS_CODE_LET_LOCAL_1 | XS_CODE_CONST_LOCAL_1 => {
                    let k = u1!(1);
                    let top = *self.stack.last().unwrap_or(&Slot::undefined());
                    self.set_local(k, top);
                    pc += size as usize;
                }
                XS_CODE_SET_LOCAL_1 => {
                    let k = u1!(1);
                    let top = *self.stack.last().unwrap_or(&Slot::undefined());
                    self.set_local(k, top);
                    pc += size as usize;
                }
                XS_CODE_PULL_LOCAL_1 => {
                    let k = u1!(1);
                    let v = self.pop();
                    self.set_local(k, v);
                    pc += size as usize;
                }
                XS_CODE_GET_LOCAL_1 => {
                    let k = u1!(1);
                    let v = self.get_local(k);
                    match v {
                        Some(s) => self.push(s),
                        None => return Halt::Throw("get: not initialized yet".into()),
                    }
                    pc += size as usize;
                }
                XS_CODE_UNWIND_1 => {
                    let n = u1!(1);
                    // Discard the n most-recently-declared scope slots
                    // (XS advances mxScope past them); prune their names.
                    let keep = self.locals.len().saturating_sub(n);
                    self.locals.truncate(keep);
                    self.id_map.retain(|_, &mut idx| idx < keep);
                    pc += size as usize;
                }

                // ---- variables (environment-resolved names) ---------
                // `get_this_variable` shares `get_variable`'s handler in
                // `xsRun.c` (a fused case): it resolves the name against the
                // top-of-stack environment reference and replaces it with
                // the value, which for a plain call is exactly a variable
                // read (the frame's `this` was pushed separately as
                // `undefined` before the reference).
                XS_CODE_GET_VARIABLE | XS_CODE_GET_THIS_VARIABLE => {
                    let name = id!(1);
                    // Consume the environment reference EVAL_REFERENCE
                    // pushed and resolve the name.
                    let _envref = self.pop();
                    let v = self.resolve_get(name);
                    match v {
                        Some(s) => self.push(s),
                        None => {
                            return Halt::Throw(format!("get {}: undefined variable", name))
                        }
                    }
                    pc += ilen;
                }
                XS_CODE_SET_VARIABLE => {
                    let name = id!(1);
                    // Stack: [.., envref, value]. Keep the value, drop
                    // the reference from under it (XS's SET_ALL pops the
                    // reference and leaves the assigned value).
                    let value = self.pop();
                    let _envref = self.pop();
                    // A frame-local var writes its scope slot; an
                    // undeclared name resolves to the global object,
                    // creating the property (a sloppy global) if absent —
                    // metered as one property creation exactly where
                    // `mxBehaviorSetProperty` allocates it.
                    if !self.id_map.contains_key(&name)
                        && !self.global_props.contains_key(&name)
                    {
                        self.materialize_global_property(name);
                    }
                    self.resolve_set(name, value);
                    // The property store itself is one built-in step
                    // (`mxMeterOne`, `XS_BUILTIN_METERING` = 1<<14),
                    // metered on every `SET_VARIABLE` whether the property
                    // pre-existed or was just created.
                    self.meter.tick_builtin();
                    self.push(value);
                    pc += ilen;
                }

                // ---- objects and properties -------------------------
                // `fxNewObject`: push a reference to a fresh instance.
                XS_CODE_OBJECT => {
                    let inst = self.new_object();
                    self.push(Slot::of(Kind::Reference, Payload::Reference(inst)));
                    pc += size as usize;
                }
                // Define a new own property (object-literal member).
                // Stack: [.., objectRef, value]; consumes both. Encoded
                // as 5 bytes — opcode + 2-byte id + a 2-byte inline flag
                // operand the compiler emits (`fxRunDefine`'s attributes),
                // which `gxCodeSizes` marks as an ID opcode (3) but whose
                // handler advances two further bytes (`xsRun.c` NEW_PROPERTY),
                // so the flag pair is NOT a separate dispatched opcode.
                XS_CODE_NEW_PROPERTY => {
                    if pc + 5 > len {
                        return Halt::Decode(format!("new_property at {} needs 5 bytes", pc));
                    }
                    let id = id!(1);
                    let value = self.pop();
                    let obj = self.pop();
                    if let Payload::Reference(inst) = obj.value {
                        // `fxRunDefine` creating a data property: one
                        // built-in step plus the property-slot allocation.
                        self.instance_put(inst, id, value);
                        self.meter.tick_builtin();
                    }
                    pc += 5;
                }
                // `o.k = v`. Stack: [.., objectRef, value] → [.., value].
                // Unlike `SET_VARIABLE`, the handler runs no `fxRunHas`
                // pre-check, so an overwrite meters nothing and a create
                // meters only the property allocation (536) — verified
                // against the pin's raw meter.
                XS_CODE_SET_PROPERTY => {
                    let id = id!(1);
                    let value = self.pop();
                    let obj = self.pop();
                    if let Payload::Reference(inst) = obj.value {
                        self.instance_put(inst, id, value);
                    }
                    self.push(value);
                    pc += ilen;
                }
                // `o.k`. Stack: [.., objectRef] → [.., value]. The handler
                // calls `mxBehaviorGetProperty` directly (no `mxGetID`
                // wrapper), so — like `GET_VARIABLE` — a property read
                // meters no built-in step (verified against the pin: a
                // repeated `o.a;` adds only its dispatch computrons).
                XS_CODE_GET_PROPERTY => {
                    let id = id!(1);
                    let obj = self.pop();
                    let v = match obj.value {
                        Payload::Reference(inst) => self.instance_get(inst, id),
                        _ => Slot::undefined(),
                    };
                    self.push(v);
                    pc += ilen;
                }

                // ---- user functions: definition ---------------------
                // `constructor_function` / `function` (`fxNewFunctionInstance`):
                // push a fresh callable instance. `constructor_function`
                // additionally runs `fxDefaultFunctionPrototype` (the
                // `.prototype`/`constructor` pair); both allocation clusters
                // are the measured [`FUNCTION_DEFINE_METERING`]. The body
                // range and closures are filled in by the following `code`
                // and `function_environment` opcodes.
                XS_CODE_CONSTRUCTOR_FUNCTION | XS_CODE_FUNCTION => {
                    let name = id!(1);
                    let f = self.new_function(name);
                    self.push(Slot::of(Kind::Reference, Payload::Reference(f)));
                    pc += ilen;
                }
                // `code N` (`XS_CODE_CODE_*`): `fxNewChunk(N)` copies the N
                // body bytes into a chunk (metered per byte) and records the
                // body address on the top-of-stack function; execution skips
                // past the body (it runs only when the function is called).
                XS_CODE_CODE_1 | XS_CODE_CODE_2 | XS_CODE_CODE_4 => {
                    let n = match op {
                        XS_CODE_CODE_1 => code[pc + 1] as usize,
                        XS_CODE_CODE_2 => {
                            u16::from_le_bytes([code[pc + 1], code[pc + 2]]) as usize
                        }
                        _ => u32::from_le_bytes([
                            code[pc + 1],
                            code[pc + 2],
                            code[pc + 3],
                            code[pc + 4],
                        ]) as usize,
                    };
                    let body_start = pc + size as usize;
                    // `fxNewChunk(N)` meters the header+alignment-adjusted
                    // body size, not N (see `Meter::tick_chunk_new`).
                    self.meter.tick_chunk_new(n as u64);
                    // The function's declared parameters/locals each carry a
                    // fixed definition-time allocation cost
                    // ([`FUNCTION_LOCAL_METERING`]); count the `new_local`
                    // opcodes in this body (skipping nested function bodies)
                    // and accrue it here, where C-XS incurs it at
                    // definition rather than per call.
                    let locals = count_new_locals(code, body_start, n);
                    self.meter
                        .tick_raw(FUNCTION_LOCAL_METERING * locals as u64);
                    if let Payload::Reference(f) = self.stack.last().map(|s| s.value).unwrap_or(Payload::None) {
                        let info = self.functions.entry(f).or_default();
                        info.body_start = body_start;
                        info.body_len = n;
                    }
                    pc = body_start + n;
                }
                // `function_environment` (`fxNewEnvironmentInstance`): the
                // function captures its defining scope through a fresh
                // closure environment instance whose prototype is the
                // defining frame's environment. XS pushes the env reference
                // on top of the function (net +1 slot); the following
                // `store` opcodes append captured closure cells to it, then
                // a `pop` discards it. (For a non-capturing function no
                // `store` follows and the `pop` discards the env directly.)
                XS_CODE_FUNCTION_ENVIRONMENT => {
                    let env = self.new_environment();
                    // The function is the current top; record its captured
                    // environment before pushing the env reference.
                    if let Some(&Slot { value: Payload::Reference(f), .. }) = self.stack.last() {
                        self.functions.entry(f).or_default().closures = env;
                    }
                    self.push(Slot::of(Kind::Reference, Payload::Reference(env)));
                    pc += size as usize;
                }

                // ---- user functions: call --------------------------
                // `call` (`XS_CODE_CALL`): reserve the RESULT (undefined)
                // and FRAME (marker) slots above the already-pushed
                // FUNCTION and THIS. No heap allocation — the frame lives on
                // the value stack (metered by dispatch only).
                XS_CODE_CALL => {
                    self.push(Slot::undefined()); // RESULT
                    self.push(Slot::of(Kind::Uninitialized, Payload::None)); // FRAME
                    pc += size as usize;
                }
                // `run`/`run_N` (`XS_CODE_RUN*`): invoke the function with N
                // arguments. Stack below the N args is
                // `[THIS, FUNCTION, RESULT, FRAME]` (XS's frame geometry:
                // args below the frame, `result`/`function`/`this` at fixed
                // offsets). Enter the callee's body frame; the call-entry
                // `mxFirstCode` meter check fires here.
                XS_CODE_RUN | XS_CODE_RUN_1 | XS_CODE_RUN_2 | XS_CODE_RUN_4 => {
                    let argc = match op {
                        XS_CODE_RUN => self.pop_run_count(),
                        XS_CODE_RUN_1 => code[pc + 1] as usize,
                        XS_CODE_RUN_2 => {
                            u16::from_le_bytes([code[pc + 1], code[pc + 2]]) as usize
                        }
                        _ => u32::from_le_bytes([
                            code[pc + 1],
                            code[pc + 2],
                            code[pc + 3],
                            code[pc + 4],
                        ]) as usize,
                    };
                    let ret_pc = pc + size as usize;
                    match self.enter_call(argc, ret_pc) {
                        Ok(body_start) => {
                            // Call entry: `mxFirstCode()` runs a meter check
                            // before the callee's first opcode.
                            if self.check_meter() == MeterCheck::Abort {
                                return Halt::MeterAbort;
                            }
                            pc = body_start;
                        }
                        Err(h) => return h,
                    }
                }
                // `argument i` (`XS_CODE_ARGUMENT`): push the frame's i-th
                // positional argument (`mxFrameArgv(i)`), or `undefined`
                // when fewer were passed.
                XS_CODE_ARGUMENT => {
                    let i = u1!(1);
                    let v = self.args.get(i).copied().unwrap_or_else(Slot::undefined);
                    self.push(v);
                    pc += size as usize;
                }

                // ---- closures (captured variables via heap cells) ---
                // `new_closure id` (`XS_CODE_NEW_CLOSURE`): declare a
                // captured binding. Allocate a heap cell (`fxNewSlot`,
                // metered) initialized uninitialized, and append a
                // closure-kind scope slot pointing at it. The cell is what
                // the capturing closures share.
                XS_CODE_NEW_CLOSURE => {
                    let name = id!(1);
                    let cell = self.slots.alloc(Slot::uninitialized());
                    self.meter.tick_slot_alloc(); // fxNewSlot for the cell
                    let mut slot = Slot::of(Kind::Closure, Payload::Reference(cell));
                    slot.id = name;
                    self.locals.push(slot);
                    self.id_map.insert(name, self.locals.len() - 1);
                    pc += ilen;
                }
                // `get_closure #k`: read the shared cell of scope closure k.
                XS_CODE_GET_CLOSURE_1 | XS_CODE_GET_CLOSURE_2 => {
                    let k = self.closure_index(op, code, pc);
                    match self.closure_cell(k) {
                        Some(cell) => {
                            let s = *self.slots.get(cell);
                            if s.kind == Kind::Uninitialized {
                                return Halt::Throw("get closure: not initialized yet".into());
                            }
                            self.push(Slot::of(s.kind, s.value));
                        }
                        None => return Halt::Throw("get closure: no cell".into()),
                    }
                    pc += op.size() as usize;
                }
                // `var_closure #k` / `set_closure #k`: write the shared cell
                // from the stack top **without** popping (an explicit `pop`
                // discards it when unwanted).
                XS_CODE_VAR_CLOSURE_1
                | XS_CODE_VAR_CLOSURE_2
                | XS_CODE_SET_CLOSURE_1
                | XS_CODE_SET_CLOSURE_2 => {
                    let k = self.closure_index(op, code, pc);
                    let top = *self.stack.last().unwrap_or(&Slot::undefined());
                    self.write_closure_cell(k, top);
                    pc += op.size() as usize;
                }
                // `pull_closure #k`: pop and write the shared cell.
                XS_CODE_PULL_CLOSURE_1 | XS_CODE_PULL_CLOSURE_2 => {
                    let k = self.closure_index(op, code, pc);
                    let v = self.pop();
                    self.write_closure_cell(k, v);
                    pc += op.size() as usize;
                }
                // `retrieve #k` (`XS_CODE_RETRIEVE_*`): import the callee's
                // `k` captured closures from its closure environment into
                // the frame scope (copying the closure-kind slots, which
                // point at the shared cells — no allocation).
                XS_CODE_RETRIEVE_1 | XS_CODE_RETRIEVE_2 => {
                    let k = self.closure_index(op, code, pc);
                    self.retrieve_closures(k);
                    pc += op.size() as usize;
                }
                // `store #k` / `store_arrow` (`XS_CODE_STORE_*`): capture the
                // scope closure `k` into the top-of-stack environment,
                // appending a shared-cell reference (`fxNewSlot`, metered).
                XS_CODE_STORE_1 | XS_CODE_STORE_2 => {
                    let k = self.closure_index(op, code, pc);
                    self.store_closure(k);
                    pc += op.size() as usize;
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
                // `end` (`XS_CODE_END`, xsRun.c:1049): a function body's
                // terminator. Pop the callee frame, reset the value stack to
                // the frame boundary, push the callee's result into the
                // caller, and resume the caller. C-XS runs `mxFirstCode()`
                // (a meter check) **only when the caller is a JS frame**;
                // when the popped frame's caller is the C boundary (an empty
                // call stack here), it returns to C with **no** check. The
                // top-level program never reaches `end` (it ends in
                // `return`), so a JS caller always exists when `end` runs in
                // the covered grammar — but the guard is explicit so the
                // abort-point semantics are exact (stage-2a review finding
                // 1).
                XS_CODE_END
                | XS_CODE_END_ARROW
                | XS_CODE_END_BASE
                | XS_CODE_END_DERIVED => {
                    if self.call_stack.is_empty() {
                        // A function as the top activation returning to C:
                        // no meter check (exit-to-host END).
                        return Halt::Return;
                    }
                    let ret = self.result;
                    let resume = self.leave_call();
                    self.push(ret);
                    pc = resume;
                    // Returning into a JS caller: `mxFirstCode()` checks.
                    if self.check_meter() == MeterCheck::Abort {
                        return Halt::MeterAbort;
                    }
                }
                // `return` (`XS_CODE_RETURN`, xsRun.c:1080): the top-level
                // program's terminator. C-XS always returns to the C caller
                // here with **no** meter check. Only the program frame emits
                // it (a `return x` inside a function compiles to
                // `set_result; end`), so this is the exit-to-host boundary.
                XS_CODE_RETURN => {
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

    /// Allocate a fresh user-function instance (`fxNewFunctionInstance` +
    /// `fxDefaultFunctionPrototype`, driven by `constructor_function`).
    /// The instance is a real arena object; its body range and closures are
    /// recorded in [`Self::functions`] by the following `code` /
    /// `function_environment` opcodes. Meters the measured allocation
    /// cluster [`FUNCTION_DEFINE_METERING`].
    fn new_function(&mut self, name: u16) -> crate::value::SlotIndex {
        self.meter.tick_raw(FUNCTION_DEFINE_METERING);
        // `fxNewFunctionInstance` runs `fxRenameFunction`; naming the
        // instance with a real id (an inferred `var f = function(){}` or a
        // `function g(){}` declaration — anything but `XS_NO_ID` = 0)
        // costs two additional built-in steps (`mxMeterOne`) over the
        // anonymous case folded into [`FUNCTION_DEFINE_METERING`]. Measured
        // against the pin as exactly `2 * XS_BUILTIN_METERING` = 32768 raw,
        // independent of the name's length (the name symbol's string chunk
        // is interned at parse time, outside the run-only meter).
        if name != crate::value::XS_NO_ID {
            self.meter.tick_builtin_some(2);
        }
        let f = self.slots.alloc(Slot::instance(crate::value::SlotIndex::NULL));
        self.functions.insert(f, FuncInfo::default());
        f
    }

    /// Allocate a closure environment instance (`fxNewEnvironmentInstance`,
    /// driven by `function_environment`). Meters
    /// [`FUNCTION_ENVIRONMENT_METERING`]. The environment is a real arena
    /// instance so its captured cells are GC-traced.
    fn new_environment(&mut self) -> crate::value::SlotIndex {
        self.meter.tick_raw(FUNCTION_ENVIRONMENT_METERING);
        // `fxNewEnvironmentInstance` allocates the instance plus one
        // internal behavior slot (`XS_ENVIRONMENT_BEHAVIOR`); captured
        // closures (`store`) append after it, and `retrieve` reads them at
        // `env.next.next`. The two-slot cost is folded into
        // [`FUNCTION_DEFINE_METERING`] (calibrated on a function whose
        // `function_environment` runs), so it is not metered again here.
        let env = self.slots.alloc(Slot::instance(crate::value::SlotIndex::NULL));
        let behavior = self
            .slots
            .alloc(Slot::of(Kind::Uninitialized, Payload::None));
        self.slots.get_mut(env).next = behavior;
        env
    }

    /// `XS_CODE_RUN`'s inline argument count (pushed as an integer just
    /// below the frame). The variadic `run` reads it off the stack.
    fn pop_run_count(&mut self) -> usize {
        match self.pop().value {
            Payload::Integer(i) if i >= 0 => i as usize,
            _ => 0,
        }
    }

    /// Enter a user-function call with `argc` arguments (`XS_CODE_RUN_ALL`).
    /// The value stack below the `argc` args holds the frame geometry
    /// `[THIS, FUNCTION, RESULT, FRAME]`; read the function and `this`,
    /// collect the arguments, unwind those `4 + argc` slots, save the
    /// caller's activation, and install the callee's fresh scope. Returns
    /// the callee body's start pc, or `Halt::Throw` when the callee is not a
    /// known user function (the covered grammar only calls functions it
    /// defined).
    fn enter_call(&mut self, argc: usize, ret_pc: usize) -> Result<usize, Halt> {
        let len = self.stack.len();
        if len < argc + 4 {
            return Err(Halt::Throw("call: stack underflow".into()));
        }
        let base = len - argc - 4; // index of THIS
        let func_slot = self.stack[base + 1];
        // Collect arguments (arg0 is the deepest of the argc; XS's
        // `mxFrameArgv(i) = mxFrame - 1 - i`).
        let args: Vec<Slot> = self.stack[base + 4..base + 4 + argc].to_vec();
        let func = match func_slot.value {
            Payload::Reference(f) if self.functions.contains_key(&f) => f,
            _ => return Err(Halt::Throw("call: not a function".into())),
        };
        let body_start = self.functions[&func].body_start;
        let this_val = self.stack[base];
        // Unwind the frame region (THIS..last arg).
        self.stack.truncate(base);
        // Save the caller's activation and install the callee's.
        self.call_stack.push(CallerState {
            locals: std::mem::take(&mut self.locals),
            id_map: std::mem::take(&mut self.id_map),
            result: self.result,
            strict: self.strict,
            args: std::mem::take(&mut self.args),
            this_val: self.this_val,
            cur_func: self.cur_func,
            ret_pc,
        });
        self.result = Slot::undefined();
        self.strict = false;
        self.args = args;
        self.this_val = this_val;
        self.cur_func = func;
        Ok(body_start)
    }

    /// Leave a user-function call (`XS_CODE_END`): restore the caller's
    /// saved activation and return the pc to resume the caller at. The
    /// callee's result has already been captured by the caller of this
    /// method (which pushes it onto the shared value stack, matching XS's
    /// `mxStack = mxFrameEnd; *mxStack = *result`).
    fn leave_call(&mut self) -> usize {
        let caller = self
            .call_stack
            .pop()
            .expect("leave_call with empty call stack");
        self.locals = caller.locals;
        self.id_map = caller.id_map;
        self.result = caller.result;
        self.strict = caller.strict;
        self.args = caller.args;
        self.this_val = caller.this_val;
        self.cur_func = caller.cur_func;
        caller.ret_pc
    }

    /// Read a `*_CLOSURE_*`/`retrieve`/`store` opcode's 1-based scope index
    /// operand (`mxEnvironment - index`): a `u8` for the `_1` variant, a
    /// little-endian `u16` for `_2`.
    fn closure_index(&self, op: Opcode, code: &[u8], pc: usize) -> usize {
        if op.size() == 2 {
            code[pc + 1] as usize
        } else {
            u16::from_le_bytes([code[pc + 1], code[pc + 2]]) as usize
        }
    }

    /// The shared heap cell a closure scope slot `k` (1-based) indirects
    /// to, or `None` if the slot is out of range or not a closure.
    fn closure_cell(&self, k: usize) -> Option<crate::value::SlotIndex> {
        let i = self.local_index(k)?;
        let s = self.locals[i];
        match (s.kind, s.value) {
            (Kind::Closure, Payload::Reference(cell)) => Some(cell),
            _ => None,
        }
    }

    /// Write value `v` through closure scope slot `k` into its shared cell
    /// (all closures capturing the binding observe the mutation).
    fn write_closure_cell(&mut self, k: usize, v: Slot) {
        if let Some(cell) = self.closure_cell(k) {
            let c = self.slots.get_mut(cell);
            c.kind = v.kind;
            c.value = v.value;
        }
    }

    /// `XS_CODE_RETRIEVE`: import the running function's `k` captured
    /// closures from its closure environment (`functions[cur_func].closures`,
    /// whose stored closures live at `env.next.next` onward) into the frame
    /// scope, copying the closure-kind slots so they point at the same
    /// shared cells. No allocation (the cells already exist).
    fn retrieve_closures(&mut self, k: usize) {
        let env = self
            .functions
            .get(&self.cur_func)
            .map(|f| f.closures)
            .unwrap_or(crate::value::SlotIndex::NULL);
        if env.is_null() {
            return;
        }
        // env.next = behavior slot; behavior.next = first stored closure.
        let behavior = self.slots.get(env).next;
        let mut cur = if behavior.is_null() {
            crate::value::SlotIndex::NULL
        } else {
            self.slots.get(behavior).next
        };
        for _ in 0..k {
            if cur.is_null() {
                break;
            }
            let s = *self.slots.get(cur);
            let mut copy = Slot::of(s.kind, s.value);
            copy.id = s.id;
            copy.flag = s.flag;
            self.locals.push(copy);
            if s.id != 0 {
                self.id_map.insert(s.id, self.locals.len() - 1);
            }
            cur = s.next;
        }
    }

    /// `XS_CODE_STORE`: capture scope closure `k` into the top-of-stack
    /// closure environment, appending a shared-cell reference to the
    /// environment's property list (`fxNewSlot`, metered). The stored slot
    /// keeps the same cell reference, so the captured closure and the
    /// defining frame share one cell.
    fn store_closure(&mut self, k: usize) {
        let env = match self.stack.last() {
            Some(&Slot { value: Payload::Reference(e), .. }) => e,
            _ => return,
        };
        let i = match self.local_index(k) {
            Some(i) => i,
            None => return,
        };
        let src = self.locals[i];
        // fxNewSlot for the appended closure slot.
        self.meter.tick_slot_alloc();
        let mut stored = Slot::of(src.kind, src.value);
        stored.id = src.id;
        stored.flag = src.flag;
        let idx = self.slots.alloc(stored);
        // Append to the end of the environment's property chain.
        let mut tail = env;
        loop {
            let next = self.slots.get(tail).next;
            if next.is_null() {
                break;
            }
            tail = next;
        }
        self.slots.get_mut(tail).next = idx;
    }

    /// `XS_CODE_BEGIN_SLOPPY`'s `this` binding: an `undefined`/`null` `this`
    /// in a sloppy function frame binds to the realm global. Recorded for
    /// the `this`/method semantics that observe it; the covered call
    /// grammar (plain calls) passes `undefined`.
    fn bind_this_sloppy(&mut self) {
        if matches!(self.this_val.kind, Kind::Undefined | Kind::Null) {
            self.this_val = Slot::of(
                Kind::Reference,
                Payload::Reference(self.global_obj),
            );
        }
    }

    /// Allocate a fresh object instance on the slot heap (`fxNewObject`)
    /// and return its index. Prototype is null for now (the intrinsic
    /// `%Object.prototype%` wiring lands with the intrinsics seam; the
    /// covered grammar only reads/writes own properties). Meters the
    /// `fxNewObject` cost measured against the pin: one built-in step
    /// (`mxMeterOne`) plus one property-slot `fxNewSlot`
    /// ([`crate::meter::SLOT_ALLOCATION_METERING`]) — 16640 raw total.
    fn new_object(&mut self) -> crate::value::SlotIndex {
        self.meter.tick_builtin();
        self.meter.tick_slot_alloc();
        self.slots.alloc(Slot::instance(crate::value::SlotIndex::NULL))
    }

    /// Find an own property slot of `inst` by key `id`, walking its
    /// `next`-linked property list. Every slot in the list is a property
    /// (XS's property slots hold the value directly, keyed by `id`), so
    /// the match is by `id` alone — a property slot's `kind` is the
    /// value's kind, not a separate marker.
    fn find_property(&self, inst: crate::value::SlotIndex, id: u16) -> Option<crate::value::SlotIndex> {
        let mut cur = self.slots.get(inst).next;
        while !cur.is_null() {
            let s = self.slots.get(cur);
            if s.id == id {
                return Some(cur);
            }
            cur = s.next;
        }
        None
    }

    /// Define/set an own property `id = value` on instance `inst`,
    /// creating the property slot (metered `fxNewSlot`) when absent.
    /// Returns `true` if a new property was created. The property slot
    /// holds the value directly (its `kind`/`value` are the value's),
    /// with `id` the key and `next` the following property.
    fn instance_put(&mut self, inst: crate::value::SlotIndex, id: u16, value: Slot) -> bool {
        if let Some(p) = self.find_property(inst, id) {
            let s = self.slots.get_mut(p);
            s.kind = value.kind;
            s.value = value.value;
            false
        } else {
            self.tick_property_create(); // fxNewSlot + property-table growth (536)
            let head = self.slots.get(inst).next;
            let mut prop = value;
            prop.id = id;
            prop.flag = 0;
            prop.next = head;
            let idx = self.slots.alloc(prop);
            self.slots.get_mut(inst).next = idx;
            true
        }
    }

    /// Read own property `id` of instance `inst` (or `undefined` when
    /// absent — the covered grammar has a null prototype, so there is no
    /// prototype walk yet).
    fn instance_get(&self, inst: crate::value::SlotIndex, id: u16) -> Slot {
        match self.find_property(inst, id) {
            Some(p) => {
                let s = self.slots.get(p);
                Slot::of(s.kind, s.value)
            }
            None => Slot::undefined(),
        }
    }

    /// Address a 1-based scope index `k` (XS's `mxEnvironment - index`).
    #[inline]
    fn local_index(&self, k: usize) -> Option<usize> {
        if k == 0 || k > self.locals.len() {
            None
        } else {
            Some(k - 1)
        }
    }

    /// Read scope slot `k`; `None` if it is still uninitialized (a TDZ
    /// read) or the index is out of range.
    fn get_local(&self, k: usize) -> Option<Slot> {
        let i = self.local_index(k)?;
        let s = self.locals[i];
        if s.kind == Kind::Uninitialized {
            None
        } else {
            Some(s)
        }
    }

    /// Write scope slot `k` from a value (kind + payload), mirroring
    /// XS's `variable->kind = ...; variable->value = ...`.
    fn set_local(&mut self, k: usize, v: Slot) {
        if let Some(i) = self.local_index(k) {
            self.locals[i].kind = v.kind;
            self.locals[i].value = v.value;
        }
    }

    /// Resolve a name for reading: a frame local when declared (unless
    /// uninitialized), else the global object's property.
    fn resolve_get(&self, name: u16) -> Option<Slot> {
        if let Some(&i) = self.id_map.get(&name) {
            let s = self.locals[i];
            if s.kind == Kind::Uninitialized {
                None
            } else {
                Some(s)
            }
        } else if let Some(&idx) = self.global_props.get(&name) {
            let p = self.slots.get(idx);
            Some(Slot::of(p.kind, p.value))
        } else {
            None
        }
    }

    /// Resolve a name for writing: a frame local when declared, else the
    /// global object's property slot (which must already exist — the
    /// caller materializes it and meters the creation first).
    fn resolve_set(&mut self, name: u16, value: Slot) {
        if let Some(&i) = self.id_map.get(&name) {
            self.locals[i].kind = value.kind;
            self.locals[i].value = value.value;
        } else if let Some(&idx) = self.global_props.get(&name) {
            let p = self.slots.get_mut(idx);
            p.kind = value.kind;
            p.value = value.value;
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

/// Count the `new_local` opcodes in a function body `[start, start+len)`,
/// skipping any nested function bodies (a nested `code M` embeds M bytes
/// that belong to the inner function, whose own `code` opcode counts
/// them). Mirrors the dispatch loop's instruction-advance quirks: an
/// `id`-operand opcode is `1 + ID_SIZE`, `new_property`/`new_property_at`
/// carry a 2-byte inline flag operand (5 bytes total), and `code_*`
/// advances past both its length operand and the embedded body. Used to
/// meter the per-declared-local definition cost ([`FUNCTION_LOCAL_METERING`])
/// at `code` time.
fn count_new_locals(code: &[u8], start: usize, len: usize) -> usize {
    let end = (start + len).min(code.len());
    let mut pc = start;
    let mut n = 0usize;
    while pc < end {
        let op = match Opcode::from_u8(code[pc]) {
            Some(o) => o,
            None => break,
        };
        if op == Opcode::XS_CODE_NEW_LOCAL {
            n += 1;
        }
        let step = match op {
            // Nested function body: skip the length operand and the M
            // embedded body bytes (they are the inner function's locals).
            Opcode::XS_CODE_CODE_1 => 2 + *code.get(pc + 1).unwrap_or(&0) as usize,
            Opcode::XS_CODE_CODE_2 => {
                3 + u16::from_le_bytes([
                    *code.get(pc + 1).unwrap_or(&0),
                    *code.get(pc + 2).unwrap_or(&0),
                ]) as usize
            }
            Opcode::XS_CODE_CODE_4 => {
                5 + u32::from_le_bytes([
                    *code.get(pc + 1).unwrap_or(&0),
                    *code.get(pc + 2).unwrap_or(&0),
                    *code.get(pc + 3).unwrap_or(&0),
                    *code.get(pc + 4).unwrap_or(&0),
                ]) as usize
            }
            // The 2-byte inline flag operand `new_property`/`new_property_at`
            // carry past their id (the dispatch loop advances 5, not the
            // `instruction_len` id-opcode 3).
            Opcode::XS_CODE_NEW_PROPERTY | Opcode::XS_CODE_NEW_PROPERTY_AT => 5,
            _ => crate::opcode::instruction_len(code, pc).unwrap_or(1),
        };
        if step == 0 {
            break;
        }
        pc += step;
    }
    n
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
        // No `BEGIN_*` here, so the program-setup overhead is not
        // accrued — this exercises the meter in isolation.
        let code = [b(Opcode::XS_CODE_BRANCH_1), 0xFE]; // -2

        // Record every computron value the host is shown; refuse at 5.
        let seen = Rc::new(RefCell::new(Vec::new()));
        let seen_cb = Rc::clone(&seen);
        let mut interp = Interp::new();
        // interval 1 computron (finding 2: `fxBeginMetering` scales it
        // <<16). Each backward branch dispatches one opcode = one
        // computron; `fxCheckMetering` fires when `meterIndex >
        // meterCount` and then advances the window by the interval, so
        // with a one-computron window and one-computron opcodes the host
        // is consulted at computrons 2, 4, 6, ... — exactly C-XS's
        // check cadence (the fire is one opcode after the window opens).
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
        // Consulted at 2, 4, 6; refuses at 6 (6 >= 5). Six dispatched
        // backward branches; computrons = meterIndex >> 16 = 6.
        assert_eq!(*seen.borrow(), vec![2, 4, 6], "host consulted on C-XS's cadence");
        assert_eq!(out.dispatched, 6, "aborts on the branch that crosses the refusal");
        assert_eq!(out.computrons, 6);
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
    fn user_function_call_runs_and_meters_bit_exact() {
        // The exact C-XS bytecode for `(function(x){return x+1})(5)`
        // (captured from the oracle), run oracle-free: the frame machinery
        // (`constructor_function`/`code`/`function_environment`/`call`/
        // `run_1`/`argument`/`end`) must produce the completion `6` and the
        // C-XS computron count `30` — a standing lock on the definition-site
        // allocation metering and dispatch-metered stack frames, so a
        // regression is caught without linking C.
        let code: [u8; 44] = [
            0x0b, 0x00, 0x4b, 0xe0, 0x38, 0x00, 0x00, 0x2e, 0x13, 0x0b, 0x01, 0x9e, 0x01, 0x86,
            0x01, 0x00, 0x02, 0x00, 0xe6, 0x01, 0x92, 0x5c, 0x01, 0x72, 0x01, 0x01, 0xbb, 0x44,
            0x58, 0x92, 0x42, 0xe0, 0x89, 0x02, 0x00, 0x72, 0x04, 0x28, 0x72, 0x05, 0xab, 0x01,
            0xbb, 0xa9,
        ];
        let out = Interp::new().run(&code);
        assert_eq!(out.halt, Halt::Return);
        assert!(out.completed);
        assert_eq!(out.result, "6", "the call returns x+1 with x=5");
        assert_eq!(out.computrons, 30, "bit-exact computrons vs C-XS");
    }

    #[test]
    fn nested_user_function_calls_run_and_meter_bit_exact() {
        // `(function(){return (function(){return 1})()})()`, captured from
        // the oracle: two definitions and two nested calls, completion `1`,
        // C-XS computrons `36`.
        let code: [u8; 51] = [
            0x0b, 0x00, 0x4b, 0xe0, 0x38, 0x00, 0x00, 0x2e, 0x1c, 0x0b, 0x00, 0xe0, 0x38, 0x00,
            0x00, 0x2e, 0x06, 0x0b, 0x00, 0x72, 0x01, 0xbb, 0x44, 0x58, 0x92, 0x42, 0xe0, 0x89,
            0x01, 0x00, 0x72, 0x04, 0x28, 0xab, 0x00, 0xbb, 0x44, 0x58, 0x92, 0x42, 0xe0, 0x89,
            0x01, 0x00, 0x72, 0x04, 0x28, 0xab, 0x00, 0xbb, 0xa9,
        ];
        let out = Interp::new().run(&code);
        assert_eq!(out.halt, Halt::Return);
        assert!(out.completed);
        assert_eq!(out.result, "1");
        assert_eq!(out.computrons, 36, "bit-exact computrons vs C-XS");
    }

    #[test]
    fn closure_capture_and_mutation_run_and_meter_bit_exact() {
        // The exact C-XS bytecode for
        // `var mk=function(){var c=0; return function(){c=c+1; return c}};
        //  var f=mk(); f(); f()` (captured from the oracle), run
        // oracle-free: the closure machinery
        // (`new_closure`/`store`/`function_environment`/`retrieve`/
        // `get_closure`/`pull_closure`) shares one heap cell between the
        // factory frame and the returned closure, so the two `f()` calls
        // mutate `c` to `2`, and the computron count matches C-XS's `87` —
        // a standing lock on the cell-allocation metering without linking C.
        let code: [u8; 131] = [
            0x0b, 0x00, 0x9e, 0x02, 0x86, 0x02, 0x00, 0xe0, 0xe6, 0x01, 0x92, 0x86, 0x03, 0x00,
            0xe0, 0xe6, 0x02, 0x92, 0x4b, 0x4d, 0x03, 0x00, 0x38, 0x03, 0x00, 0x2e, 0x33, 0x0b,
            0x00, 0x9e, 0x01, 0x85, 0x01, 0x00, 0xe0, 0xe4, 0x01, 0x92, 0x72, 0x00, 0xe4, 0x01,
            0x92, 0x38, 0x00, 0x00, 0x2e, 0x11, 0x0b, 0x00, 0x9e, 0x01, 0xa5, 0x01, 0x5a, 0x01,
            0x72, 0x01, 0x01, 0x95, 0x01, 0x5a, 0x01, 0xbb, 0x44, 0x58, 0xc4, 0x01, 0x92, 0x42,
            0xe0, 0x89, 0x04, 0x00, 0x72, 0x04, 0xbb, 0x44, 0x58, 0x92, 0x42, 0xe0, 0x89, 0x04,
            0x00, 0x72, 0x04, 0xbf, 0x03, 0x00, 0x92, 0x4d, 0x02, 0x00, 0xe0, 0x4d, 0x03, 0x00,
            0x66, 0x03, 0x00, 0x28, 0xab, 0x00, 0xbf, 0x02, 0x00, 0x92, 0xe0, 0x4d, 0x02, 0x00,
            0x66, 0x02, 0x00, 0x28, 0xab, 0x00, 0xbb, 0xe0, 0x4d, 0x02, 0x00, 0x66, 0x02, 0x00,
            0x28, 0xab, 0x00, 0xbb, 0xa9,
        ];
        let out = Interp::new().run(&code);
        assert_eq!(out.halt, Halt::Return);
        assert!(out.completed);
        assert_eq!(out.result, "2", "the shared closure cell mutates across the two f() calls");
        assert_eq!(out.computrons, 87, "bit-exact computrons vs C-XS");
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
