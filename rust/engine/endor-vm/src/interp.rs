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

/// The raw 16.16 cost C-XS accrues unwinding an **uncaught** throw across
/// the host boundary (`fxJump` longjmps into `fxBeginHost`'s `mxTry`). Two
/// effects combine, both measured against the pin `48ee02d8cfe0`: the
/// escaping `throw`/`rethrow` opcode is **never metered** (the longjmp
/// bypasses its `mxBreak`, where the computed-goto dispatch accrues an
/// opcode's `XS_CODE_METERING`), and the host-boundary teardown accrues a
/// fixed `1<<15` raw. Verified: an uncaught `throw 7` and `1; throw 7`
/// each carry exactly `PROGRAM_ENV_SETUP_METERING + 32768` beyond their
/// pre-throw dispatch metering (raw `443672` and `574744` respectively,
/// = 6 and 8 metered opcodes plus this remainder). Modeled by
/// [`crate::meter::Meter::untick_code`] on the escaping opcode plus
/// accruing this constant. A *caught* throw needs no adjustment: the
/// `CATCH` resume's `mxBreak` meters the catch target exactly as endor's
/// dispatch does, so caught exceptions are bit-exact without it.
pub const THROW_HOST_ESCAPE_METERING: u64 = 1 << 15;

/// XS's value stack is a fixed array of `stackCount` slots
/// (`endor-oracle/csrc/endor_shim.c` and the endo `DEFAULT_CREATION`:
/// **4096**), holding the value stack, every call frame's
/// result/function/this/frame slots, its arguments, and its scope in one
/// downward-growing region. XS's geometry is **width, not depth**: a
/// program overflows when the *total* concurrent slot count crosses the
/// bottom, so both deep recursion and a single very wide frame exhaust the
/// same fixed budget. Exhaustion aborts with
/// `XS_JAVASCRIPT_STACK_OVERFLOW_EXIT` (`fxOverflow` → `fxAbort`), an
/// abort to the host rather than a catchable `RangeError`. endor models
/// the same fixed geometry so a stack-exhausting program aborts on both
/// engines instead of endor's unbounded `Vec` completing where C-XS
/// overflows.
pub const STACK_SLOT_COUNT: usize = 4096;
/// XS reserves a fixed band at the top of the stack for the machine roots
/// (`mxGlobal`/`mxException`/`mxProgram`/… — the `*StackIndex` slots in
/// `xsAll.h`) plus the frame scratch `fxOverflow` guards against; the
/// usable value region is below them. Held as a small reserve so endor's
/// overflow point brackets C-XS's rather than overrunning it.
pub const STACK_SLOT_RESERVED: usize = 32;
/// The per-call fixed frame footprint XS keeps live on the stack for the
/// duration of a call: the `result`/`function`/`this`/`frame` quartet
/// (XS's `mxFrameResult`/`mxFrameFunction`/`mxFrameThis` at fixed offsets
/// around the frame slot). Arguments and scope slots are counted
/// separately.
pub const FRAME_OVERHEAD_SLOTS: usize = 4;

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

/// The fixed cost `fxRunConstructor` accrues over a plain call, beyond the
/// `this`-instance `fxNewSlot`: the `fxBeginHost`/`fxEndHost` host-frame
/// entry/exit around the prototype lookup and `fxNewHostInstance`. Measured
/// against the pin `48ee02d8cfe0` as exactly `2 × XS_CODE_METERING` (131072
/// raw) — the whole-computron gap between `new f()` and `f()` for an empty
/// constructor, independent of body or arity. Accrued once per constructor
/// entry at `begin`, in [`Interp::run_constructor`].
pub const CONSTRUCTOR_HOST_FRAME_METERING: u64 = 2 << 16;

/// Per-method raw 16.16 costs for the native prototype methods, measured
/// against the pin `48ee02d8cfe0` via the differential raw-gap. Each is the
/// method's cost beyond its call dispatch; the result-string chunk (for the
/// `toString` family) is metered separately at its `fxNewChunk`.
pub const METHOD_OBJECT_TOSTRING_METERING: u64 = 49216;
pub const METHOD_FUNCTION_TOSTRING_METERING: u64 = 131176;
pub const METHOD_ERROR_TOSTRING_METERING: u64 = 98360;
pub const METHOD_HAS_OWN_PROPERTY_METERING: u64 = 1 << 16;
/// The fixed re-dispatch overhead `Function.prototype.call` accrues beyond
/// the visible `.call` opcodes and the callee body (measured as `2<<16`),
/// plus one built-in step ([`CALL_TRAMPOLINE_PER_ARG`]) per forwarded
/// argument (XS copies each). Calibrated against the pin via the raw-gap.
pub const CALL_TRAMPOLINE_METERING: u64 = 2 << 16;
pub const CALL_TRAMPOLINE_PER_ARG: u64 = 1 << 14;

/// The raw 16.16 cost the `instanceof` operator accrues beyond its own
/// dispatch for the `Symbol.hasInstance` host-frame call itself
/// (`fxRunInstanceOf` → `fxOrdinaryHasInstance`), measured against the pin
/// `48ee02d8cfe0` as `2 × XS_CODE_METERING` — paid for every operand,
/// object or primitive.
pub const INSTANCEOF_METERING: u64 = 2 << 16;

/// The raw 16.16 cost the `in` operator accrues beyond its own dispatch when
/// the property is present: `fxRunIn` wraps `fxHasAt` in a host frame — one
/// code unit plus one built-in step. Measured against the pin `48ee02d8cfe0`
/// as exactly `(1<<16) + (1<<14)` (81920 raw), independent of the object.
pub const IN_METERING: u64 = (1 << 16) + (1 << 14);

/// The additional raw 16.16 cost when the left operand is an object:
/// `fxOrdinaryHasInstance` reads the constructor's `.prototype` and walks the
/// chain, whereas a primitive short-circuits to `false` before it. Measured
/// as a further `2 × XS_CODE_METERING`, independent of chain depth or result.
pub const INSTANCEOF_OBJECT_METERING: u64 = 2 << 16;

/// The raw 16.16 cost a primitive-wrapper constructor (`new Boolean`/
/// `new Number`/`new String`) accrues over the native `Object` constructor's
/// empty-object cost: the internal `[[XxxData]]` slot plus the wrap step.
/// Measured against the pin `48ee02d8cfe0` as the raw gap between
/// `new Boolean()` and `new Object()` = `(1<<16) + 256` (65792). Accrued in
/// [`Interp::build_wrapper`].
pub const WRAPPER_CONSTRUCT_EXTRA: u64 = (1 << 16) + 256;

/// The raw 16.16 cost of a `Symbol()` call (`fx_Symbol`/`fxNewSymbol`): the
/// symbol slot plus its registration. Measured against the pin `48ee02d8cfe0`
/// as 33792 raw, independent of the description. Accrued per `Symbol()` call.
pub const SYMBOL_CREATE_METERING: u64 = 33792;

/// The raw 16.16 cost an Error constructor accrues over the native `Object`
/// constructor's empty-object cost: the extra internal slots and steps an
/// error instance carries (`fx_Error`/`fxNewErrorInstance` — the stack-trace
/// capture and internal `[[ErrorData]]`). Measured against the pin
/// `48ee02d8cfe0` as the raw gap between `new Error()` and `new Object()` =
/// 66304 (one built-in step `1<<16` plus 768 for the extra slots). Accrued
/// in [`Interp::build_error`].
pub const ERROR_CONSTRUCT_EXTRA: u64 = (1 << 16) + 768;

/// The raw 16.16 cost of an Error's own `message` property when a message
/// argument is supplied (`fx_Error` defining `message`). Measured against the
/// pin as `new Error('x')` minus `new Error()` = 280 raw, independent of the
/// message length (the message string's own chunk is metered at its literal).
pub const ERROR_MESSAGE_METERING: u64 = 280;

/// The raw 16.16 cost the `XS_CODE_ARRAY` opcode accrues beyond its own
/// dispatch: `fxNewArray(the, 0)` runs `fxNewArrayInstance`
/// (`fxNewObjectInstance` — one instance `fxNewSlot` — plus one internal
/// `XS_ARRAY_KIND` behavior slot `fxNewSlot`: `2 × XS_SLOT_ALLOCATION_METERING`
/// = 512), plus one built-in step (`XS_BUILTIN_METERING` = 1<<14 = 16384) the
/// array-instance construction runs (`fxNewArrayInstance`/`fxIndexArray`) —
/// 16896 raw total. Isolated against the pin `48ee02d8cfe0` via a *second*
/// `arr.length = N` store (which the fuzz arm generated): the length
/// accessor-setter itself meters **nothing** beyond dispatch when it does not
/// resize the chunk ([`ARRAY_LENGTH_SET_METERING`] = 0), so the fixed
/// per-array constant that made array *literals* bit-exact belongs to the
/// `ARRAY` create, not to the literal's length prelude. Accrued in
/// [`Interp::new_array`].
pub const ARRAY_CREATE_METERING: u64 = 512 + (1 << 14);

/// The raw 16.16 cost of an `arr.length = N` store that does **not** resize
/// the item chunk (setting the length of an array with no live item chunk, or
/// to a value the chunk already spans). Measured against the pin
/// `48ee02d8cfe0` — isolated by a *second* `arr.length = N` store the fuzz arm
/// generated — as **zero** beyond the store's own dispatch: the fixed
/// per-array constant that makes literals bit-exact is the `ARRAY` create's
/// build step ([`ARRAY_CREATE_METERING`]), not the length set. (A length
/// store that *shrinks* an array with a live item chunk additionally reallocs
/// the chunk; that chunk metering is a later increment — the covered corpus
/// shrinks only hole/short arrays whose chunk is unaffected.)
pub const ARRAY_LENGTH_SET_METERING: u64 = 0;

/// The raw 16.16 cost of an `arr.length` read beyond its own dispatch.
/// Measured against the pin `48ee02d8cfe0` as **zero**: the length accessor
/// getter (`fxArrayLengthGetter`) returning the stored length adds no
/// built-in step or allocation over the `GET_PROPERTY` dispatch already
/// metered. Kept as a named constant so a future revision can revise it in
/// one place.
pub const ARRAY_LENGTH_GET_METERING: u64 = 0;

/// The raw 16.16 cost `NEW_PROPERTY_AT` accrues defining a fresh array item
/// beyond its dispatch and the item-chunk growth: one built-in step
/// (`fxRunDefine`'s `mxMeterOne`). Measured against the pin as `1 <<
/// 14` = 16384 (verified: an N-element literal's per-element raw delta is
/// exactly `5 × XS_CODE_METERING + 16384 + item_chunk_bytes`). The chunk
/// growth is metered separately by [`Interp::array_item_grow_metering`].
pub const ARRAY_ITEM_DEFINE_STEP_METERING: u64 = 1 << 14;

/// The fixed raw 16.16 cost of a dense `Array.prototype.push` call beyond the
/// per-item `mxMeterSome(5)`, the two bracketing `mxMeterSome(2)` steps, and
/// the item-chunk grow this stage already models: two further built-in steps
/// (`2 << 14` = 32768) the fast path runs unconditionally (host-frame /
/// `fxCheckArray` residual). Measured against the pin `48ee02d8cfe0` as the
/// constant raw-gap across a spread of receiver lengths and argument counts.
pub const ARRAY_PUSH_FRAME_METERING: u64 = 2 << 14;
/// The fixed raw 16.16 cost of a dense `Array.prototype.pop` call beyond its
/// modeled `mxMeterSome(2 + 8 + 4)` and the chunk shrink: **zero** (measured
/// bit-exact against the pin with no residual).
pub const ARRAY_POP_FRAME_METERING: u64 = 0;
/// The fixed frame cost of `Array.prototype.indexOf` (`2 << 14`) and its
/// per-element scan step (`5 << 14` = 81920, `mxMeterSome(5)` per compared
/// element). Measured against the pin: `gap = 32768 + 81920 × elements_scanned`
/// (scanning stops at the first strict-equal match).
pub const ARRAY_METHOD_INDEXOF_FRAME_METERING: u64 = 2 << 14;
pub const ARRAY_INDEXOF_PER_STEP: u64 = 5 << 14;
/// `Array.prototype.includes` frame + per-element scan step. Calibrated
/// against the pin `48ee02d8cfe0` via the completed-call raw-gap.
pub const ARRAY_INCLUDES_FRAME_METERING: u64 = 2 << 14;
pub const ARRAY_INCLUDES_PER_STEP: u64 = 3 << 14;
/// `Array.prototype.lastIndexOf` frame + per-element (backward) scan step.
pub const ARRAY_LASTINDEXOF_FRAME_METERING: u64 = 2 << 14;
pub const ARRAY_LASTINDEXOF_PER_STEP: u64 = 5 << 14;
/// `Array.prototype.fill` frame cost (the full-fill chunk realloc and the
/// per-element `mxMeterSome(5)` are metered separately). Calibrated.
pub const ARRAY_FILL_FRAME_METERING: u64 = 2 << 14;
/// `Array.prototype.slice` frame cost (the result array's `fxCreateArraySpecies`
/// + host frame + closing `mxMeterSome(3)`); a non-empty slice adds the result
/// chunk and `mxMeterSome(count*10)`. Calibrated against the pin.
pub const ARRAY_SLICE_FRAME_METERING: u64 = 377344;
/// `Array.prototype.at` frame cost + the in-range element read (`mxGetAt`).
/// Calibrated against the pin.
pub const ARRAY_AT_FRAME_METERING: u64 = 0;
pub const ARRAY_AT_READ_METERING: u64 = 98304;
/// `Array.prototype.reverse` frame cost + per-swap cost (each swap does
/// `mxHasAt`/`mxGetAt`×2/`mxSetAt`×2 over the generic path). Calibrated
/// against the pin.
pub const ARRAY_REVERSE_FRAME_METERING: u64 = 98304;
pub const ARRAY_REVERSE_PER_SWAP_METERING: u64 = 8 << 16;
/// `Array.prototype.unshift` fixed frame cost (`fxCheckArray` host frame),
/// beyond the grow chunk, `mxMeterSome(length*10)`, per-arg `mxMeterSome(4)`,
/// and closing `mxMeterSome(2)`. Measured against the pin as `2 << 14`. (shift
/// needs no such residual — its `mxMeterSome(2+3+3+4)` fully accounts for it.)
pub const ARRAY_UNSHIFT_FRAME_METERING: u64 = 2 << 14;
/// `Array.prototype.concat` frame cost + the `Symbol.isConcatSpreadable`
/// check per reference operand + the per-spread-element read and per-appended-
/// value residual (beyond the per-element/per-value key slot and `mxMeterSome`,
/// the result chunk, and the closing `mxMeterSome(3)`). Calibrated against the
/// pin by solving the linear system over a spread of operand shapes.
pub const ARRAY_CONCAT_FRAME_METERING: u64 = 311808;
pub const ARRAY_CONCAT_CHECK_METERING: u64 = 196608;
/// Extra raw per spread element (its `mxGetIndex`/`fxHasIndex` read), over the
/// key slot + `mxMeterSome(2)`.
pub const ARRAY_CONCAT_SPREAD_EXTRA_METERING: u64 = 98304;
/// Extra raw per appended non-array value, over the key slot + `mxMeterSome(4)`.
pub const ARRAY_CONCAT_PRIM_EXTRA_METERING: u64 = 2 << 14;
/// `Array.prototype.copyWithin` frame cost, beyond the `mxMeterSome(count*10)`
/// for the copied block. Calibrated against the pin.
pub const ARRAY_COPYWITHIN_FRAME_METERING: u64 = 98304;
/// `Array.prototype.with` frame cost + per-element copy over the generic
/// `mxGetAt`/`mxDefineAt` path (plus the result chunk). Calibrated against the
/// pin.
pub const ARRAY_WITH_FRAME_METERING: u64 = 66048;
pub const ARRAY_WITH_PER_ELEM_METERING: u64 = 10 << 14;
/// `Array.prototype.toReversed` frame cost (the same copy loop as `with`, one
/// code unit more of setup). Measured against the pin as 131584.
pub const ARRAY_TOREVERSED_FRAME_METERING: u64 = 131584;
/// `Array.prototype.splice` frame cost (`fxCreateArraySpecies` + host frame),
/// beyond the modeled result chunk, tail-shift, per-item, and per-`mxMeterSome`
/// costs. Calibrated against the pin.
pub const ARRAY_SPLICE_FRAME_METERING: u64 = 377344;
/// `Array.prototype.flat` frame cost (`fxCreateArraySpecies` + host frame, as
/// slice/splice), plus the per-appended-leaf cost (the visit read + the
/// `mxDefineIndex` step, `9 << 14`; the chunk growth is metered separately) and
/// the per-array-element cost (the visit read + the `.length` read before
/// recursing, `11 << 14`). Calibrated against the pin by solving the linear
/// system (the visit count is `leaves + arrays`, so two constants suffice).
pub const ARRAY_FLAT_FRAME_METERING: u64 = 377344;
pub const ARRAY_FLAT_PER_LEAF_METERING: u64 = 9 << 14;
pub const ARRAY_FLAT_PER_ARRAY_METERING: u64 = 11 << 14;
/// The per-source-element callback overhead of `flatMap` (`fxCallThisItem` in
/// `flatAux`'s function branch), beyond the callback body and the result
/// flattening (which reuses the `flat` constants). Calibrated against the pin.
pub const ARRAY_FLATMAP_CALLBACK_METERING: u64 = 6 << 14;
/// `Array.prototype.toSpliced` frame cost (`fxNewArray` host frame), beyond the
/// modeled result chunk and the per-region `mxMeterSome` copy costs
/// (`start * 10` for the head, `5` per insertion, `rest * 10` for the tail,
/// plus a trailing `4`). Non-mutating: the receiver is untouched. Calibrated
/// against the pin.
pub const ARRAY_TOSPLICED_FRAME_METERING: u64 = 131584;
/// `Array.prototype.toString` prelude cost beyond the delegated `join` body:
/// the `mxThis`/`mxDub`/`mxGetID(_join)` lookup plus the `mxCall`/`mxRunCount(0)`
/// call-frame setup that invokes `join`. Calibrated against the pin.
pub const ARRAY_TOSTRING_PRELUDE_METERING: u64 = 114688;
/// `Array.prototype.forEach` frame cost + the per-element `fxCallThisItem`
/// overhead (`mxGetIndex` + the callback call-frame setup), beyond the
/// callback body's own metering. Calibrated against the pin.
pub const ARRAY_FOREACH_FRAME_METERING: u64 = 8;
pub const ARRAY_FOREACH_PER_ELEM_METERING: u64 = 13 << 14;
/// Frame/per-element residuals for the other callback-taking methods, beyond
/// the shared per-element `fxCallThisItem` overhead
/// ([`ARRAY_FOREACH_PER_ELEM_METERING`]) and the callback body. Calibrated
/// against the pin.
pub const ARRAY_MAP_FRAME_METERING: u64 = 377352;
pub const ARRAY_SOMEEVERY_FRAME_METERING: u64 = 8;
pub const ARRAY_FILTER_FRAME_METERING: u64 = 328456;
pub const ARRAY_FILTER_KEEP_METERING: u64 = 65792;
/// The `fxToBoolean` of a predicate callback's result (`some`/`every`/`find`/
/// `filter`).
pub const ARRAY_PREDICATE_TOBOOL_METERING: u64 = 0;
/// `find`/`findIndex` use `fxFindThisItem` (calls the callback for every index,
/// holes included), a different per-element overhead than `fxCallThisItem`.
pub const ARRAY_FIND_FRAME_METERING: u64 = 8;
pub const ARRAY_FIND_PER_ELEM_METERING: u64 = 9 << 14;
/// `Array.prototype.reduce`/`reduceRight` frame + per-fold-step
/// `fxReduceThisItem` overhead (a 4-arg callback), beyond the callback body.
/// Calibrated against the pin.
pub const ARRAY_REDUCE_FRAME_METERING: u64 = 8;
pub const ARRAY_REDUCE_PER_ELEM_METERING: u64 = 13 << 14;
/// The seed-finding scan `reduce`/`reduceRight` runs when no initial value is
/// given: for a dense array the accumulator seeds from the first (or last)
/// present element in one iteration (`mxGetIndex` read), `6 << 14`.
pub const ARRAY_REDUCE_INIT_SCAN_METERING: u64 = 6 << 14;
/// The fixed backward-scan setup `findLast`/`findLastIndex` accrue over the
/// forward `find`/`findIndex`. Measured against the pin as `6 << 14`.
pub const ARRAY_FINDLAST_EXTRA_METERING: u64 = 6 << 14;
/// `Array.prototype.join` frame cost (the host frame + `fxGetArrayLimit` + the
/// result setup, beyond the modeled key-list/element-slot/ToString/final-chunk
/// allocations). Calibrated against the pin for the default (",") separator; a
/// non-default *string* separator argument carries a documented −24-raw
/// sub-computron residual (well under a `>> 16` boundary; every corpus/fuzz/
/// test262 check compares computrons and stays exact).
pub const ARRAY_JOIN_FRAME_METERING: u64 = 65560;
/// The per-element base cost `Array.prototype.join` accrues for every index
/// (the `mxGetIndex` read + loop overhead), on top of the element's ToString
/// allocation: `1 << 16`. Calibrated against the pin.
pub const ARRAY_JOIN_PER_ELEMENT_METERING: u64 = 1 << 16;

/// The constant raw 16.16 cost of an `Array(...)` / `new Array(...)` call
/// beyond the element item-chunk allocation: the native host frame,
/// `fxGetPrototypeFromConstructor`, and `fxNewArrayInstance`. Measured
/// against the pin `48ee02d8cfe0` as the constant raw-gap of `Array()` /
/// `Array(n)` / `new Array()` (no chunk), independent of the length; the
/// element forms add exactly one `array_chunk_size_metering(count)` on top.
/// (98816 = six built-in steps + the two `fxNewArrayInstance` slots; the
/// raw-gap the differential harness reports for a completed call, not the
/// larger figure a *halted* endor showed before the call was modeled.)
pub const ARRAY_CTOR_BASE_METERING: u64 = 98816;
/// The raw 16.16 cost of `Array.isArray(v)` beyond its dispatch: **zero**
/// (measured against the pin — the completed-call raw-gap, independent of the
/// argument).
pub const ARRAY_ISARRAY_METERING: u64 = 0;

/// The raw 16.16 cost of `Array.prototype.values()`/`keys()`/`entries()`
/// beyond its dispatch: the native host frame plus `fxNewIteratorInstance`
/// (the iterator instance + the reused `{value, done}` result object + the
/// internal kind/iterable/index slots — a fixed cluster of `fxNewSlot`s).
/// Calibrated against the pin `48ee02d8cfe0` via the completed-call raw-gap
/// (isolated from `next()` by comparing one- vs two-`next()` programs).
pub const ARRAY_ITERATOR_CREATE_METERING: u64 = 67592;
/// The base raw 16.16 cost of `%ArrayIteratorPrototype%.next()` beyond its
/// dispatch: the host frame, `fxCheckIteratorInstance`, and the result-object
/// mutation (the result object is reused, so `next()` allocates nothing for
/// kinds 0/1). A `values`/`entries` next that actually yields an element adds
/// one array-element read ([`ARRAY_ITERATOR_ELEMENT_READ`]). Calibrated
/// against the pin: `keys` next = 32768, `values` next = 65536.
pub const ARRAY_ITERATOR_NEXT_METERING: u64 = 2 << 14;
/// The extra raw 16.16 cost a `values`/`entries` `next()` accrues reading the
/// array element it yields (`mxGetIndex`), over a `keys` next: `2 << 14`.
pub const ARRAY_ITERATOR_ELEMENT_READ: u64 = 2 << 14;
/// The raw 16.16 cost of `XS_CODE_FOR_OF` (`fxRunForOf` → `fxGetIterator`)
/// beyond the `values()` iterator creation it performs: the `fxGetIterator`
/// host frame, the `arr[Symbol.iterator]` lookup, and the zero-argument call
/// dispatch. Calibrated against the pin `48ee02d8cfe0` via the completed
/// for-of loop raw-gap (the `values()` create cost itself is metered inside
/// [`Interp::make_array_iterator`]) — a constant `2 << 16`, independent of the
/// iterable's length.
pub const FOR_OF_GET_ITERATOR_METERING: u64 = 2 << 16;
/// The raw 16.16 cost of building a for-in enumerator (`XS_CODE_FOR_IN` →
/// `mxEnumeratorFunction` → `fx_Enumerator`): the enumerator + result objects,
/// the own-keys collection, and the host frame — a fixed cluster independent
/// of the key count (the per-key string allocation is metered in
/// [`ENUMERATOR_NEXT_METERING`] + the key chunk). Calibrated against the pin
/// for an empty ordinary-object enumeration; an array adds
/// [`ARRAY_FOR_IN_EXTRA_METERING`]. Known sub-computron residual: the exact
/// non-empty keys-list handling carries a ±8-raw chunk-alignment gap
/// (analogous to the array-spread residual) that is well under one computron
/// and never crosses a `>> 16` boundary in a bounded program, so the
/// computron-level bar (which every corpus/fuzz/test262 check uses) stays
/// exact; modeling the keys-instance chunk capacity to close it is a later
/// refinement.
pub const FOR_IN_ENUMERATOR_METERING: u64 = 202248;
/// The extra raw 16.16 cost of a for-in enumerator over an **array** (vs an
/// ordinary object): `mxBehaviorOwnKeys` for an exotic array (`fxArrayOwnKeys`
/// queuing the index keys) does more than `fxOrdinaryOwnKeys`. Measured
/// against the pin as a constant, independent of the element count.
pub const ARRAY_FOR_IN_EXTRA_METERING: u64 = 10488;
/// The base raw 16.16 cost of a yielding `fx_Enumerator_prototype_next` beyond
/// the yielded key's own string-chunk allocation. Calibrated against the pin.
pub const ENUMERATOR_NEXT_METERING: u64 = (2 << 14) + 256;

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
    /// For an intrinsic (native) function — a `Some` marks this instance a
    /// C-backed built-in (XS's `XS_CALLBACK_KIND`) rather than a bytecode
    /// function: `call`/`run` dispatches to the native handler instead of
    /// entering a bytecode frame, and the completion renders as
    /// `function ["name"] (){[native code]}`. `None` for a user function.
    native: Option<Native>,
    /// For a native **prototype method** (`Object.prototype.toString`,
    /// `Function.prototype.toString`, `Error.prototype.toString`, the wrapper
    /// `valueOf`/`toString`, …): dispatched with the call's receiver as
    /// `this`. `None` for a constructor or a user function.
    method: Option<NativeMethod>,
    /// The function's own name (for `Function.prototype.toString`), an empty
    /// string for an anonymous function.
    name: String,
}

/// A native prototype method endor models (dispatched with the receiver as
/// `this`). These compute a value from the receiver with no re-entry into
/// user code — the `call`/`apply`/`bind` re-entrant methods are separate.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NativeMethod {
    ObjectToString,
    ObjectHasOwnProperty,
    ObjectValueOf,
    ObjectIsPrototypeOf,
    FunctionToString,
    /// `Function.prototype.call` — a re-entrant trampoline: invoke the
    /// receiver function with the first argument as `this` and the rest as
    /// its arguments. Handled specially in the `run` dispatch (it re-enters
    /// the interpreter frame machinery rather than computing a value).
    FunctionCall,
    /// `Function.prototype.apply` — like `call`, but the arguments come from
    /// an array. endor models the no-array subset (`f.apply(thisArg)` /
    /// `f.apply(thisArg, null|undefined)`), identical to `call` with no
    /// arguments; an actual arguments array self-names (the Array read is
    /// child-3 machinery).
    FunctionApply,
    ErrorToString,
    /// A primitive wrapper's `valueOf` (returns the wrapped primitive).
    WrapperValueOf,
    /// A primitive wrapper's `toString` (stringifies the wrapped primitive).
    WrapperToString,
    /// `Array.prototype.push(...items)` — the **dense** fast path
    /// (`fx_Array_prototype_push` with `fxCheckArray` succeeding): append the
    /// arguments and return the new length. A sparse receiver (holes) takes
    /// XS's generic slow path (different metering), so endor self-names it an
    /// honest skip.
    ArrayPush,
    /// `Array.prototype.pop()` — the dense fast path
    /// (`fx_Array_prototype_pop`): remove and return the last element (or
    /// `undefined` on an empty array), shrinking the item chunk.
    ArrayPop,
    /// `Array.prototype.indexOf(value[, from])` — the dense fast path
    /// (`fx_Array_prototype_indexOf`): the first index at which `value` is
    /// found by strict equality, or `-1`.
    ArrayIndexOf,
    /// `Array.prototype.join([sep])` — the dense fast path
    /// (`fx_Array_prototype_join`): the elements stringified and joined by
    /// `sep` (default `","`), holes/`undefined`/`null` contributing empty.
    ArrayJoin,
    /// `Array.prototype.includes(value[, from])` — dense fast path
    /// (`fx_Array_prototype_includes`): whether `value` is an element (by
    /// SameValueZero), scanning from `from`.
    ArrayIncludes,
    /// `Array.prototype.lastIndexOf(value[, from])` — dense fast path: the last
    /// index at which `value` is found (strict equality) scanning backward, or
    /// `-1`.
    ArrayLastIndexOf,
    /// `Array.prototype.fill(value[, start[, end]])` — dense fast path
    /// (`fx_Array_prototype_fill`): set `[start, end)` to `value`, returning
    /// the array.
    ArrayFill,
    /// `Array.prototype.reverse()` — dense fast path
    /// (`fx_Array_prototype_reverse`): reverse the elements in place, returning
    /// the array.
    ArrayReverse,
    /// `Array.prototype.slice([start[, end]])` — dense fast path
    /// (`fx_Array_prototype_slice`): a new array with the elements of
    /// `[start, end)`.
    ArraySlice,
    /// `Array.prototype.concat(...args)` — dense fast path
    /// (`fx_Array_prototype_concat`): a new array of the receiver's elements
    /// followed by each argument (spreading array arguments).
    ArrayConcat,
    /// `Array.prototype.at(index)` — dense fast path (`fx_Array_prototype_at`):
    /// the element at `index` (negative counts from the end), or `undefined`.
    ArrayAt,
    /// `Array.prototype.shift()` — dense fast path (`fx_Array_prototype_shift`):
    /// remove and return the first element, shifting the rest down.
    ArrayShift,
    /// `Array.prototype.unshift(...items)` — dense fast path
    /// (`fx_Array_prototype_unshift`): prepend the arguments, returning the new
    /// length.
    ArrayUnshift,
    /// `Array.prototype.copyWithin(target[, start[, end]])` — dense fast path
    /// (`fx_Array_prototype_copyWithin`): copy the block `[start, end)` to
    /// `target` in place, returning the array.
    ArrayCopyWithin,
    /// `Array.prototype.with(index, value)` (`fx_Array_prototype_with`): a new
    /// array copying the receiver with `index` replaced by `value` (negative
    /// index counts from the end; out-of-range is a RangeError).
    ArrayWith,
    /// `Array.prototype.forEach(callback[, thisArg])`
    /// (`fx_Array_prototype_forEach`): call `callback(item, index, array)` for
    /// each present element; returns `undefined`. The first re-entrant method
    /// (drives a user callback per element via [`Interp::run_callback`]).
    ArrayForEach,
    /// `Array.prototype.map(callback[, thisArg])`: a new array of the callback
    /// results, one per element.
    ArrayMap,
    /// `Array.prototype.some(callback[, thisArg])`: `true` if the callback is
    /// truthy for any element (short-circuits).
    ArraySome,
    /// `Array.prototype.every(callback[, thisArg])`: `true` if the callback is
    /// truthy for every element (short-circuits on the first falsy).
    ArrayEvery,
    /// `Array.prototype.find(callback[, thisArg])`: the first element for which
    /// the callback is truthy, or `undefined`.
    ArrayFind,
    /// `Array.prototype.findIndex(callback[, thisArg])`: the index of the first
    /// element for which the callback is truthy, or `-1`.
    ArrayFindIndex,
    /// `Array.prototype.filter(callback[, thisArg])`: a new array of the
    /// elements for which the callback is truthy.
    ArrayFilter,
    /// `Array.prototype.reduce(callback[, initial])`: fold left with
    /// `callback(acc, item, index, array)`.
    ArrayReduce,
    /// `Array.prototype.reduceRight(callback[, initial])`: fold right.
    ArrayReduceRight,
    /// `Array.prototype.findLast(callback[, thisArg])`: the last element for
    /// which the callback is truthy, or `undefined`.
    ArrayFindLast,
    /// `Array.prototype.findLastIndex(callback[, thisArg])`: the index of the
    /// last element for which the callback is truthy, or `-1`.
    ArrayFindLastIndex,
    /// `Array.prototype.toReversed()` (`fx_Array_prototype_toReversed`): a new
    /// array with the receiver's elements reversed (non-mutating).
    ArrayToReversed,
    /// `Array.prototype.splice(start[, deleteCount, ...items])`
    /// (`fx_Array_prototype_splice`): remove `deleteCount` elements at `start`
    /// and insert `items`, returning a new array of the removed elements.
    ArraySplice,
    /// `Array.prototype.flat([depth])` (`fx_Array_prototype_flat`): a new array
    /// with sub-array elements flattened to `depth` (default 1).
    ArrayFlat,
    /// `Array.prototype.flatMap(callback[, thisArg])`
    /// (`fx_Array_prototype_flatMap`): map then flatten by one level.
    ArrayFlatMap,
    /// `Array.prototype.toSpliced(start, deleteCount, ...items)`
    /// (`fx_Array_prototype_toSpliced`): a non-mutating splice into a new array.
    ArrayToSpliced,
    /// `Array.prototype.toString()` (`fx_Array_prototype_toString`): delegates
    /// to `this.join()` with the default separator (spec 23.1.3.36).
    ArrayToString,
    /// `Array.prototype.sort([cmp])` — an honest named skip: the quicksort's
    /// comparison count (and thus its metering) is data- and comparator-
    /// dependent, so it is not modeled to a clean constant this stage.
    ArraySort,
    /// `Array.prototype.toSorted([cmp])` — an honest named skip for the same
    /// data-dependent-comparison reason as `sort`.
    ArrayToSorted,
    /// `Array.prototype.toLocaleString()` — an honest named skip (locale-aware
    /// element stringification is out of this stage's scope).
    ArrayToLocaleString,
    /// `Array.from(iterable[, mapFn[, thisArg]])` — an honest named skip: the
    /// C-level `fxGetIterator`/`fxIteratorNext` protocol metering (routing
    /// through `%ArrayIteratorPrototype%.next` via `mxRunCount`) is not modeled
    /// to a clean per-element constant this stage.
    ArrayFrom,
    /// `Array.fromAsync(...)` — an honest named skip (returns a Promise; async
    /// iteration is stage-4+ territory).
    ArrayFromAsync,
    /// `Array.isArray(v)` — a static on the `Array` constructor: whether `v`
    /// is an array exotic object.
    ArrayIsArray,
    /// `Array.of(...items)` — a static: a new array whose elements are the
    /// arguments (`fx_Array_of`, always elements, never a length).
    ArrayOf,
    /// `Array.prototype.values()` / `keys()` / `entries()`
    /// (`fx_Array_prototype_values` &co.): construct an Array Iterator over the
    /// receiver with the given kind (0 values / 1 keys / 2 entries).
    ArrayValues,
    ArrayKeys,
    ArrayEntries,
    /// `%ArrayIteratorPrototype%.next()` (`fx_ArrayIterator_prototype_next`):
    /// yield the next `{value, done}` (mutating and returning the iterator's
    /// reused result object).
    ArrayIteratorNext,
}

impl Default for FuncInfo {
    fn default() -> Self {
        FuncInfo {
            body_start: 0,
            body_len: 0,
            closures: crate::value::SlotIndex::NULL,
            native: None,
            method: None,
            name: String::new(),
        }
    }
}

/// An exotic array's data (XS's `XS_ARRAY_KIND` internal slot). `length`
/// is the array length (`fxArraySetLength` semantics); `items` holds the
/// present elements sparsely by index (an absent index in `[0, length)` is
/// a hole). A `BTreeMap` keeps the indices ordered so `for-in` enumeration
/// and `Array.prototype` iteration visit them in ascending index order,
/// matching XS's item-chunk order.
#[derive(Clone, Debug, Default)]
struct ArrayData {
    length: u32,
    items: std::collections::BTreeMap<u32, Slot>,
}

/// An iterator's state. For an **array iterator** (`kind` 0 = values, 1 =
/// keys, 2 = entries) `iterable` is the array and `index` the cursor. For a
/// **for-in enumerator** (`kind` = 3) `enum_keys` is the pre-collected list of
/// enumerable property keys `(id, index)` to yield as strings (an `id ==
/// XS_NO_ID` entry is an array index), and `index` cursors it. `result` is the
/// reused `{value, done}` object `next()` mutates and returns.
#[derive(Clone, Debug)]
struct IterState {
    iterable: crate::value::SlotIndex,
    index: u32,
    kind: u8,
    result: crate::value::SlotIndex,
    done: bool,
    enum_keys: Vec<(u16, u32)>,
}

/// An Error instance's stringification data (XS's `Error.prototype.toString`
/// inputs): the constructor's `name` and the optional own `message`.
#[derive(Clone, Debug)]
struct ErrorInfo {
    name: &'static str,
    message: Option<String>,
}

/// One intrinsic (native) function endor models. The variant is the
/// identity the `run`/`new` dispatch and the completion renderer key off;
/// [`Native::display_name`] is the name XS's `Function.prototype.toString`
/// prints for it (`function ["Object"] (){[native code]}`).
///
/// The **fundamentals** built-ins (stage-3 child 2): the constructors and
/// the Error hierarchy the design's stage-3 decomposition names. A bare
/// reference and `typeof` are modeled for every variant (both are pure
/// dispatch, bit-exact); the *call* and *construct* behaviors land
/// incrementally, and an unmodeled one self-names [`Halt::Unsupported`]
/// (an honest skip) rather than mis-executing.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Native {
    Object,
    Function,
    Boolean,
    Symbol,
    Number,
    String,
    Array,
    Error,
    EvalError,
    RangeError,
    ReferenceError,
    SyntaxError,
    TypeError,
    URIError,
    AggregateError,
}

impl Native {
    /// The name XS prints for this built-in (its `name` property, shown by
    /// `Function.prototype.toString` and by the completion renderer).
    pub fn display_name(self) -> &'static str {
        match self {
            Native::Object => "Object",
            Native::Function => "Function",
            Native::Boolean => "Boolean",
            Native::Symbol => "Symbol",
            Native::Number => "Number",
            Native::String => "String",
            Native::Array => "Array",
            Native::Error => "Error",
            Native::EvalError => "EvalError",
            Native::RangeError => "RangeError",
            Native::ReferenceError => "ReferenceError",
            Native::SyntaxError => "SyntaxError",
            Native::TypeError => "TypeError",
            Native::URIError => "URIError",
            Native::AggregateError => "AggregateError",
        }
    }

    /// The intrinsic global constructors endor binds, in `(name, variant)`
    /// pairs. The name is what the C-XS compiler records in the symbols
    /// atom; [`Interp::link_intrinsics`] binds each to the program-local id
    /// the compiler assigned it.
    pub fn intrinsics() -> &'static [(&'static str, Native)] {
        &[
            ("Object", Native::Object),
            ("Function", Native::Function),
            ("Boolean", Native::Boolean),
            ("Symbol", Native::Symbol),
            ("Number", Native::Number),
            ("String", Native::String),
            ("Array", Native::Array),
            ("Error", Native::Error),
            ("EvalError", Native::EvalError),
            ("RangeError", Native::RangeError),
            ("ReferenceError", Native::ReferenceError),
            ("SyntaxError", Native::SyntaxError),
            ("TypeError", Native::TypeError),
            ("URIError", Native::URIError),
            ("AggregateError", Native::AggregateError),
        ]
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
    /// The value stack was exhausted (XS's `fxOverflow` →
    /// `fxAbort(XS_JAVASCRIPT_STACK_OVERFLOW_EXIT)`): a fixed-geometry
    /// stack overflow. Like XS's, this is an **abort to the host**, not a
    /// catchable `RangeError` — a deterministic, consensus-relevant limit
    /// in the xsnap lineage. Carries the slot count over the limit for
    /// diagnostics.
    StackOverflow(usize),
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
    /// The interned `typeof` result strings (XS's `mxUndefinedString`
    /// &co.), allocated once at construction so `typeof` is dispatch-only.
    static_str: StaticStrings,
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
    /// Whether the active frame is a **constructor** invocation (XS's
    /// `mxFrameHasTarget` — a `new f(...)`). Set when `run` enters a callee
    /// whose `THIS` slot is the uninitialized construct placeholder; drives
    /// `begin`'s `fxRunConstructor` (allocate the `this` instance) and the
    /// construct return semantics at `end` (a non-object completion yields
    /// `this`). `false` for a plain call and the program frame.
    cur_target: bool,
    /// The pending thrown value (XS's `mxException`). `THROW` sets it and
    /// unwinds to the innermost jump; `EXCEPTION` moves it to the stack
    /// (binding the catch parameter) and clears it back to `undefined`;
    /// `RETHROW` re-unwinds with it. Default `undefined`.
    exception: Slot,
    /// Running total of slots held by the **suspended** call frames (the
    /// `call_stack` activations): each contributes its
    /// [`FRAME_OVERHEAD_SLOTS`] plus its saved argument and scope slots.
    /// Combined with the active frame's live slots
    /// ([`Self::live_stack_slots`]), this mirrors XS's `stackTop - stack`
    /// so the stack-overflow abort fires at the same fixed-geometry budget.
    frame_slots: usize,
    /// The intrinsic (native) constructors, keyed by name, created once at
    /// construction (an unmetered machine-boot cost, as XS builds its
    /// intrinsics before the guest runs). [`Self::link_intrinsics`] binds
    /// each into the global object under the program-local symbol id the
    /// C-XS compiler assigned that name. Each value is a `functions`-tracked
    /// native function instance.
    intrinsics: std::collections::HashMap<&'static str, crate::value::SlotIndex>,
    /// The realm's `%Object.prototype%` (XS's `mxObjectPrototype`), the root
    /// of every ordinary object's prototype chain. A boot object; ordinary
    /// objects ([`Self::new_object`]) and constructed `this` instances point
    /// their prototype at it (or a subclass prototype), which is what
    /// `instanceof` walks. Property *lookup* is unchanged (own-only) — the
    /// prototype objects carry no data properties — so this is invisible to
    /// the existing corpora; only the prototype *identity* chain is new.
    object_proto: crate::value::SlotIndex,
    /// The realm's `%Function.prototype%`: the prototype of every function
    /// instance (native and user), so `f.toString`/`f.call`/… resolve up the
    /// chain. A boot object.
    function_proto: crate::value::SlotIndex,
    /// Each constructor instance's `.prototype` object, by slot (XS's
    /// `constructor.prototype`): the intrinsics' prototypes (wired at boot)
    /// and every user function's default prototype (wired at
    /// `constructor_function`). `fxRunConstructor` reads it to set the new
    /// `this`'s prototype, and `instanceof` reads it as the right-hand test
    /// object — so `(new F()) instanceof F` and `err instanceof TypeError`
    /// are prototype-chain identity checks (`fxOrdinaryHasInstance`).
    ctor_prototype: std::collections::HashMap<crate::value::SlotIndex, crate::value::SlotIndex>,
    /// Native prototype methods to bind at link time: `(prototype instance,
    /// method name, method function)`. Populated once at boot; a method is
    /// installed as an own property of its prototype only when the program
    /// references its name (so it relinks to the program-local symbol id and
    /// stays invisible to programs that never mention it).
    proto_methods: Vec<(crate::value::SlotIndex, &'static str, crate::value::SlotIndex)>,
    /// Native prototype **data** properties to bind at link time: `(prototype,
    /// property name, string value)`. Used for the inherited Error prototype
    /// `name`/`message` (so `err.name` resolves up the chain and
    /// `err.hasOwnProperty('name')` is correctly `false`, matching XS). Bound
    /// only when the program references the name; unmetered.
    proto_data: Vec<(crate::value::SlotIndex, &'static str, String)>,
    /// The well-known symbols (`Symbol.iterator`, `Symbol.hasInstance`, …) as
    /// `(name, symbol value)` — fixed `Kind::Symbol` values created once at
    /// boot and bound as own properties of the `Symbol` constructor at link
    /// time (only when referenced), so `Symbol.iterator === Symbol.iterator`.
    well_known_symbols: Vec<(&'static str, Slot)>,
    /// The program's symbol `name → id` table, built at
    /// [`Self::link_intrinsics`] from the decoded symbols atom (the inverse
    /// of the id→name vector). A native built-in that must set a
    /// well-known-named own property (`message`/`name` on an Error) looks up
    /// the program-local id here — XS uses a fixed global symbol id
    /// (`mxID(_message)`), which endor's program-local numbering must relink
    /// against, exactly as the intrinsic constructors relink by name. A name
    /// the program never references has no id (and no read of it occurs).
    symbol_ids: std::collections::HashMap<String, u16>,
    /// The program's symbol names indexed by `id - 1` (the decoded symbols
    /// atom, verbatim), so a function definition can recover its own name
    /// string for `Function.prototype.toString`.
    symbol_names: Vec<String>,
    /// Per-instance Error metadata (name + message), keyed by the error
    /// instance's slot index. An Error object's completion/abort value
    /// stringifies as `name` (no/empty message) or `name: message` — XS's
    /// `Error.prototype.toString`. Kept here so [`Self::render`] produces the
    /// exact abort value without a symbol-id lookup, graduating abort-value
    /// parity from primitive throws to real Error objects.
    error_data: std::collections::HashMap<crate::value::SlotIndex, ErrorInfo>,
    /// Per-instance primitive-wrapper data (`new Boolean`/`Number`/`String`),
    /// keyed by the wrapper instance's slot: the wrapped primitive slot
    /// (XS's `[[BooleanData]]`/`[[NumberData]]`/`[[StringData]]`). A wrapper's
    /// completion/`String()` stringifies as its wrapped primitive, so
    /// [`Self::render`] reads it here.
    wrapper_data: std::collections::HashMap<crate::value::SlotIndex, Slot>,
    /// The realm's `%Array.prototype%` (a boot object). Every array literal
    /// and `new Array` instance chains to it, so `arr.push`/`arr.join`/… (the
    /// native methods bound on it) resolve up the prototype chain.
    array_proto: crate::value::SlotIndex,
    /// Per-instance array data (XS's exotic array's `XS_ARRAY_KIND` internal
    /// slot: `length` plus the item chunk). Keyed by the array instance's
    /// slot. `length` is the array length semantics of `fxArraySetLength`;
    /// `items` holds the present (non-hole) elements sparsely by index —
    /// an absent index in `[0, length)` is a hole. Kept in a side table like
    /// [`Self::error_data`]/[`Self::wrapper_data`]; no mid-run GC runs, so the
    /// item value slots (which may be references) are never swept underneath
    /// it (the stage-2 GC roots contract).
    arrays: std::collections::HashMap<crate::value::SlotIndex, ArrayData>,
    /// The program-local symbol id of `length`, resolved at
    /// [`Self::link_intrinsics`] (XS's `mxID(_length)`), so an
    /// `arr.length` get/set routes to the array length semantics. `None`
    /// when the program never references `length`.
    length_id: Option<u16>,
    /// The realm's `%Array Iterator.prototype%` (a boot object) — the
    /// prototype of the iterators `arr.values()`/`keys()`/`entries()` and
    /// `arr[Symbol.iterator]()` produce. Carries `next` and a
    /// `Symbol.iterator` returning the iterator itself.
    array_iterator_proto: crate::value::SlotIndex,
    /// Per-instance array-iterator state (XS's `fxNewIteratorInstance`
    /// internal slots): the array being iterated, the next index to yield,
    /// the iteration `kind` (0 = values, 1 = keys, 2 = entries), and the
    /// **reused** result object (`{value, done}`) `next()` mutates and returns
    /// — XS allocates it once at iterator creation, not per `next()`.
    iterators: std::collections::HashMap<crate::value::SlotIndex, IterState>,
    /// The program-local symbol ids of `value`/`done`, resolved at
    /// [`Self::link_intrinsics`], so `next()` sets them on the result object
    /// under the ids the program reads them by.
    value_id: Option<u16>,
    done_id: Option<u16>,
    /// The jump-buffer chain (XS's `the->firstJump`), innermost last.
    /// `CATCH` pushes a [`CatchJump`]; `UNCATCH` pops it; `THROW`/`RETHROW`
    /// unwind to the top entry, restoring the value stack, scope, and call
    /// frames it recorded, then resume at its target. An empty chain means
    /// the throw escapes every JS handler and propagates to the host
    /// boundary as [`Halt::Throw`] — the JS/host flag reduced to a
    /// structural predicate (every `self.jumps` entry is a JS jump,
    /// XS's `jump->flag = 1`; the host is the absence of a jump).
    jumps: Vec<CatchJump>,
}

/// The interned `typeof`-result strings, held as chunk offsets into the
/// machine chunk heap. Allocated once at [`Interp::new`], before any run,
/// so `typeof` names a preexisting string (XS's `XS_STRING_X_KIND`
/// interned strings) rather than allocating — dispatch-only, as C-XS.
#[derive(Copy, Clone)]
struct StaticStrings {
    undefined: crate::value::ChunkOffset,
    object: crate::value::ChunkOffset,
    boolean: crate::value::ChunkOffset,
    number: crate::value::ChunkOffset,
    string: crate::value::ChunkOffset,
    function: crate::value::ChunkOffset,
    symbol: crate::value::ChunkOffset,
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
    cur_target: bool,
    /// The caller's code cursor to resume at (just past its `run`).
    ret_pc: usize,
}

/// One entry of the exception jump-buffer chain (XS's `txJump`, pushed by
/// `CATCH`). It records exactly what XS's `c_setjmp` restore restores when
/// a throw longjmps here: where to resume (`target_pc`, XS's `jump->code`),
/// the value-stack cut (`stack_len`, XS's `jump->stack`), the scope cut
/// (`locals_len`/`id_map`, XS's `jump->scope`/environment), and the call
/// depth to unwind to (`call_depth`, XS's `jump->frame` — a throw that
/// crosses called functions pops their activations back to the frame that
/// established the catch). `flag` mirrors XS's `jump->flag = 1` (a JS
/// jump); every endor jump is JS, and the host boundary is the empty chain.
struct CatchJump {
    target_pc: usize,
    stack_len: usize,
    locals_len: usize,
    id_map: std::collections::HashMap<u16, usize>,
    call_depth: usize,
    flag: u8,
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
        // Intern the `typeof` result strings (XS's `mxUndefinedString`
        // &co. — preexisting `XS_STRING_X_KIND` slots), allocated into the
        // chunk arena *before* any run so `typeof` costs only its dispatch,
        // exactly as C-XS (no per-use allocation for an interned string).
        let mut chunks = ChunkArena::new();
        let static_str = StaticStrings {
            undefined: chunks.alloc(b"undefined"),
            object: chunks.alloc(b"object"),
            boolean: chunks.alloc(b"boolean"),
            number: chunks.alloc(b"number"),
            string: chunks.alloc(b"string"),
            function: chunks.alloc(b"function"),
            symbol: chunks.alloc(b"symbol"),
        };
        let mut interp = Interp {
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
            chunks,
            static_str,
            n_dispatched: 0,
            functions: std::collections::HashMap::new(),
            call_stack: Vec::new(),
            args: Vec::new(),
            this_val: Slot::undefined(),
            cur_func: crate::value::SlotIndex::NULL,
            cur_target: false,
            exception: Slot::undefined(),
            frame_slots: 0,
            intrinsics: std::collections::HashMap::new(),
            object_proto: crate::value::SlotIndex::NULL,
            function_proto: crate::value::SlotIndex::NULL,
            ctor_prototype: std::collections::HashMap::new(),
            proto_methods: Vec::new(),
            proto_data: Vec::new(),
            well_known_symbols: Vec::new(),
            symbol_ids: std::collections::HashMap::new(),
            symbol_names: Vec::new(),
            error_data: std::collections::HashMap::new(),
            wrapper_data: std::collections::HashMap::new(),
            array_proto: crate::value::SlotIndex::NULL,
            arrays: std::collections::HashMap::new(),
            length_id: None,
            array_iterator_proto: crate::value::SlotIndex::NULL,
            iterators: std::collections::HashMap::new(),
            value_id: None,
            done_id: None,
            jumps: Vec::new(),
        };
        interp.create_intrinsics();
        interp
    }

    /// Materialize the intrinsic (native) constructor instances once, at
    /// machine boot, before any guest bytecode runs — so, like XS's
    /// intrinsic construction, they carry **no** run-only metering. Each is
    /// a real arena instance registered in [`Self::functions`] with its
    /// [`Native`] marker (so `typeof` reads "function" and `run`/`new`
    /// dispatch to the native handler), and remembered by name in
    /// [`Self::intrinsics`] for per-program linking.
    fn create_intrinsics(&mut self) {
        // The prototype roots: %Object.prototype% (null proto) and
        // %Function.prototype% (chains to it). Every native constructor is a
        // callable whose own prototype is %Function.prototype%.
        let object_proto = self.slots.alloc(Slot::instance(crate::value::SlotIndex::NULL));
        self.object_proto = object_proto;
        let func_proto = self.slots.alloc(Slot::instance(object_proto));
        self.function_proto = func_proto;
        // Each Error type's `.prototype`: the base `%Error.prototype%` chains
        // to %Object.prototype%; each subtype's prototype chains to
        // %Error.prototype% (so `TypeError` `instanceof Error`).
        let error_proto = self.slots.alloc(Slot::instance(object_proto));
        for &(name, native) in Native::intrinsics() {
            let f = self.slots.alloc(Slot::instance(func_proto));
            self.functions.insert(
                f,
                FuncInfo {
                    native: Some(native),
                    ..FuncInfo::default()
                },
            );
            self.intrinsics.insert(name, f);
            // Wire the constructor's `.prototype` object (the `instanceof`
            // right-hand test / the `new` this-prototype). Object and
            // Function reuse the two prototype roots; the Error base reuses
            // `%Error.prototype%`; every subtype gets a prototype chaining to
            // it; the wrapper constructors get a plain `%X.prototype%`.
            let proto = match native {
                Native::Object => object_proto,
                Native::Function => func_proto,
                Native::Error => error_proto,
                Native::EvalError
                | Native::RangeError
                | Native::ReferenceError
                | Native::SyntaxError
                | Native::TypeError
                | Native::URIError
                | Native::AggregateError => self.slots.alloc(Slot::instance(error_proto)),
                Native::Boolean
                | Native::Symbol
                | Native::Number
                | Native::String => self.slots.alloc(Slot::instance(object_proto)),
                // `%Array.prototype%` is itself an (empty) exotic array in XS;
                // endor models it as an ordinary boot object chaining to
                // %Object.prototype% (its own array-ness is unobservable to the
                // covered grammar, which never reads `Array.prototype.length`).
                Native::Array => self.slots.alloc(Slot::instance(object_proto)),
            };
            self.ctor_prototype.insert(f, proto);
        }
        // Remember `%Array.prototype%` — every array literal / `new Array`
        // instance chains to it so its methods resolve up the chain.
        self.array_proto = self
            .intrinsics
            .get("Array")
            .and_then(|&c| self.ctor_prototype.get(&c).copied())
            .unwrap_or(crate::value::SlotIndex::NULL);
        // The `Array.prototype` methods endor models (dense fast paths), bound
        // as own properties of `%Array.prototype%` at link time only when the
        // program references the method name.
        for (name, m) in [
            ("push", NativeMethod::ArrayPush),
            ("pop", NativeMethod::ArrayPop),
            ("indexOf", NativeMethod::ArrayIndexOf),
            ("join", NativeMethod::ArrayJoin),
            ("values", NativeMethod::ArrayValues),
            ("keys", NativeMethod::ArrayKeys),
            ("entries", NativeMethod::ArrayEntries),
            ("includes", NativeMethod::ArrayIncludes),
            ("lastIndexOf", NativeMethod::ArrayLastIndexOf),
            ("fill", NativeMethod::ArrayFill),
            ("reverse", NativeMethod::ArrayReverse),
            ("slice", NativeMethod::ArraySlice),
            ("concat", NativeMethod::ArrayConcat),
            ("at", NativeMethod::ArrayAt),
            ("shift", NativeMethod::ArrayShift),
            ("unshift", NativeMethod::ArrayUnshift),
            ("copyWithin", NativeMethod::ArrayCopyWithin),
            ("with", NativeMethod::ArrayWith),
            ("forEach", NativeMethod::ArrayForEach),
            ("map", NativeMethod::ArrayMap),
            ("some", NativeMethod::ArraySome),
            ("every", NativeMethod::ArrayEvery),
            ("find", NativeMethod::ArrayFind),
            ("findIndex", NativeMethod::ArrayFindIndex),
            ("filter", NativeMethod::ArrayFilter),
            ("reduce", NativeMethod::ArrayReduce),
            ("reduceRight", NativeMethod::ArrayReduceRight),
            ("findLast", NativeMethod::ArrayFindLast),
            ("findLastIndex", NativeMethod::ArrayFindLastIndex),
            ("toReversed", NativeMethod::ArrayToReversed),
            ("splice", NativeMethod::ArraySplice),
            ("flat", NativeMethod::ArrayFlat),
            ("flatMap", NativeMethod::ArrayFlatMap),
            ("toSpliced", NativeMethod::ArrayToSpliced),
            ("toString", NativeMethod::ArrayToString),
            // Recognized-but-unimplemented methods, bound so a reference is an
            // honest NAMED skip (`Halt::Unsupported`) rather than a completion
            // divergence (`this.M is not a function`) or a wrong value.
            ("sort", NativeMethod::ArraySort),
            ("toSorted", NativeMethod::ArrayToSorted),
            ("toLocaleString", NativeMethod::ArrayToLocaleString),
        ] {
            let mf = self.alloc_method(m);
            self.proto_methods.push((self.array_proto, name, mf));
        }
        // `%Array Iterator.prototype%`: a boot object chaining to
        // %Object.prototype%, carrying `next` (the iterators produced by
        // `values`/`keys`/`entries` chain to it).
        let array_iter_proto = self.slots.alloc(Slot::instance(object_proto));
        self.array_iterator_proto = array_iter_proto;
        let next_mf = self.alloc_method(NativeMethod::ArrayIteratorNext);
        self.proto_methods.push((array_iter_proto, "next", next_mf));
        // `Array.isArray` — a static bound as an own property of the `Array`
        // constructor instance (not the prototype).
        if let Some(&array_ctor) = self.intrinsics.get("Array") {
            let mf = self.alloc_method(NativeMethod::ArrayIsArray);
            self.proto_methods.push((array_ctor, "isArray", mf));
            let of = self.alloc_method(NativeMethod::ArrayOf);
            self.proto_methods.push((array_ctor, "of", of));
            // Recognized-but-unimplemented statics (honest named skips).
            let from = self.alloc_method(NativeMethod::ArrayFrom);
            self.proto_methods.push((array_ctor, "from", from));
            let from_async = self.alloc_method(NativeMethod::ArrayFromAsync);
            self.proto_methods.push((array_ctor, "fromAsync", from_async));
        }
        // Native prototype methods (bound to their prototype at link time,
        // only when the program references the method name). %Object.prototype%
        // carries toString/valueOf/hasOwnProperty/isPrototypeOf; each Error
        // prototype an `Error.prototype.toString`; each wrapper prototype a
        // `valueOf`/`toString` over the wrapped primitive; %Function.prototype%
        // a `toString`.
        let obj_methods = [
            ("toString", NativeMethod::ObjectToString),
            ("valueOf", NativeMethod::ObjectValueOf),
            ("hasOwnProperty", NativeMethod::ObjectHasOwnProperty),
            ("isPrototypeOf", NativeMethod::ObjectIsPrototypeOf),
        ];
        for (name, m) in obj_methods {
            let mf = self.alloc_method(m);
            self.proto_methods.push((object_proto, name, mf));
        }
        let fp_tostring = self.alloc_method(NativeMethod::FunctionToString);
        self.proto_methods.push((func_proto, "toString", fp_tostring));
        let fp_call = self.alloc_method(NativeMethod::FunctionCall);
        self.proto_methods.push((func_proto, "call", fp_call));
        let fp_apply = self.alloc_method(NativeMethod::FunctionApply);
        self.proto_methods.push((func_proto, "apply", fp_apply));
        // Every Error prototype (base + each subtype) gets `toString`.
        let error_protos: Vec<crate::value::SlotIndex> = {
            let mut v = vec![error_proto];
            for &(_, native) in Native::intrinsics() {
                if matches!(
                    native,
                    Native::EvalError
                        | Native::RangeError
                        | Native::ReferenceError
                        | Native::SyntaxError
                        | Native::TypeError
                        | Native::URIError
                        | Native::AggregateError
                ) {
                    if let Some(&c) = self.intrinsics.get(native.display_name()) {
                        if let Some(p) = self.prototype_of(c) {
                            v.push(p);
                        }
                    }
                }
            }
            v
        };
        for p in error_protos {
            let mf = self.alloc_method(NativeMethod::ErrorToString);
            self.proto_methods.push((p, "toString", mf));
        }
        // The inherited Error prototype `name` (per type) and `message` (""
        // on `%Error.prototype%`, inherited by subtypes). Placing `name` on
        // the prototype — not the instance — is what makes `err.name` resolve
        // up the chain while `err.hasOwnProperty('name')` is `false`, as XS.
        self.proto_data.push((error_proto, "name", "Error".to_string()));
        self.proto_data.push((error_proto, "message", String::new()));
        for &(_, native) in Native::intrinsics() {
            if matches!(
                native,
                Native::EvalError
                    | Native::RangeError
                    | Native::ReferenceError
                    | Native::SyntaxError
                    | Native::TypeError
                    | Native::URIError
                    | Native::AggregateError
            ) {
                if let Some(&c) = self.intrinsics.get(native.display_name()) {
                    if let Some(p) = self.prototype_of(c) {
                        self.proto_data
                            .push((p, "name", native.display_name().to_string()));
                    }
                }
            }
        }
        // The well-known symbols: each a fixed `Kind::Symbol` value whose
        // descriptor slot holds its `Symbol.<name>` description, bound as own
        // properties of the `Symbol` constructor at link time.
        for name in [
            "iterator",
            "asyncIterator",
            "hasInstance",
            "isConcatSpreadable",
            "match",
            "matchAll",
            "replace",
            "search",
            "species",
            "split",
            "toPrimitive",
            "toStringTag",
            "unscopables",
        ] {
            let desc = self.chunks.alloc(format!("Symbol.{}", name).as_bytes());
            let d = self
                .slots
                .alloc(Slot::of(Kind::String, Payload::String(desc)));
            let value = Slot::of(Kind::Symbol, Payload::Reference(d));
            self.well_known_symbols.push((name, value));
        }
        // The wrapper prototypes carry valueOf + toString over the primitive.
        for native in [Native::Boolean, Native::Number, Native::String] {
            if let Some(&c) = self.intrinsics.get(native.display_name()) {
                if let Some(p) = self.prototype_of(c) {
                    let v = self.alloc_method(NativeMethod::WrapperValueOf);
                    self.proto_methods.push((p, "valueOf", v));
                    let t = self.alloc_method(NativeMethod::WrapperToString);
                    self.proto_methods.push((p, "toString", t));
                }
            }
        }
    }

    /// Allocate a native prototype-method function instance (chained to
    /// %Function.prototype% is unnecessary for these — they are only ever
    /// dispatched, never re-inspected) registered in [`Self::functions`].
    fn alloc_method(&mut self, m: NativeMethod) -> crate::value::SlotIndex {
        let f = self.slots.alloc(Slot::instance(crate::value::SlotIndex::NULL));
        self.functions.insert(
            f,
            FuncInfo {
                method: Some(m),
                ..FuncInfo::default()
            },
        );
        f
    }

    /// The native-method identity of a function instance, if it is one.
    #[inline]
    fn method_of(&self, f: crate::value::SlotIndex) -> Option<NativeMethod> {
        self.functions.get(&f).and_then(|fi| fi.method)
    }

    /// The `.prototype` object of a constructor instance, if it is one.
    #[inline]
    fn prototype_of(&self, ctor: crate::value::SlotIndex) -> Option<crate::value::SlotIndex> {
        self.ctor_prototype.get(&ctor).copied()
    }

    /// Whether `target` appears in `obj`'s prototype chain — the core of
    /// `fxOrdinaryHasInstance`. The prototype is the instance slot's payload
    /// reference (XS's `instance->value.instance.prototype`); the walk
    /// follows it to the chain root (`NULL`).
    fn prototype_chain_has(
        &self,
        obj: crate::value::SlotIndex,
        target: crate::value::SlotIndex,
    ) -> bool {
        let mut cur = self.instance_prototype(obj);
        while !cur.is_null() {
            if cur == target {
                return true;
            }
            cur = self.instance_prototype(cur);
        }
        false
    }

    /// An instance slot's prototype (its payload reference), or `NULL`.
    #[inline]
    fn instance_prototype(&self, inst: crate::value::SlotIndex) -> crate::value::SlotIndex {
        match self.slots.get(inst).value {
            Payload::Reference(p) => p,
            _ => crate::value::SlotIndex::NULL,
        }
    }

    /// Bind the intrinsic constructors this program references into the
    /// global object, keyed by the program-local symbol id the C-XS compiler
    /// assigned each name (`names[k]` is the name of id `k + 1`; see
    /// [`crate::symbols`]). Only names that match a known intrinsic are
    /// bound; everything else is left to resolve as an ordinary global (a
    /// `var`/sloppy-global) or to miss. Unmetered: these globals pre-exist
    /// the guest run exactly as XS's do, so no allocation is charged.
    pub fn link_intrinsics(&mut self, names: &[String]) {
        self.symbol_names = names.to_vec();
        // Cache the program-local id of `length` (XS's `mxID(_length)`) so an
        // `arr.length` get/set routes to the array length semantics.
        self.length_id = names
            .iter()
            .position(|n| n == "length")
            .map(|k| (k + 1) as u16);
        let id_of = |want: &str| {
            names
                .iter()
                .position(|n| n == want)
                .map(|k| (k + 1) as u16)
        };
        self.value_id = id_of("value");
        self.done_id = id_of("done");
        for (k, name) in names.iter().enumerate() {
            let id = (k + 1) as u16;
            // Record the program-local id for every name, so a native
            // built-in can relink a well-known property name (`message`,
            // `name`, …) to the id the compiler assigned it in this program.
            self.symbol_ids.entry(name.clone()).or_insert(id);
            if self.global_props.contains_key(&id) {
                continue;
            }
            if let Some(&func) = self.intrinsics.get(name.as_str()) {
                // The global binding is an own property whose value is a
                // **reference** to the intrinsic function instance, exactly
                // like any other global property (so `get_variable` /
                // `get_this_variable` resolve a `Reference`, and `typeof`
                // sees a callable). Not metered — a pre-existing global.
                self.create_global_property(id, (Kind::Reference, Payload::Reference(func)));
            } else if let Some(v) = value_global(name) {
                // The primitive value globals `undefined`/`NaN`/`Infinity`
                // (XS's non-writable realm globals): bound as ordinary global
                // properties holding the value, so a reference reads it with
                // no built-in step (pure dispatch, bit-exact against the pin).
                self.create_global_property(id, (v.kind, v.value));
            }
        }
        // Install the native prototype methods whose names this program
        // references, as own properties of their prototype (unmetered — an
        // inherited intrinsic method, present before the guest runs).
        let methods = std::mem::take(&mut self.proto_methods);
        for &(proto, mname, mfunc) in &methods {
            if let Some(&mid) = self.symbol_ids.get(mname) {
                self.set_own_unmetered(
                    proto,
                    mid,
                    Slot::of(Kind::Reference, Payload::Reference(mfunc)),
                );
            }
        }
        self.proto_methods = methods;
        // Inherited prototype data (Error `name`/`message`).
        let data = std::mem::take(&mut self.proto_data);
        for (proto, pname, value) in &data {
            if let Some(&pid) = self.symbol_ids.get(*pname) {
                let off = self.chunks.alloc(value.as_bytes());
                self.set_own_unmetered(*proto, pid, Slot::of(Kind::String, Payload::String(off)));
            }
        }
        self.proto_data = data;
        // Well-known symbols as own properties of the `Symbol` constructor.
        if let Some(&symbol_ctor) = self.intrinsics.get("Symbol") {
            let wks = std::mem::take(&mut self.well_known_symbols);
            for (name, value) in &wks {
                if let Some(&wid) = self.symbol_ids.get(*name) {
                    self.set_own_unmetered(symbol_ctor, wid, *value);
                }
            }
            self.well_known_symbols = wks;
        }
    }

    /// The native identity of a function instance, if it is an intrinsic.
    #[inline]
    fn native_of(&self, f: crate::value::SlotIndex) -> Option<Native> {
        self.functions.get(&f).and_then(|fi| fi.native)
    }

    /// The slots the *active* frame holds live: the shared value stack, the
    /// current scope, the current arguments, and the frame quartet. Added
    /// to [`Self::frame_slots`] (the suspended frames) it mirrors XS's
    /// `stackTop - stack` closely enough that the overflow abort brackets
    /// C-XS's — over-counting slightly (the value stack still carries the
    /// pre-truncation frame region at a call site) rather than under, so
    /// endor never *completes* a program C-XS overflows on.
    #[inline]
    fn live_stack_slots(&self) -> usize {
        self.stack.len() + self.locals.len() + self.args.len() + FRAME_OVERHEAD_SLOTS
    }

    /// Total concurrent slot usage across the active and suspended frames
    /// (XS's `stackTop - stack`). The stack-overflow guard compares this
    /// against the fixed budget.
    #[inline]
    fn stack_slots_in_use(&self) -> usize {
        self.frame_slots + self.live_stack_slots()
    }

    /// Whether allocating `extra` more slots would exhaust the fixed value
    /// stack (XS's `fxOverflow`: `stack + count < stackBottom`). The usable
    /// budget is [`STACK_SLOT_COUNT`] minus the reserved root band.
    #[inline]
    fn would_overflow(&self, extra: usize) -> bool {
        self.stack_slots_in_use() + extra > STACK_SLOT_COUNT - STACK_SLOT_RESERVED
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

    /// The content bytes of a heap string (up to the C NUL terminator, or
    /// the whole payload for an interned string stored without one): XS's
    /// `mxStringLength`/`c_strlen` view of a string value.
    #[inline]
    fn str_content(&self, off: crate::value::ChunkOffset) -> &[u8] {
        let p = self.chunks.payload(off);
        let end = p.iter().position(|&b| b == 0).unwrap_or(p.len());
        &p[..end]
    }

    /// Render a completion/thrown value the way the oracle shim does:
    /// `fxToString` then the raw CESU-8 bytes up to the NUL through
    /// `from_utf8_lossy` (`endor-oracle` `cstr_field`). Because both
    /// engines run the same CESU-8 bytes through `from_utf8_lossy`, a
    /// string value renders byte-identically to the oracle even for astral
    /// code points (CESU-8 surrogate pairs both decode lossily the same
    /// way). Non-string kinds defer to [`slot_to_ecma_string`].
    fn render(&self, s: &Slot) -> String {
        match s.value {
            Payload::String(off) => {
                String::from_utf8_lossy(self.str_content(off)).into_owned()
            }
            Payload::Reference(r) => {
                // An Error instance stringifies through `Error.prototype.
                // toString`: `name` with an empty/absent message, else
                // `name: message` — the abort/completion value parity the
                // Error hierarchy graduates.
                if let Some(a) = self.arrays.get(&r) {
                    // An array stringifies through `Array.prototype.toString` →
                    // `join(",")`: each index in `[0, length)` rendered, holes
                    // and `undefined`/`null` rendered as the empty string,
                    // joined with commas.
                    let mut out = String::new();
                    for i in 0..a.length {
                        if i > 0 {
                            out.push(',');
                        }
                        if let Some(item) = a.items.get(&i) {
                            if item.kind != Kind::Undefined && item.kind != Kind::Null {
                                out.push_str(&self.render(item));
                            }
                        }
                    }
                    out
                } else if let Some(info) = self.error_data.get(&r) {
                    match &info.message {
                        Some(m) if !m.is_empty() => format!("{}: {}", info.name, m),
                        _ => info.name.to_string(),
                    }
                } else if let Some(prim) = self.wrapper_data.get(&r).copied() {
                    // A primitive wrapper (`new Boolean`/`Number`/`String`)
                    // stringifies as its wrapped primitive value.
                    self.render(&prim)
                } else if let Some(n) = self.native_of(r) {
                    // A native (intrinsic) function stringifies through
                    // `Function.prototype.toString` as a host function
                    // (verified against the pin for a bare `Object`/`Boolean`).
                    format!("function [\"{}\"] (){{[native code]}}", n.display_name())
                } else {
                    slot_to_ecma_string(s)
                }
            }
            _ => slot_to_ecma_string(s),
        }
    }

    /// Run a program bytecode buffer to completion.
    pub fn run(&mut self, code: &[u8]) -> RunOutcome {
        let mut halt = self.dispatch(code);
        // A program that completes with a Symbol *value* is coerced to a
        // string by the harness (`String(result)`), which throws — so the
        // oracle reports the run as an abort, not a completion. Mirror that:
        // a Symbol completion becomes the same `TypeError` abort. The ToString
        // throw is post-run in the shim, so it adds no run computrons (the
        // meter already matches the oracle's run-only count).
        if halt == Halt::Return && self.result.kind == Kind::Symbol {
            halt = Halt::Throw("TypeError: cannot coerce symbol to string".to_string());
        }
        let completed = halt == Halt::Return;
        let result = if completed {
            self.render(&self.result)
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
        // The top-level program: start at pc 0, return to the host (C
        // boundary) when the call stack fully unwinds (depth 0).
        self.dispatch_at(code, 0, 0)
    }

    /// The interpreter dispatch loop, runnable from any `start_pc` and stopping
    /// when an `END` pops the call stack back to `return_depth` (the top-level
    /// program uses `0`/`0`). A native method drives a callback by entering its
    /// frame and calling this with the callback's `body_start` and the caller's
    /// current call depth, so the callback runs to its own `END` and returns
    /// control here — the re-entrant substrate the callback-taking
    /// `Array.prototype` methods (`forEach`/`map`/…) need.
    fn dispatch_at(&mut self, code: &[u8], start_pc: usize, return_depth: usize) -> Halt {
        let len = code.len();
        let mut pc: usize = start_pc;

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
                        self.bind_program_this();
                    } else if self.cur_target {
                        // A constructor frame (`new f(...)`): allocate the
                        // `this` instance (`fxRunConstructor`) before the body.
                        self.run_constructor();
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
                        // A top-level *script* frame's `this` is the realm
                        // global in strict mode too (only an ES module's is
                        // `undefined`, and modules are structurally skipped).
                        self.bind_program_this();
                    } else if self.cur_target && op == XS_CODE_BEGIN_STRICT {
                        // A strict `function` constructor invoked with `new`:
                        // `fxRunConstructor` allocates `this` (the class
                        // `*_BASE`/`*_DERIVED` forms need the class machinery,
                        // out of the covered grammar — left to self-name).
                        self.run_constructor();
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
                XS_CODE_VAR_LOCAL_1
                | XS_CODE_VAR_LOCAL_2
                | XS_CODE_LET_LOCAL_1
                | XS_CODE_LET_LOCAL_2
                | XS_CODE_CONST_LOCAL_1
                | XS_CODE_CONST_LOCAL_2
                | XS_CODE_SET_LOCAL_1
                | XS_CODE_SET_LOCAL_2 => {
                    let k = self.local_operand(op, code, pc);
                    let top = *self.stack.last().unwrap_or(&Slot::undefined());
                    self.set_local(k, top);
                    pc += size as usize;
                }
                XS_CODE_PULL_LOCAL_1 | XS_CODE_PULL_LOCAL_2 => {
                    let k = self.local_operand(op, code, pc);
                    let v = self.pop();
                    self.set_local(k, v);
                    pc += size as usize;
                }
                XS_CODE_GET_LOCAL_1 | XS_CODE_GET_LOCAL_2 => {
                    let k = self.local_operand(op, code, pc);
                    let v = self.get_local(k);
                    match v {
                        Some(s) => self.push(s),
                        None => return Halt::Throw("get: not initialized yet".into()),
                    }
                    pc += size as usize;
                }
                XS_CODE_UNWIND_1 | XS_CODE_UNWIND_2 => {
                    let n = self.local_operand(op, code, pc);
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
                        // Creating a sloppy global through `SET_VARIABLE`
                        // dispatches XS's setter machinery
                        // (`mxBehaviorSetProperty` → the missing-property
                        // define path), which meters one extra code unit
                        // beyond the property allocation. Measured against
                        // the pin: `y = 1` costs one create's 65536 raw more
                        // than endor's allocation model, and N fresh
                        // globals cost exactly N of them (an overwrite costs
                        // none). This is the `SET_VARIABLE`-create path
                        // only; the declared-`var` hoist at
                        // `EVAL_ENVIRONMENT` (already bit-exact) does not
                        // carry it.
                        self.meter.tick_code();
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
                // `array` (`XS_CODE_ARRAY`): `fxNewArray(the, 0)` — push a
                // reference to a fresh empty exotic array. The array-literal
                // prelude stores it, sets `.length`, then fills item slots via
                // `NEW_PROPERTY_AT`.
                XS_CODE_ARRAY => {
                    let inst = self.new_array();
                    self.push(Slot::of(Kind::Reference, Payload::Reference(inst)));
                    pc += size as usize;
                }
                // `at` / `at_2` (`XS_CODE_AT`/`AT_2`): convert a computed key on
                // the stack (`o[k]`) into an `XS_AT_KIND` key the
                // `*_PROPERTY_AT` opcodes consume. `AT` operates on the top
                // slot; `AT_2` on `mxStack+1` (used by the define/set forms
                // where the value sits on top). An integer/number that is a
                // valid array index becomes an index key; a symbol or a string
                // that names a program symbol becomes a named key. XS meters a
                // non-index string key `2 × XS_CODE_METERING` extra; the
                // integer/symbol paths are dispatch-only.
                XS_CODE_AT | XS_CODE_AT_2 => {
                    let depth = if op == XS_CODE_AT_2 { 1 } else { 0 };
                    let idx = self.stack.len().checked_sub(1 + depth);
                    let key = match idx.map(|i| self.stack[i]) {
                        Some(k) => k,
                        None => return Halt::Unsupported(op.name()),
                    };
                    let at = match self.to_at_key(key) {
                        Some(at) => at,
                        None => return Halt::Unsupported(op.name()),
                    };
                    if let Some(i) = idx {
                        self.stack[i] = at;
                    }
                    pc += size as usize;
                }
                // `arr[k]` read (`XS_CODE_GET_PROPERTY_AT`). Stack:
                // [.., objectRef, atKey] → [.., value]. Like `GET_PROPERTY`,
                // meters no built-in step.
                XS_CODE_GET_PROPERTY_AT => {
                    let key = self.pop();
                    let obj = self.pop();
                    let v = self.property_at_get(obj, key);
                    match v {
                        Ok(s) => self.push(s),
                        Err(h) => return h,
                    }
                    pc += size as usize;
                }
                // `arr[k] = v` (`XS_CODE_SET_PROPERTY_AT`). Stack:
                // [.., objectRef, atKey, value] → [.., value].
                XS_CODE_SET_PROPERTY_AT => {
                    let value = self.pop();
                    let key = self.pop();
                    let obj = self.pop();
                    if let Err(h) = self.property_at_set(obj, key, value, false) {
                        return h;
                    }
                    self.push(value);
                    pc += size as usize;
                }
                // `arr[k] = v` in a literal / definition (`NEW_PROPERTY_AT`).
                // Stack: [.., objectRef, atKey, value]; a 2-byte trailing
                // operand carries the property attributes (the AT form has no
                // id operand, so its total length is opcode + 2 = 3 bytes, and
                // the flag is the *second* of those two — see the `xsRun.c`
                // `NEW_PROPERTY_ALL` pointer walk). Consumes all three stack
                // slots (defines the item), leaving the base object the
                // literal keeps below.
                XS_CODE_NEW_PROPERTY_AT => {
                    if pc + 3 > len {
                        return Halt::Decode(format!("new_property_at at {} needs 3 bytes", pc));
                    }
                    let value = self.pop();
                    let key = self.pop();
                    let obj = self.pop();
                    if let Err(h) = self.property_at_set(obj, key, value, true) {
                        return h;
                    }
                    pc += 3;
                }
                // `for_of` (`XS_CODE_FOR_OF` → `fxRunForOf` → `fxGetIterator`):
                // replace the top-of-stack iterable with its iterator,
                // `iterable[Symbol.iterator]()`. For an array that is the
                // `values` iterator; the surrounding loop then reads `.next`
                // and drives the {value,done} protocol through already-modeled
                // opcodes. A non-array iterable (string/user iterator) self-
                // names an honest skip (its iterator wiring is a later
                // increment). Metering is the `fxGetIterator` cost measured
                // against the pin.
                XS_CODE_FOR_OF => {
                    let iterable = self.pop();
                    let arr = match iterable.value {
                        Payload::Reference(i) if self.arrays.contains_key(&i) => i,
                        _ => return Halt::Unsupported(op.name()),
                    };
                    self.meter.tick_raw(FOR_OF_GET_ITERATOR_METERING);
                    let it = self.make_array_iterator(arr, 0);
                    self.push(it);
                    pc += size as usize;
                }
                // `for_in` (`XS_CODE_FOR_IN`): call the enumerator function on
                // the top-of-stack object, replacing it with a for-in
                // enumerator; the surrounding loop reads `.next` and drives the
                // key-yielding {value,done} protocol through already-modeled
                // opcodes. A non-object (or an object with a non-covered
                // prototype) self-names an honest skip. XS's `XS_CODE_FOR_IN`
                // sets up a `RUN_ALL` of `mxEnumeratorFunction`; endor builds
                // the enumerator in place with the equivalent metering.
                XS_CODE_FOR_IN => {
                    let obj = self.pop();
                    let inst = match obj.value {
                        // `undefined`/`null` for-in is a legal empty loop, but
                        // its zero-key enumerator setup is a later increment;
                        // an object receiver is the covered case.
                        Payload::Reference(i) => i,
                        _ => return Halt::Unsupported(op.name()),
                    };
                    let it = self.make_enumerator(inst);
                    self.push(it);
                    pc += size as usize;
                }
                // `check_instance` (`XS_CODE_CHECK_INSTANCE`): the iterator
                // result must be an object; a non-reference top throws a
                // `TypeError` (XS's `fxRunDebug`). Dispatch-metered only.
                XS_CODE_CHECK_INSTANCE => {
                    let top = self.stack.last().copied().unwrap_or_else(Slot::undefined);
                    if top.kind != Kind::Reference {
                        return Halt::Throw("iterator result: not an object".into());
                    }
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
                        if self.arrays.contains_key(&inst) && Some(id) == self.length_id {
                            // `arr.length = N`: the exotic-array length accessor
                            // setter (`fxArrayLengthSetter` → `fxArraySetLength`).
                            self.array_set_length(inst, value);
                        } else {
                            self.instance_put(inst, id, value);
                        }
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
                        Payload::Reference(inst)
                            if self.arrays.contains_key(&inst) && Some(id) == self.length_id =>
                        {
                            // `arr.length`: the exotic-array length accessor
                            // getter (`fxArrayLengthGetter`).
                            self.meter.tick_raw(ARRAY_LENGTH_GET_METERING);
                            Slot::integer(self.arrays[&inst].length as i32)
                        }
                        Payload::Reference(inst) => self.instance_get(inst, id),
                        _ => Slot::undefined(),
                    };
                    self.push(v);
                    pc += ilen;
                }
                // `delete o.k` (XS_CODE_DELETE_PROPERTY, xsRun.c): remove the
                // own property `id` from the top-of-stack object, replacing
                // the object slot with the boolean result (XS keeps the stack
                // slot in place). A configurable own data property (all the
                // covered grammar creates) deletes to `true`; deleting an
                // absent own property is also `true`. A non-reference target
                // needs `mxToInstance` (which throws), so it self-names
                // unsupported.
                XS_CODE_DELETE_PROPERTY => {
                    let id = id!(1);
                    let obj = *self.stack.last().unwrap_or(&Slot::undefined());
                    match obj.value {
                        Payload::Reference(inst) => {
                            // `fxRunDelete` wraps `mxBehaviorDeleteProperty`
                            // in a host frame (`fxBeginHost`/`fxEndHost`),
                            // whose teardown meters one built-in step
                            // (`mxMeterOne`) — measured against the pin as
                            // exactly `XS_BUILTIN_METERING` over the
                            // allocation-free unlink.
                            self.meter.tick_builtin();
                            let deleted = self.delete_own_property(inst, id);
                            if let Some(s) = self.stack.last_mut() {
                                *s = Slot::boolean(deleted);
                            }
                        }
                        _ => return Halt::Unsupported(op.name()),
                    }
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
                // `new` (`XS_CODE_NEW`, xsRun.c): the constructor is already
                // on the stack (from `get_variable`). Reshape the single
                // constructor slot into the construct frame geometry
                // `[THIS, FUNCTION, RESULT, FRAME]`, where `THIS` is the
                // **uninitialized** construct placeholder — XS's `RUN_ALL`
                // reads that (`mxFrameThis->kind == XS_UNINITIALIZED_KIND`) as
                // the target flag, and `begin`'s `fxRunConstructor` fills it
                // with the fresh instance. No heap allocation here (the frame
                // is stack slots); dispatch-metered only, as C-XS's `NEW`.
                XS_CODE_NEW => {
                    let ctor = self.pop();
                    self.push(Slot::uninitialized()); // THIS (construct placeholder)
                    self.push(ctor); // FUNCTION
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
                    // A native (intrinsic) callee runs a C handler in place
                    // rather than entering a bytecode frame (XS's
                    // `XS_CALLBACK_KIND` branch of `RUN_ALL`): no call-entry
                    // `mxFirstCode` check (the C path leaves `mxCode` null),
                    // the handler meters its own steps, and control returns
                    // into this JS frame — where `END_ALL`'s `mxFirstCode`
                    // does check. A non-target (plain) call only; `new` on a
                    // native is a separate, not-yet-modeled path.
                    // A construct call (`new`) leaves the `THIS` slot as the
                    // uninitialized placeholder (XS's `RUN_ALL` target
                    // detection: `mxFrameThis->kind == XS_UNINITIALIZED_KIND`);
                    // a plain call pushed a real/`undefined` `this`.
                    let base_opt = self.stack.len().checked_sub(argc + 4);
                    let has_target = base_opt
                        .and_then(|b| self.stack.get(b))
                        .map(|s| s.kind == Kind::Uninitialized)
                        .unwrap_or(false);
                    let func_ref = base_opt.and_then(|base| {
                        match self.stack.get(base + 1).map(|s| s.value) {
                            Some(Payload::Reference(f)) => Some((f, base)),
                            _ => None,
                        }
                    });
                    let callee = func_ref.and_then(|(f, base)| self.native_of(f).map(|n| (n, base)));
                    let method = func_ref.and_then(|(f, base)| self.method_of(f).map(|m| (m, base)));
                    if let Some((native, base)) = callee {
                        // A native (intrinsic) constructor callee.
                        match self.call_native(native, base, argc, has_target) {
                            Ok(()) => {
                                // Return into the JS caller: `END_ALL` checks.
                                if self.check_meter() == MeterCheck::Abort {
                                    return Halt::MeterAbort;
                                }
                                pc = ret_pc;
                            }
                            Err(h) => return h,
                        }
                    } else if let Some((NativeMethod::FunctionCall, base)) = method {
                        // `Function.prototype.call`: re-enter the target frame
                        // (a trampoline), resuming the caller after this `run`.
                        match self.enter_call_dot_call(base, argc, ret_pc) {
                            Ok(body_start) => {
                                if self.check_meter() == MeterCheck::Abort {
                                    return Halt::MeterAbort;
                                }
                                pc = body_start;
                            }
                            Err(h) => return h,
                        }
                    } else if let Some((NativeMethod::FunctionApply, base)) = method {
                        // `Function.prototype.apply` (no-array subset): re-enter
                        // the target with the rebound `this` and no arguments.
                        match self.enter_call_dot_apply(base, argc, ret_pc) {
                            Ok(body_start) => {
                                if self.check_meter() == MeterCheck::Abort {
                                    return Halt::MeterAbort;
                                }
                                pc = body_start;
                            }
                            Err(h) => return h,
                        }
                    } else if let Some((m, base)) = method {
                        // A native prototype method: the call's receiver is
                        // `this` (stack[base]); its arguments follow. `code` is
                        // threaded through so a callback-taking method
                        // (`forEach`/`map`/…) can drive the callback via
                        // `run_callback`.
                        match self.call_native_method(m, base, argc, code) {
                            Ok(()) => {
                                if self.check_meter() == MeterCheck::Abort {
                                    return Halt::MeterAbort;
                                }
                                pc = ret_pc;
                            }
                            Err(h) => return h,
                        }
                    } else {
                        match self.enter_call(argc, ret_pc, has_target) {
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
                // `var_closure #k` / `set_closure #k` / `let_closure #k` /
                // `const_closure #k`: write the shared cell from the stack
                // top **without** popping (an explicit `pop` discards it when
                // unwanted). XS's `let_closure`/`const_closure`
                // (xsRun.c:LET_CLOSURE/CONST_CLOSURE) initialize a
                // `let`/`const` binding's cell exactly as `set_closure`
                // writes it; the const "already initialized" guard and the
                // DONT_SET/DONT_ENUM flags do not fire in the covered
                // single-assignment grammar and are metering-neutral.
                XS_CODE_VAR_CLOSURE_1
                | XS_CODE_VAR_CLOSURE_2
                | XS_CODE_SET_CLOSURE_1
                | XS_CODE_SET_CLOSURE_2
                | XS_CODE_LET_CLOSURE_1
                | XS_CODE_LET_CLOSURE_2
                | XS_CODE_CONST_CLOSURE_1
                | XS_CODE_CONST_CLOSURE_2 => {
                    let k = self.closure_index(op, code, pc);
                    let top = *self.stack.last().unwrap_or(&Slot::undefined());
                    self.write_closure_cell(k, top);
                    pc += op.size() as usize;
                }
                // `reset_closure #k` (xsRun.c:RESET_CLOSURE): point scope
                // closure `k` at a **fresh** uninitialized cell
                // (`fxNewSlot`, metered) — a loop body's per-iteration
                // `let` binding gets a new cell each turn.
                XS_CODE_RESET_CLOSURE_1 | XS_CODE_RESET_CLOSURE_2 => {
                    let k = self.closure_index(op, code, pc);
                    let cell = self.slots.alloc(Slot::uninitialized());
                    self.meter.tick_slot_alloc();
                    self.repoint_closure(k, cell);
                    pc += op.size() as usize;
                }
                // `refresh_closure #k` (xsRun.c:REFRESH_CLOSURE): point scope
                // closure `k` at a fresh cell (`fxNewSlot`, metered) that
                // **copies** the old cell's flag/kind/value — a per-iteration
                // `let` capture that snapshots the current binding.
                XS_CODE_REFRESH_CLOSURE_1 | XS_CODE_REFRESH_CLOSURE_2 => {
                    let k = self.closure_index(op, code, pc);
                    let old = self.closure_cell(k);
                    let src = old.map(|c| *self.slots.get(c)).unwrap_or_else(Slot::uninitialized);
                    let mut fresh = Slot::of(src.kind, src.value);
                    fresh.flag = src.flag;
                    let cell = self.slots.alloc(fresh);
                    self.meter.tick_slot_alloc();
                    self.repoint_closure(k, cell);
                    pc += op.size() as usize;
                }
                // `refresh_local #k` (xsRun.c:REFRESH_LOCAL): a no-op in the
                // run (`variable = mxEnvironment - index` then nothing);
                // dispatch-metered only.
                XS_CODE_REFRESH_LOCAL_1 | XS_CODE_REFRESH_LOCAL_2 => {
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
                // `string` (XS_CODE_STRING_1/2/4, xsRun.c:3044): a string
                // literal. The operand is a length-prefixed run of inline
                // CESU-8 bytes (including the compiler's trailing NUL);
                // `fxNewChunk(len)` copies them into a fresh chunk (metered
                // per adjusted byte, `tick_chunk_new`), and a String slot
                // referencing the chunk is pushed. `len` is the byte count
                // from the length prefix, exactly XS's `index`.
                XS_CODE_STRING_1 | XS_CODE_STRING_2 | XS_CODE_STRING_4 => {
                    let (n, data) = match op {
                        XS_CODE_STRING_1 => (code[pc + 1] as usize, pc + 2),
                        XS_CODE_STRING_2 => {
                            (u16::from_le_bytes([code[pc + 1], code[pc + 2]]) as usize, pc + 3)
                        }
                        _ => (
                            u32::from_le_bytes([
                                code[pc + 1],
                                code[pc + 2],
                                code[pc + 3],
                                code[pc + 4],
                            ]) as usize,
                            pc + 5,
                        ),
                    };
                    self.meter.tick_chunk_new(n as u64);
                    let off = self.chunks.alloc(&code[data..data + n]);
                    self.push(Slot::of(Kind::String, Payload::String(off)));
                    pc += ilen;
                }
                // `typeof` (XS_CODE_TYPEOF, xsRun.c:4162): replace the stack
                // top with the interned type-name string. A reference is a
                // "function" when it is a callable instance (endor tracks
                // those in `functions`), else "object"; `null` is "object".
                // Dispatch-only: the type strings are preinterned.
                XS_CODE_TYPEOF => {
                    let top = self.stack.last().copied().unwrap_or_else(Slot::undefined);
                    let off = match top.kind {
                        Kind::Undefined => self.static_str.undefined,
                        Kind::Null => self.static_str.object,
                        Kind::Boolean => self.static_str.boolean,
                        Kind::Integer | Kind::Number => self.static_str.number,
                        Kind::String => self.static_str.string,
                        Kind::Reference => match top.value {
                            Payload::Reference(r) if self.functions.contains_key(&r) => {
                                self.static_str.function
                            }
                            _ => self.static_str.object,
                        },
                        Kind::Symbol => self.static_str.symbol,
                        // Closure/EnvReference/Uninitialized are never live
                        // stack *values*; a bigint would need its own interned
                        // name (later stages).
                        _ => return Halt::Unsupported(op.name()),
                    };
                    if let Some(s) = self.stack.last_mut() {
                        *s = Slot::of(Kind::String, Payload::String(off));
                    }
                    pc += size as usize;
                }

                // ---- arithmetic -------------------------------------
                XS_CODE_ADD => {
                    if self.op_add().is_err() {
                        return Halt::Unsupported(op.name());
                    }
                    pc += size as usize;
                }
                XS_CODE_SUBTRACT => {
                    if self.binary_arith(ArithOp::Sub).is_err() {
                        return Halt::Unsupported(op.name());
                    }
                    pc += size as usize;
                }
                XS_CODE_MULTIPLY => {
                    if self.binary_arith(ArithOp::Mul).is_err() {
                        return Halt::Unsupported(op.name());
                    }
                    pc += size as usize;
                }
                XS_CODE_DIVIDE => {
                    if self.binary_arith(ArithOp::Div).is_err() {
                        return Halt::Unsupported(op.name());
                    }
                    pc += size as usize;
                }
                XS_CODE_MODULO => {
                    if self.binary_arith(ArithOp::Mod).is_err() {
                        return Halt::Unsupported(op.name());
                    }
                    pc += size as usize;
                }

                // ---- bitwise ----------------------------------------
                XS_CODE_BIT_AND => {
                    if self.binary_bit(BitOp::And).is_err() {
                        return Halt::Unsupported(op.name());
                    }
                    pc += size as usize;
                }
                XS_CODE_BIT_OR => {
                    if self.binary_bit(BitOp::Or).is_err() {
                        return Halt::Unsupported(op.name());
                    }
                    pc += size as usize;
                }
                XS_CODE_BIT_XOR => {
                    if self.binary_bit(BitOp::Xor).is_err() {
                        return Halt::Unsupported(op.name());
                    }
                    pc += size as usize;
                }
                XS_CODE_LEFT_SHIFT => {
                    if self.binary_bit(BitOp::Shl).is_err() {
                        return Halt::Unsupported(op.name());
                    }
                    pc += size as usize;
                }
                XS_CODE_SIGNED_RIGHT_SHIFT => {
                    if self.binary_bit(BitOp::Sar).is_err() {
                        return Halt::Unsupported(op.name());
                    }
                    pc += size as usize;
                }
                XS_CODE_UNSIGNED_RIGHT_SHIFT => {
                    if self.binary_bit(BitOp::Shr).is_err() {
                        return Halt::Unsupported(op.name());
                    }
                    pc += size as usize;
                }
                XS_CODE_BIT_NOT => {
                    let a = self.pop();
                    if a.kind == Kind::String {
                        return Halt::Unsupported(op.name());
                    }
                    self.push(Slot::integer(!to_int32(to_number(&a))));
                    pc += size as usize;
                }

                // ---- comparison -------------------------------------
                XS_CODE_LESS => {
                    if self.relational(RelOp::Less).is_err() {
                        return Halt::Unsupported(op.name());
                    }
                    pc += size as usize;
                }
                XS_CODE_LESS_EQUAL => {
                    if self.relational(RelOp::LessEqual).is_err() {
                        return Halt::Unsupported(op.name());
                    }
                    pc += size as usize;
                }
                XS_CODE_MORE => {
                    if self.relational(RelOp::More).is_err() {
                        return Halt::Unsupported(op.name());
                    }
                    pc += size as usize;
                }
                XS_CODE_MORE_EQUAL => {
                    if self.relational(RelOp::MoreEqual).is_err() {
                        return Halt::Unsupported(op.name());
                    }
                    pc += size as usize;
                }
                XS_CODE_STRICT_EQUAL => {
                    if self.equality(true, false).is_err() {
                        return Halt::Unsupported(op.name());
                    }
                    pc += size as usize;
                }
                XS_CODE_STRICT_NOT_EQUAL => {
                    if self.equality(true, true).is_err() {
                        return Halt::Unsupported(op.name());
                    }
                    pc += size as usize;
                }
                XS_CODE_EQUAL => {
                    if self.equality(false, false).is_err() {
                        return Halt::Unsupported(op.name());
                    }
                    pc += size as usize;
                }
                XS_CODE_NOT_EQUAL => {
                    if self.equality(false, true).is_err() {
                        return Halt::Unsupported(op.name());
                    }
                    pc += size as usize;
                }

                // ---- unary ------------------------------------------
                XS_CODE_MINUS => {
                    let a = self.pop();
                    // `-string` needs ToNumber(string); defer.
                    if a.kind == Kind::String {
                        return Halt::Unsupported(op.name());
                    }
                    self.push(unary_minus(&a));
                    pc += size as usize;
                }
                XS_CODE_PLUS => {
                    let a = self.pop();
                    // ToNumber; an integer stays an integer. `+string` needs
                    // string→number parsing; defer.
                    match a.kind {
                        Kind::Integer => self.push(a),
                        Kind::String => return Halt::Unsupported(op.name()),
                        _ => self.push(Slot::number(to_number(&a))),
                    }
                    pc += size as usize;
                }
                XS_CODE_NOT => {
                    let a = self.pop();
                    let t = self.truthy(&a);
                    self.push(Slot::boolean(!t));
                    pc += size as usize;
                }
                XS_CODE_VOID => {
                    let _ = self.pop();
                    self.push(Slot::undefined());
                    pc += size as usize;
                }

                // ---- stage-3 language opcodes -----------------------
                // `global` (XS_CODE_GLOBAL, xsRun.c:2733): push a
                // reference to the realm's global object. Dispatch-metered
                // (no allocation).
                XS_CODE_GLOBAL => {
                    let g = self.global_obj;
                    self.push(Slot::of(Kind::Reference, Payload::Reference(g)));
                    pc += size as usize;
                }
                // `this` (XS_CODE_THIS, xsRun.c:1334): push the frame's
                // `this` (`*mxFrameThis`). Bound to the realm global for a
                // top-level script frame (set at program entry) and for a
                // sloppy function call; dispatch-metered.
                XS_CODE_THIS => {
                    let t = self.this_val;
                    self.push(t);
                    pc += size as usize;
                }
                // `current` (XS_CODE_CURRENT, xsRun.c:1308): push the
                // running function (`*mxFrameFunction`). Defined only inside
                // a user-function frame; at program level there is no user
                // function instance to name, so it self-names unsupported
                // rather than pushing a bogus value.
                XS_CODE_CURRENT => {
                    if self.cur_func.is_null() {
                        return Halt::Unsupported(op.name());
                    }
                    let f = self.cur_func;
                    self.push(Slot::of(Kind::Reference, Payload::Reference(f)));
                    pc += size as usize;
                }
                // `to_numeric` (XS_CODE_TO_NUMERIC, xsRun.c:3358): coerce
                // the stack top to a numeric. An int/number is already
                // numeric (a no-op, exactly as C-XS); boolean/null/undefined
                // coerce with `ToNumber` (no metering — `fxToNumber` on a
                // primitive allocates nothing); a string/reference/bigint
                // needs the ToPrimitive/BigInt path outside the covered
                // primitive subset, so it self-names unsupported.
                XS_CODE_TO_NUMERIC => {
                    let top = *self.stack.last().unwrap_or(&Slot::undefined());
                    match top.kind {
                        Kind::Integer | Kind::Number => {}
                        Kind::Boolean | Kind::Null | Kind::Undefined => {
                            if let Some(s) = self.stack.last_mut() {
                                *s = Slot::number(to_number(&top));
                            }
                        }
                        _ => return Halt::Unsupported(op.name()),
                    }
                    pc += size as usize;
                }
                // `increment`/`decrement` (XS_CODE_INCREMENT/DECREMENT,
                // xsRun.c:3391/3366): ±1 on the numeric stack top, with XS's
                // exact int-boundary promotion to number (INT_MAX for
                // increment, -(INT_MAX) for decrement). A non-numeric top
                // needs ToNumeric (unsupported here); the compiler emits a
                // preceding `to_numeric`, so the top is numeric in the
                // covered grammar.
                XS_CODE_INCREMENT | XS_CODE_DECREMENT => {
                    let inc = op == XS_CODE_INCREMENT;
                    let top = match self.stack.last_mut() {
                        Some(s) => s,
                        None => return Halt::Unsupported(op.name()),
                    };
                    match (top.kind, top.value) {
                        (Kind::Integer, Payload::Integer(v)) => {
                            let boundary = if inc { i32::MAX } else { -i32::MAX };
                            if v != boundary {
                                top.value = Payload::Integer(if inc { v + 1 } else { v - 1 });
                            } else {
                                top.kind = Kind::Number;
                                top.value =
                                    Payload::Number(if inc { v as f64 + 1.0 } else { v as f64 - 1.0 });
                            }
                        }
                        (Kind::Number, Payload::Number(n)) => {
                            top.value = Payload::Number(if inc { n + 1.0 } else { n - 1.0 });
                        }
                        _ => return Halt::Unsupported(op.name()),
                    }
                    pc += size as usize;
                }
                // `exponentiation` (XS_CODE_EXPONENTIATION, xsRun.c:3574):
                // `a ** b` → `fx_pow(a, b)` as a number. Numeric operands
                // only (int/number × int/number, XS's fast path); a
                // non-numeric operand needs the ToNumeric/BigInt general
                // path, so it self-names unsupported without disturbing the
                // operands.
                XS_CODE_EXPONENTIATION => {
                    let n = self.stack.len();
                    if n < 2 {
                        return Halt::Unsupported(op.name());
                    }
                    let a = numeric_of(&self.stack[n - 2]);
                    let b = numeric_of(&self.stack[n - 1]);
                    match (a, b) {
                        (Some(x), Some(y)) => {
                            self.stack.truncate(n - 2);
                            self.push(Slot::number(fx_pow(x, y)));
                        }
                        _ => return Halt::Unsupported(op.name()),
                    }
                    pc += size as usize;
                }

                // `instanceof` (`XS_CODE_INSTANCEOF`, xsRun.c → fxRunInstanceOf
                // → fxOrdinaryHasInstance): is the right operand's `.prototype`
                // in the left operand's prototype chain. Stack: [.., left
                // (object), right (constructor)]. A non-callable right operand
                // needs the `Symbol.hasInstance` general path (self-names); a
                // non-object left is simply `false`. Meters the fixed
                // host-frame `Symbol.hasInstance` cost ([`INSTANCEOF_METERING`],
                // 4 computrons) beyond its dispatch.
                XS_CODE_INSTANCEOF => {
                    let right = self.pop();
                    let left = self.pop();
                    let ctor = match right.value {
                        Payload::Reference(r) => r,
                        _ => return Halt::Unsupported(op.name()),
                    };
                    let proto = match self.prototype_of(ctor) {
                        Some(p) => p,
                        // Not a modeled constructor (no `.prototype`): the
                        // general `Symbol.hasInstance` path is a later
                        // increment — self-name rather than answer wrongly.
                        None => return Halt::Unsupported(op.name()),
                    };
                    // The `Symbol.hasInstance` host-frame call is paid for
                    // every operand; an object left additionally reads the
                    // constructor prototype and walks the chain.
                    self.meter.tick_raw(INSTANCEOF_METERING);
                    let result = match left.value {
                        Payload::Reference(x) => {
                            self.meter.tick_raw(INSTANCEOF_OBJECT_METERING);
                            self.prototype_chain_has(x, proto)
                        }
                        _ => false,
                    };
                    self.push(Slot::boolean(result));
                    pc += size as usize;
                }

                // `in` (`XS_CODE_IN`, xsRun.c → fxRunIn → fxHasAt): does the
                // right operand (object) have a property named by the left
                // (key). Stack: [.., left (key), right (object)]. endor answers
                // only the case it can decide soundly: the key resolves to a
                // program symbol whose property is an **own** property of the
                // object ⇒ `true` (metered one built-in step, `fxHasAt`). A
                // key that is *not* an own property cannot be answered `false`
                // safely — endor's per-program symbol table cannot tell a
                // genuinely-absent key from an unreferenced inherited built-in
                // (`'toString' in {}` is `true` in XS), so it self-names rather
                // than risk a wrong `false`. A non-object right operand throws
                // in XS ("in: not an object") — self-name there too.
                XS_CODE_IN => {
                    let obj = self.pop();
                    let key = self.pop();
                    let objref = match obj.value {
                        Payload::Reference(r) => r,
                        _ => return Halt::Unsupported(op.name()),
                    };
                    let id = match key.value {
                        Payload::String(off) => {
                            let s = String::from_utf8_lossy(self.str_content(off)).into_owned();
                            self.symbol_ids.get(&s).copied()
                        }
                        _ => None,
                    };
                    match id.and_then(|i| self.find_property(objref, i)) {
                        Some(_) => {
                            self.meter.tick_raw(IN_METERING);
                            self.push(Slot::boolean(true));
                            pc += size as usize;
                        }
                        None => return Halt::Unsupported(op.name()),
                    }
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
                // `swap` (`XS_CODE_SWAP`): exchange the top two stack slots
                // (`aSlot = mxStack[0]; mxStack[0] = mxStack[1];
                // mxStack[1] = aSlot`). Pure stack, dispatch-metered.
                XS_CODE_SWAP => {
                    let n = self.stack.len();
                    if n >= 2 {
                        self.stack.swap(n - 1, n - 2);
                    }
                    pc += size as usize;
                }
                // Debug / source markers (`line`, `file`, `debugger`,
                // `profile`): semantics-free in the run — a debug build
                // uses them for source mapping and breakpoints, the engine
                // otherwise steps past them. Dispatch-metered (as C-XS
                // meters them under `mxMetering`), no stack/heap effect.
                // The covered grammar's captured bytecode does not emit
                // them, but stubbing keeps decode+dispatch total.
                // (`file` is an ID-operand opcode, `size == 0`, so it must
                // advance by the resolved `ilen`, never `size`.)
                XS_CODE_LINE | XS_CODE_FILE | XS_CODE_DEBUGGER | XS_CODE_PROFILE => {
                    pc += ilen;
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
                    let v = self.pop();
                    let cond = self.truthy(&v);
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
                    let v = self.pop();
                    let cond = self.truthy(&v);
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
                    let v = self.pop();
                    let cond = self.truthy(&v);
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
                    let v = self.pop();
                    let cond = self.truthy(&v);
                    if cond {
                        if off < 0 && self.check_meter() == MeterCheck::Abort {
                            return Halt::MeterAbort;
                        }
                        pc = branch_target(pc, size, off);
                    } else {
                        pc += size as usize;
                    }
                }
                // `branch_coalesce` (`??`, xsRun.c:BRANCH_COALESCE): if the
                // stack top is undefined/null, **pop** it and fall through
                // (evaluate the right operand); otherwise keep it and branch
                // (skip the right operand). The kept-value branch is the
                // `mxBranch`, so it meter-checks on a backward offset.
                XS_CODE_BRANCH_COALESCE_1
                | XS_CODE_BRANCH_COALESCE_2
                | XS_CODE_BRANCH_COALESCE_4 => {
                    let off = match op {
                        XS_CODE_BRANCH_COALESCE_1 => s1!(1),
                        XS_CODE_BRANCH_COALESCE_2 => {
                            i16::from_le_bytes([code[pc + 1], code[pc + 2]]) as i32
                        }
                        _ => i32::from_le_bytes([
                            code[pc + 1],
                            code[pc + 2],
                            code[pc + 3],
                            code[pc + 4],
                        ]),
                    };
                    let top = *self.stack.last().unwrap_or(&Slot::undefined());
                    if matches!(top.kind, Kind::Undefined | Kind::Null) {
                        let _ = self.pop();
                        pc += size as usize;
                    } else {
                        if off < 0 && self.check_meter() == MeterCheck::Abort {
                            return Halt::MeterAbort;
                        }
                        pc = branch_target(pc, size, off);
                    }
                }
                // `branch_chain` (`?.`, xsRun.c:BRANCH_CHAIN): if the stack
                // top is undefined/null, normalize it to undefined and branch
                // (short-circuit the optional chain); otherwise fall through
                // (continue the chain), keeping the value.
                XS_CODE_BRANCH_CHAIN_1
                | XS_CODE_BRANCH_CHAIN_2
                | XS_CODE_BRANCH_CHAIN_4 => {
                    let off = match op {
                        XS_CODE_BRANCH_CHAIN_1 => s1!(1),
                        XS_CODE_BRANCH_CHAIN_2 => {
                            i16::from_le_bytes([code[pc + 1], code[pc + 2]]) as i32
                        }
                        _ => i32::from_le_bytes([
                            code[pc + 1],
                            code[pc + 2],
                            code[pc + 3],
                            code[pc + 4],
                        ]),
                    };
                    let top = *self.stack.last().unwrap_or(&Slot::undefined());
                    if matches!(top.kind, Kind::Undefined | Kind::Null) {
                        if let Some(s) = self.stack.last_mut() {
                            *s = Slot::undefined();
                        }
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
                    if self.call_stack.len() == return_depth {
                        // The frame this dispatch was entered to run has
                        // returned: hand control back to the caller (the C/host
                        // boundary for the top-level program at depth 0, or the
                        // native method driving a callback via `run_callback`).
                        // Construct/`this` return still applies; leave the
                        // result on the value stack for the caller to read.
                        let ret = if self.cur_target && self.result.kind != Kind::Reference {
                            self.this_val
                        } else {
                            self.result
                        };
                        if return_depth != 0 {
                            // A callback frame: pop the activation and push its
                            // result, exactly as a normal `END` does, so
                            // `run_callback` can read it and the caller's
                            // activation is restored.
                            let _ = self.leave_call();
                            self.push(ret);
                        }
                        return Halt::Return;
                    }
                    // Construct return (XS's `END` with `mxFrameHasTarget`):
                    // a constructor's completion is its `this` instance unless
                    // the body explicitly returned an object.
                    let ret = if self.cur_target && self.result.kind != Kind::Reference {
                        self.this_val
                    } else {
                        self.result
                    };
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

                // ---- exceptions: the jump-buffer chain --------------
                // `catch L` (`XS_CODE_CATCH_*`, xsRun.c:1365): establish a
                // handler. Push a jump recording the resume target
                // (`pc + size + offset`), the value-stack/scope cuts, and
                // the call depth — the state a throw longjmps back to.
                // Execution continues into the try body (no branch). The
                // `c_malloc(txJump)` is not a slot allocation, so — like
                // XS — `catch` meters only its dispatch.
                XS_CODE_CATCH_1 | XS_CODE_CATCH_2 | XS_CODE_CATCH_4 => {
                    let off = match op {
                        XS_CODE_CATCH_1 => s1!(1),
                        XS_CODE_CATCH_2 => i16::from_le_bytes([code[pc + 1], code[pc + 2]]) as i32,
                        _ => i32::from_le_bytes([
                            code[pc + 1],
                            code[pc + 2],
                            code[pc + 3],
                            code[pc + 4],
                        ]),
                    };
                    let target = (pc as isize + size as isize + off as isize) as usize;
                    self.jumps.push(CatchJump {
                        target_pc: target,
                        stack_len: self.stack.len(),
                        locals_len: self.locals.len(),
                        id_map: self.id_map.clone(),
                        call_depth: self.call_stack.len(),
                        flag: 1,
                    });
                    pc += size as usize;
                }
                // `uncatch` (`XS_CODE_UNCATCH`, xsRun.c:1440): the try body
                // completed normally; pop the handler off the chain.
                XS_CODE_UNCATCH => {
                    self.jumps.pop();
                    pc += size as usize;
                }
                // `exception` (`XS_CODE_EXCEPTION`, xsRun.c:1359): push the
                // pending thrown value onto the stack (the catch clause
                // binds it) and clear `mxException` back to `undefined`.
                XS_CODE_EXCEPTION => {
                    let ex = self.exception;
                    self.push(ex);
                    self.exception = Slot::undefined();
                    pc += size as usize;
                }
                // `throw` (`XS_CODE_THROW`, xsRun.c:1409): `mxException =
                // *mxStack` (peek), then `fxJump` — unwind to the innermost
                // handler, restoring its recorded state and resuming at its
                // target (a `mxFirstCode` meter check fires on resume). With
                // no handler the throw escapes to the host: `Halt::Throw`.
                XS_CODE_THROW => {
                    let v = *self.stack.last().unwrap_or(&Slot::undefined());
                    self.exception = v;
                    match self.unwind_to_jump() {
                        Some(target) => {
                            pc = target;
                            if self.check_meter() == MeterCheck::Abort {
                                return Halt::MeterAbort;
                            }
                        }
                        None => {
                            self.meter_host_escape();
                            return Halt::Throw(self.render(&v));
                        }
                    }
                }
                // `rethrow` (`XS_CODE_RETHROW`, xsRun.c:1405): re-`fxJump`
                // with the current `mxException` (a finally re-raising a
                // saved throw). Same unwind as `throw`, but the value is
                // already in `mxException` rather than on the stack.
                XS_CODE_RETHROW => {
                    let v = self.exception;
                    match self.unwind_to_jump() {
                        Some(target) => {
                            pc = target;
                            if self.check_meter() == MeterCheck::Abort {
                                return Halt::MeterAbort;
                            }
                        }
                        None => {
                            self.meter_host_escape();
                            return Halt::Throw(self.render(&v));
                        }
                    }
                }
                // `throw_status` (`XS_CODE_THROW_STATUS`, xsRun.c:1423):
                // throw only when the frame's status carries `XS_THROW_STATUS`
                // (a for-in/for-of/optional-chaining status check). The
                // covered grammar never sets a throw status, so this always
                // falls through; it is dispatched (metered) and advances.
                XS_CODE_THROW_STATUS => {
                    pc += size as usize;
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
        let f = self.slots.alloc(Slot::instance(self.function_proto));
        // Recover the function's own name (for `Function.prototype.toString`):
        // a real name id indexes the program's symbol names; `XS_NO_ID` is
        // anonymous.
        let fname = if name != crate::value::XS_NO_ID {
            self.symbol_names
                .get(name as usize - 1)
                .cloned()
                .unwrap_or_default()
        } else {
            String::new()
        };
        self.functions.insert(
            f,
            FuncInfo {
                name: fname,
                ..FuncInfo::default()
            },
        );
        // `fxDefaultFunctionPrototype`: a `constructor_function` gets a default
        // `.prototype` object (chaining to %Object.prototype%) that a later
        // `new f()` uses as the instance prototype and `instanceof` tests
        // against. Its allocation is already folded into the measured
        // [`FUNCTION_DEFINE_METERING`] cluster, so it is created unmetered here.
        let proto = self.slots.alloc(Slot::instance(self.object_proto));
        self.ctor_prototype.insert(f, proto);
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
    fn enter_call(&mut self, argc: usize, ret_pc: usize, has_target: bool) -> Result<usize, Halt> {
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
        // Stack-overflow guard (XS's `fxOverflow` on the callee's frame
        // allocation): entering this call suspends the caller (its frame
        // quartet, args, and scope stay live) and opens a fresh callee
        // frame. If the resulting concurrent slot count would cross the
        // fixed budget, abort to the host exactly as C-XS does — this is
        // what makes unbounded recursion overflow on endor too, rather than
        // completing where C-XS aborts.
        let caller_footprint =
            FRAME_OVERHEAD_SLOTS + self.args.len() + self.locals.len();
        // Opening the callee frame allocates its quartet and argument slots
        // on top of everything currently live (the caller's frame stays
        // suspended on the stack). If that crosses the fixed budget, abort
        // to the host exactly as C-XS's `fxOverflow`.
        if self.would_overflow(FRAME_OVERHEAD_SLOTS + argc) {
            return Err(Halt::StackOverflow(self.stack_slots_in_use()));
        }
        // Unwind the frame region (THIS..last arg).
        self.stack.truncate(base);
        // The caller's frame is now suspended: account its live slots.
        self.frame_slots += caller_footprint;
        // Save the caller's activation and install the callee's.
        self.call_stack.push(CallerState {
            locals: std::mem::take(&mut self.locals),
            id_map: std::mem::take(&mut self.id_map),
            result: self.result,
            strict: self.strict,
            args: std::mem::take(&mut self.args),
            this_val: self.this_val,
            cur_func: self.cur_func,
            cur_target: self.cur_target,
            ret_pc,
        });
        self.result = Slot::undefined();
        self.strict = false;
        self.args = args;
        self.this_val = this_val;
        self.cur_func = func;
        self.cur_target = has_target;
        Ok(body_start)
    }

    /// Synchronously invoke a user-function callback `func` with receiver
    /// `this` and `args`, running its body to `END` and returning its
    /// completion value — the re-entrant substrate the callback-taking
    /// `Array.prototype` methods use (XS's `fxRunCount` per element). It sets
    /// up the callee frame on the shared value stack, enters it, and runs a
    /// nested [`Self::dispatch_at`] that stops when the callback's frame
    /// returns to the current call depth; the caller's activation is restored
    /// exactly as an ordinary return does. A non-user-function callback (a
    /// native, or a non-callable) self-names an honest skip. Propagates a
    /// callback throw / meter abort to the caller.
    fn run_callback(
        &mut self,
        code: &[u8],
        func: Slot,
        this: Slot,
        args: &[Slot],
    ) -> Result<Slot, Halt> {
        // Only a user (bytecode) function is driven here; a native callback or
        // a non-callable is out of the modeled subset.
        match func.value {
            Payload::Reference(f)
                if self.functions.contains_key(&f)
                    && self.functions[&f].native.is_none()
                    && self.functions[&f].method.is_none() => {}
            _ => return Err(Halt::Unsupported("callback:non-user-function")),
        }
        let argc = args.len();
        // Push the callee frame geometry [THIS, FUNCTION, RESULT, FRAME] + args.
        self.push(this);
        self.push(func);
        self.push(Slot::undefined());
        self.push(Slot::of(Kind::Uninitialized, Payload::None));
        for a in args {
            self.push(*a);
        }
        let body_start = self.enter_call(argc, 0, false)?;
        // After `enter_call` the callee frame's `CallerState` is on the call
        // stack; run until its `END` pops the stack back to this depth.
        let return_depth = self.call_stack.len();
        match self.dispatch_at(code, body_start, return_depth) {
            Halt::Return => Ok(self.pop()),
            other => Err(other),
        }
    }

    /// Dispatch a plain (non-`new`) call to an intrinsic native function.
    /// The value stack below the `argc` args holds the frame geometry
    /// `[THIS, FUNCTION, RESULT, FRAME]` beginning at `base`; the handler
    /// reads its arguments, collapses the whole `[THIS..argN-1]` region to a
    /// single result slot (XS's `mxStack = mxFrameEnd; *mxStack =
    /// *mxFrameResult`), and meters exactly what the C built-in meters.
    /// A native whose call behavior endor does not yet model returns
    /// [`Halt::Unsupported`] naming the built-in — an honest skip, never a
    /// mis-executed result.
    fn call_native(
        &mut self,
        native: Native,
        base: usize,
        argc: usize,
        has_target: bool,
    ) -> Result<(), Halt> {
        // Argument i is at `base + 4 + i` (arg0 is the deepest); missing
        // arguments read `undefined`.
        let arg = |i: usize| -> Slot {
            self.stack.get(base + 4 + i).copied().unwrap_or_else(Slot::undefined)
        };
        let result: Slot = match native {
            // `Boolean(value)` (`fx_Boolean`): ToBoolean(argument0), or
            // `false` when called with no argument. Measured against the pin,
            // the primitive coercion meters **no** built-in step beyond the
            // call's dispatch — a `Boolean(x)` costs exactly its opcodes
            // (the argument expression's chunk allocations, if any, are
            // metered where they occur). A `new Boolean` (the wrapper object)
            // is the separate construct path, not yet modeled.
            Native::Boolean if !has_target => {
                let v = arg(0);
                Slot::boolean(self.truthy(&v))
            }
            // `new Boolean(v)` — the wrapper object (`[[BooleanData]]`).
            Native::Boolean => {
                let v = arg(0);
                let prim = Slot::boolean(self.truthy(&v));
                self.build_wrapper(Native::Boolean, prim)
            }
            // `Number(v)` / `new Number(v)`: the primitive number is
            // ToNumber(v). endor handles the numeric fast path (identity) and
            // the primitive `boolean`/`null`/`undefined` coercions — all
            // metering-neutral; a string needs the number parser (a later
            // increment) and self-names. `new` wraps the primitive.
            Native::Number => {
                let a = arg(0);
                let prim = match a.kind {
                    Kind::Integer | Kind::Number => a,
                    Kind::Boolean | Kind::Null | Kind::Undefined if argc >= 1 => {
                        Slot::number(to_number(&a))
                    }
                    _ if argc == 0 => Slot::integer(0),
                    _ => return Err(Halt::Unsupported(native_unsupported_name(native))),
                };
                if has_target {
                    self.build_wrapper(Native::Number, prim)
                } else {
                    prim
                }
            }
            // `String(v)` / `new String(v)`: the primitive string is
            // ToString(v). A string argument is identity (metering-neutral);
            // the general ToString of other kinds is metered via
            // `to_string_bytes_metered`. `new` wraps the primitive.
            Native::String => {
                let prim = if argc == 0 {
                    let off = self.chunks.alloc(b"");
                    Slot::of(Kind::String, Payload::String(off))
                } else {
                    let a = arg(0);
                    match a.kind {
                        Kind::String => a,
                        Kind::Reference => {
                            return Err(Halt::Unsupported(native_unsupported_name(native)))
                        }
                        _ => {
                            let bytes = self.to_string_bytes_metered(a);
                            let off = self.chunks.alloc(&bytes);
                            Slot::of(Kind::String, Payload::String(off))
                        }
                    }
                };
                if has_target {
                    self.build_wrapper(Native::String, prim)
                } else {
                    prim
                }
            }
            // `Object([value])` / `new Object([value])` (`fx_Object`): with no
            // argument (or `undefined`/`null`), create a fresh empty ordinary
            // object; with an object argument, return it unchanged (ToObject
            // identity). Both the call and construct forms behave and meter
            // identically here (verified: same raw). Wrapping a *primitive*
            // argument (a Number/String/Boolean object) needs the wrapper
            // path and self-names. Metering measured against the pin: one
            // `fxNewObject` ([`Self::new_object`], 16640) plus one extra
            // built-in step ([`crate::meter::Meter::tick_builtin`], 16384) —
            // 33024 raw total, the fractional gap over a bare object literal.
            Native::Object => {
                let a = arg(0);
                match a.kind {
                    Kind::Reference => a,
                    Kind::Undefined | Kind::Null => {
                        self.meter.tick_builtin();
                        let inst = self.new_object();
                        Slot::of(Kind::Reference, Payload::Reference(inst))
                    }
                    // A primitive → its wrapper object (Number/String/Boolean
                    // object): the wrapper machinery is a later increment.
                    _ => return Err(Halt::Unsupported(native_unsupported_name(native))),
                }
            }
            // The Error hierarchy (`fx_Error` and the per-type constructors):
            // `new TypeError(msg)` / `TypeError(msg)` both build a fresh error
            // instance carrying the type's `name` and, when a message argument
            // is given, an own `message` property set to ToString(message).
            // This is what graduates abort-value parity: a thrown error's
            // completion/abort value stringifies as `name` or `name: message`
            // (XS's `Error.prototype.toString`), not a primitive. `has_target`
            // is immaterial — an Error called as a function constructs too.
            Native::Error => self.build_error("Error", base, argc),
            Native::EvalError => self.build_error("EvalError", base, argc),
            Native::RangeError => self.build_error("RangeError", base, argc),
            Native::ReferenceError => self.build_error("ReferenceError", base, argc),
            Native::SyntaxError => self.build_error("SyntaxError", base, argc),
            Native::TypeError => self.build_error("TypeError", base, argc),
            Native::URIError => self.build_error("URIError", base, argc),
            // `Symbol([description])`: a fresh unique symbol. Its descriptor
            // slot holds the description (or `undefined`), and its identity is
            // that slot — so `Symbol('a') !== Symbol('a')`. Metering-neutral,
            // like the other primitive coercions (measured against the pin).
            // `new Symbol()` throws in JS; a `has_target` call self-names.
            Native::Symbol if !has_target => {
                let desc = arg(0);
                let d = self.slots.alloc(desc);
                self.meter.tick_raw(SYMBOL_CREATE_METERING);
                Slot::of(Kind::Symbol, Payload::Reference(d))
            }
            // `Array(...)` / `new Array(...)` (`fx_Array`): both forms build the
            // same array. A single number argument is the length (a holey
            // array of that length); a single non-number, or two-or-more
            // arguments, are the elements. Metering measured against the pin
            // `48ee02d8cfe0`: a constant constructor base ([`ARRAY_CTOR_BASE_METERING`],
            // covering the native host frame, `fxGetPrototypeFromConstructor`,
            // and `fxNewArrayInstance`) plus, for the element forms, one
            // item-chunk allocation of `count` slots (a single `fxSetIndexSize`,
            // not per-item growth).
            Native::Array => {
                self.meter.tick_raw(ARRAY_CTOR_BASE_METERING);
                let inst = self.slots.alloc(Slot::instance(self.array_proto));
                let mut data = ArrayData::default();
                if argc == 1 {
                    let a = arg(0);
                    match a.kind {
                        Kind::Integer | Kind::Number => match self.checked_array_length(a) {
                            Some(n) => data.length = n,
                            // A non-length number (`Array(2.5)`, `Array(-1)`)
                            // is a `RangeError` in XS — its abort value and
                            // metering are a later increment; honest skip.
                            None => return Err(Halt::Unsupported("native-call:Array:bad-length")),
                        },
                        _ => {
                            self.meter.tick_raw(self.array_chunk_size_metering(1));
                            let mut v = a;
                            v.id = 0;
                            v.next = crate::value::SlotIndex::NULL;
                            data.items.insert(0, v);
                            data.length = 1;
                        }
                    }
                } else if argc >= 2 {
                    self.meter
                        .tick_raw(self.array_chunk_size_metering(argc as u32));
                    for i in 0..argc {
                        let mut v = arg(i);
                        v.id = 0;
                        v.next = crate::value::SlotIndex::NULL;
                        data.items.insert(i as u32, v);
                    }
                    data.length = argc as u32;
                }
                self.arrays.insert(inst, data);
                Slot::of(Kind::Reference, Payload::Reference(inst))
            }
            // The remaining fundamentals constructors' call/coerce/construct
            // behaviors land incrementally; until then they self-name so the
            // differential runner records an honest skip.
            _ => return Err(Halt::Unsupported(native_unsupported_name(native))),
        };
        // Collapse the call region to the single result (frame teardown).
        let _ = argc;
        self.stack.truncate(base);
        self.push(result);
        Ok(())
    }

    /// Build a fresh Error instance of type `name` from a native Error
    /// constructor call/construct (`fx_Error`). Meters the construct cost
    /// (the native `Object` object cost plus [`ERROR_CONSTRUCT_EXTRA`]) and,
    /// when a message argument is present, ToString's it into an own
    /// `message` property ([`ERROR_MESSAGE_METERING`]). Records the
    /// `(name, message)` in [`Self::error_data`] so the value stringifies as
    /// `name` / `name: message`, and sets own `name`/`message` properties
    /// (under the program's relinked ids) so guest reads resolve — both
    /// unmetered, mirroring XS where `name` is the inherited prototype value
    /// and the property slot cost is folded into the measured constants.
    fn build_error(&mut self, name: &'static str, base: usize, argc: usize) -> Slot {
        // Base object cost, exactly as the native `Object` constructor
        // (`tick_builtin` + `fxNewObject`), plus the error-instance extra.
        self.meter.tick_builtin();
        let inst = self.new_object();
        self.meter.tick_raw(ERROR_CONSTRUCT_EXTRA);
        // Chain the error instance to its type's `%<Type>.prototype%` (so
        // `err instanceof TypeError` / `instanceof Error` hold) rather than
        // the plain `%Object.prototype%` `new_object` defaulted it to.
        if let Some(proto) = self.intrinsics.get(name).and_then(|&c| self.prototype_of(c)) {
            self.slots.get_mut(inst).value = Payload::Reference(proto);
        }
        // The message argument: absent or `undefined` ⇒ no own message (XS
        // inherits `Error.prototype.message == ""`).
        let message: Option<String> = if argc >= 1 {
            let a = self.stack.get(base + 4).copied().unwrap_or_else(Slot::undefined);
            if a.kind == Kind::Undefined {
                None
            } else {
                let bytes = self.to_string_bytes_metered(a);
                self.meter.tick_raw(ERROR_MESSAGE_METERING);
                Some(String::from_utf8_lossy(&bytes).into_owned())
            }
        } else {
            None
        };
        self.error_data.insert(
            inst,
            ErrorInfo {
                name,
                message: message.clone(),
            },
        );
        // An own `message` property only when a message argument was given
        // (XS): a no-argument error inherits `message == ""` from the
        // prototype. `name` is always inherited from the prototype, never own
        // — so `err.hasOwnProperty('name')` is `false`, matching XS. Both are
        // set unmetered (the own message slot cost is folded into the
        // measured construct constants).
        if let Some(text) = message {
            if let Some(&mid) = self.symbol_ids.get("message") {
                let off = self.chunks.alloc(text.as_bytes());
                self.set_own_unmetered(inst, mid, Slot::of(Kind::String, Payload::String(off)));
            }
        }
        Slot::of(Kind::Reference, Payload::Reference(inst))
    }

    /// Build a primitive-wrapper object (`new Boolean`/`Number`/`String`)
    /// around the already-computed primitive `prim`. Meters the native
    /// `Object` empty-object cost plus [`WRAPPER_CONSTRUCT_EXTRA`], chains the
    /// wrapper to the constructor's `%X.prototype%` (so it is `instanceof X`),
    /// and records the wrapped primitive so it stringifies as the primitive.
    fn build_wrapper(&mut self, native: Native, prim: Slot) -> Slot {
        self.meter.tick_builtin();
        let inst = self.new_object();
        self.meter.tick_raw(WRAPPER_CONSTRUCT_EXTRA);
        if let Some(proto) = self.intrinsics.get(native.display_name()).and_then(|&c| self.prototype_of(c)) {
            self.slots.get_mut(inst).value = Payload::Reference(proto);
        }
        self.wrapper_data.insert(inst, prim);
        Slot::of(Kind::Reference, Payload::Reference(inst))
    }

    /// Insert or overwrite an own property `id = value` on `inst` **without**
    /// metering — for intrinsic-supplied properties whose cost is either an
    /// inherited prototype value (unmetered in XS) or already folded into a
    /// measured construct constant.
    fn set_own_unmetered(&mut self, inst: crate::value::SlotIndex, id: u16, value: Slot) {
        if let Some(p) = self.find_property(inst, id) {
            let s = self.slots.get_mut(p);
            s.kind = value.kind;
            s.value = value.value;
        } else {
            let head = self.slots.get(inst).next;
            let mut prop = value;
            prop.id = id;
            prop.flag = 0;
            prop.next = head;
            let idx = self.slots.alloc(prop);
            self.slots.get_mut(inst).next = idx;
        }
    }

    /// `Function.prototype.call` trampoline: reshape the call frame from
    /// `[f, callMethod, RESULT, FRAME, thisArg, args…]` into a direct call
    /// `[thisArg, f, RESULT, FRAME, args…]` and enter the receiver's body,
    /// so the receiver runs with `thisArg` as `this` and the trailing
    /// arguments, resuming the caller after this `run`. The receiver must be
    /// a user function (a native/method receiver self-names). Meters the fixed
    /// `.call` re-dispatch overhead ([`CALL_TRAMPOLINE_METERING`]) beyond the
    /// visible opcodes and the callee body.
    fn enter_call_dot_call(
        &mut self,
        base: usize,
        argc: usize,
        ret_pc: usize,
    ) -> Result<usize, Halt> {
        let f = self.stack.get(base).copied().unwrap_or_else(Slot::undefined);
        let fref = match f.value {
            Payload::Reference(r)
                if self
                    .functions
                    .get(&r)
                    .map_or(false, |fi| fi.native.is_none() && fi.method.is_none()) =>
            {
                r
            }
            _ => return Err(Halt::Unsupported("call:non-user-function-receiver")),
        };
        let _ = fref;
        let this_arg = self
            .stack
            .get(base + 4)
            .copied()
            .unwrap_or_else(Slot::undefined);
        // A primitive `thisArg` is boxed to its wrapper object in a sloppy
        // callee (XS's `fxToInstance`) but left as-is in a strict callee — a
        // meter-affecting distinction endor does not yet model, and the
        // callee's strictness is not known until its `begin`. Self-name for a
        // primitive `thisArg` rather than answer a `this`-dependent test
        // wrongly; `undefined`/`null` (→ global / kept) and an object
        // `thisArg` are handled.
        if !matches!(
            this_arg.kind,
            Kind::Undefined | Kind::Null | Kind::Reference
        ) {
            return Err(Halt::Unsupported("call:primitive-this-boxing"));
        }
        let real_args: Vec<Slot> = if argc >= 1 {
            self.stack[base + 5..base + 4 + argc].to_vec()
        } else {
            Vec::new()
        };
        let n = real_args.len();
        self.stack.truncate(base);
        self.stack.push(this_arg); // THIS
        self.stack.push(f); // FUNCTION (the receiver)
        self.stack.push(Slot::undefined()); // RESULT
        self.stack.push(Slot::of(Kind::Uninitialized, Payload::None)); // FRAME
        for a in real_args {
            self.stack.push(a);
        }
        self.meter
            .tick_raw(CALL_TRAMPOLINE_METERING + n as u64 * CALL_TRAMPOLINE_PER_ARG);
        self.enter_call(n, ret_pc, false)
    }

    /// `Function.prototype.apply` (no-array subset): invoke the receiver with
    /// the rebound `this` and **no** arguments — the case where the arguments
    /// array is absent, `undefined`, or `null`. An actual arguments array (a
    /// reference) self-names: reading its elements is child-3 Array machinery.
    /// Identical to `call` with zero arguments (same trampoline, same meter).
    fn enter_call_dot_apply(
        &mut self,
        base: usize,
        argc: usize,
        ret_pc: usize,
    ) -> Result<usize, Halt> {
        let f = self.stack.get(base).copied().unwrap_or_else(Slot::undefined);
        match f.value {
            Payload::Reference(r)
                if self
                    .functions
                    .get(&r)
                    .map_or(false, |fi| fi.native.is_none() && fi.method.is_none()) => {}
            _ => return Err(Halt::Unsupported("apply:non-user-function-receiver")),
        }
        let this_arg = self
            .stack
            .get(base + 4)
            .copied()
            .unwrap_or_else(Slot::undefined);
        if !matches!(
            this_arg.kind,
            Kind::Undefined | Kind::Null | Kind::Reference
        ) {
            return Err(Halt::Unsupported("apply:primitive-this-boxing"));
        }
        // The arguments array (the second argument): only absent/undefined/null
        // is the no-array subset; a real array self-names (child-3 Array read).
        let arg_array = self.stack.get(base + 5).copied();
        match arg_array.map(|s| s.kind) {
            None | Some(Kind::Undefined) | Some(Kind::Null) => {}
            _ => return Err(Halt::Unsupported("apply:arguments-array")),
        }
        self.stack.truncate(base);
        self.stack.push(this_arg); // THIS
        self.stack.push(f); // FUNCTION (the receiver)
        self.stack.push(Slot::undefined()); // RESULT
        self.stack.push(Slot::of(Kind::Uninitialized, Payload::None)); // FRAME
        self.meter.tick_raw(CALL_TRAMPOLINE_METERING);
        self.enter_call(0, ret_pc, false)
    }

    /// Dispatch a native prototype **method** call (`obj.toString()`,
    /// `obj.hasOwnProperty(k)`, `wrapper.valueOf()`, …). The value stack holds
    /// the call frame `[THIS, FUNCTION, RESULT, FRAME]` from `base`; `THIS` is
    /// the receiver. Computes the result from the receiver (no re-entry into
    /// user code), meters the method's steps, collapses the region to the
    /// result, and pushes it. A method whose receiver shape endor cannot model
    /// self-names (an honest skip).
    fn call_native_method(
        &mut self,
        m: NativeMethod,
        base: usize,
        argc: usize,
        code: &[u8],
    ) -> Result<(), Halt> {
        let _ = code; // used by the callback-taking methods (run_callback)
        let this = self.stack.get(base).copied().unwrap_or_else(Slot::undefined);
        let arg0 = self.stack.get(base + 4).copied().unwrap_or_else(Slot::undefined);
        let _ = argc;
        let result: Slot = match m {
            // `Function.prototype.call` is handled by the `run` trampoline
            // (`enter_call_dot_call`) and never reaches here.
            NativeMethod::FunctionCall => return Err(Halt::Unsupported("call:unexpected")),
            // `Function.prototype.apply` is handled by the `run` trampoline
            // (`enter_call_dot_apply`) and never reaches here.
            NativeMethod::FunctionApply => return Err(Halt::Unsupported("apply:unexpected")),
            // `Object.prototype.valueOf`: returns the receiver unchanged.
            NativeMethod::ObjectValueOf => this,
            // `<wrapper>.valueOf`: the wrapped primitive.
            NativeMethod::WrapperValueOf => match this.value {
                Payload::Reference(r) => self.wrapper_data.get(&r).copied().unwrap_or(this),
                _ => this,
            },
            // `Object.prototype.toString`: `[object Object]` for an ordinary
            // object (the exotic tags — Array/Error/… — are XS overrides or a
            // later increment). Allocates the result string chunk.
            NativeMethod::ObjectToString => {
                self.meter.tick_raw(METHOD_OBJECT_TOSTRING_METERING);
                self.meter.tick_chunk_new(b"[object Object]".len() as u64);
                let off = self.chunks.alloc(b"[object Object]");
                Slot::of(Kind::String, Payload::String(off))
            }
            // `Function.prototype.toString`: XS renders any function as
            // `function ["name"] (){[native code]}`.
            NativeMethod::FunctionToString => {
                let name = match this.value {
                    Payload::Reference(r) => self
                        .functions
                        .get(&r)
                        .map(|fi| fi.name.clone())
                        .unwrap_or_default(),
                    _ => String::new(),
                };
                self.meter.tick_raw(METHOD_FUNCTION_TOSTRING_METERING);
                let s = format!("function [\"{}\"] (){{[native code]}}", name);
                self.meter.tick_chunk_new(s.len() as u64);
                let off = self.chunks.alloc(s.as_bytes());
                Slot::of(Kind::String, Payload::String(off))
            }
            // `Error.prototype.toString`: `name` / `name: message`.
            NativeMethod::ErrorToString => {
                let s = self.render(&this);
                self.meter.tick_raw(METHOD_ERROR_TOSTRING_METERING);
                self.meter.tick_chunk_new(s.len() as u64);
                let off = self.chunks.alloc(s.as_bytes());
                Slot::of(Kind::String, Payload::String(off))
            }
            // `<wrapper>.toString`: stringify the wrapped primitive with the
            // same per-type ToString metering the `String(v)` call uses (a
            // number renders through `fxNumberToString` — one built-in step
            // plus its chunk; a boolean/string is interned/identity, no cost).
            NativeMethod::WrapperToString => {
                let prim = match this.value {
                    Payload::Reference(r) => self.wrapper_data.get(&r).copied(),
                    _ => None,
                }
                .unwrap_or(this);
                let bytes = self.to_string_bytes_metered(prim);
                let off = self.chunks.alloc(&bytes);
                Slot::of(Kind::String, Payload::String(off))
            }
            // `Object.prototype.hasOwnProperty(k)`: is `k` an OWN property.
            // A key that is not a program symbol cannot be an own property
            // (own keys are interned symbol ids) ⇒ `false` — safe, unlike
            // `in`, because this never consults the prototype chain.
            NativeMethod::ObjectHasOwnProperty => {
                // Only a key that is already a program symbol is answered:
                // find it among the receiver's OWN properties (never the
                // prototype chain), bit-exact. A string-literal key that is
                // not a program symbol self-names — endor's per-program symbol
                // table cannot tell a genuinely-absent key from a
                // native-created own property under a global id (an error's
                // `message`), nor whether interning it costs `fxNewName`.
                let (o, id) = match (this.value, arg0.value) {
                    (Payload::Reference(o), Payload::String(off)) => {
                        let key = String::from_utf8_lossy(self.str_content(off)).into_owned();
                        match self.symbol_ids.get(&key) {
                            Some(&id) => (o, id),
                            None => return Err(Halt::Unsupported("hasOwnProperty:non-symbol-key")),
                        }
                    }
                    _ => return Err(Halt::Unsupported("hasOwnProperty:non-string-key")),
                };
                self.meter.tick_raw(METHOD_HAS_OWN_PROPERTY_METERING);
                Slot::boolean(self.find_property(o, id).is_some())
            }
            // `Object.prototype.isPrototypeOf(v)`: is the receiver in `v`'s
            // prototype chain.
            NativeMethod::ObjectIsPrototypeOf => {
                let r = match (this.value, arg0.value) {
                    (Payload::Reference(proto), Payload::Reference(o)) => {
                        self.prototype_chain_has(o, proto)
                    }
                    _ => false,
                };
                self.meter.tick_raw(METHOD_HAS_OWN_PROPERTY_METERING);
                Slot::boolean(r)
            }
            // `Array.prototype.push(...items)` — dense fast path only.
            NativeMethod::ArrayPush => {
                let inst = match self.dense_array_this(this) {
                    Some(i) => i,
                    None => return Err(Halt::Unsupported("push:non-dense-array")),
                };
                let args: Vec<Slot> = (0..argc)
                    .map(|i| self.stack.get(base + 4 + i).copied().unwrap_or_else(Slot::undefined))
                    .collect();
                let c = args.len() as u32;
                let length = self.arrays[&inst].length;
                // `mxMeterSome(2)` + the grow to `length + c`
                // (`fxSetIndexSize`, growable chunk) + `mxMeterSome(5)` per
                // appended item + a closing `mxMeterSome(2)`, plus the fixed
                // native-method frame constant.
                self.meter.tick_raw(ARRAY_PUSH_FRAME_METERING);
                self.meter.tick_builtin_some(2);
                if c > 0 {
                    self.meter
                        .tick_raw(self.array_chunk_size_metering(length + c));
                }
                for (i, a) in args.into_iter().enumerate() {
                    let idx = length + i as u32;
                    let mut v = a;
                    v.id = 0;
                    v.next = crate::value::SlotIndex::NULL;
                    self.arrays.get_mut(&inst).unwrap().items.insert(idx, v);
                    self.meter.tick_builtin_some(5);
                }
                let a = self.arrays.get_mut(&inst).unwrap();
                a.length = length + c;
                self.meter.tick_builtin_some(2);
                Slot::integer((length + c) as i32)
            }
            // `Array.prototype.pop()` — dense fast path only.
            NativeMethod::ArrayPop => {
                let inst = match self.dense_array_this(this) {
                    Some(i) => i,
                    None => return Err(Halt::Unsupported("pop:non-dense-array")),
                };
                self.meter.tick_raw(ARRAY_POP_FRAME_METERING);
                let length = self.arrays[&inst].length;
                self.meter.tick_builtin_some(2);
                let result = if length > 0 {
                    let new_len = length - 1;
                    let removed = self
                        .arrays
                        .get_mut(&inst)
                        .unwrap()
                        .items
                        .remove(&new_len)
                        .unwrap_or_else(Slot::undefined);
                    // `fxSetIndexSize(length-1, XS_CHUNK)` reallocs the item
                    // chunk down; `mxMeterSome(8)`.
                    self.meter.tick_raw(self.array_chunk_size_metering(new_len));
                    self.meter.tick_builtin_some(8);
                    self.arrays.get_mut(&inst).unwrap().length = new_len;
                    Slot::of(removed.kind, removed.value)
                } else {
                    Slot::undefined()
                };
                self.meter.tick_builtin_some(4);
                result
            }
            // `Array.prototype.indexOf(value[, from])` — dense fast path.
            NativeMethod::ArrayIndexOf => {
                let inst = match self.dense_array_this(this) {
                    Some(i) => i,
                    None => return Err(Halt::Unsupported("indexOf:non-dense-array")),
                };
                let target = arg0;
                let (found, steps) = {
                    let a = &self.arrays[&inst];
                    let mut found: i32 = -1;
                    let mut steps: u64 = 0;
                    for i in 0..a.length {
                        steps += 1;
                        if let Some(item) = a.items.get(&i) {
                            if self.strict_equal(item, &target) {
                                found = i as i32;
                                break;
                            }
                        }
                    }
                    (found, steps)
                };
                self.meter.tick_raw(ARRAY_METHOD_INDEXOF_FRAME_METERING);
                // `ARRAY_INDEXOF_PER_STEP` is already in raw 16.16 units.
                self.meter.tick_raw(steps * ARRAY_INDEXOF_PER_STEP);
                Slot::integer(found)
            }
            // `Array.prototype.includes(value[, from])` — dense fast path. Scan
            // from `from` (default 0) by SameValueZero; `true` on the first
            // match, else `false`. Metered like `indexOf` (a frame constant +
            // per-element scan step), calibrated against the pin.
            NativeMethod::ArrayIncludes => {
                let inst = match self.dense_array_this(this) {
                    Some(i) => i,
                    None => return Err(Halt::Unsupported("includes:non-dense-array")),
                };
                let target = arg0;
                let from = self.arg_to_index(base, 1, 0, self.arrays[&inst].length);
                let (found, steps) = {
                    let a = &self.arrays[&inst];
                    let mut found = false;
                    let mut steps: u64 = 0;
                    for i in from..a.length {
                        steps += 1;
                        let item = a.items.get(&i).copied().unwrap_or_else(Slot::undefined);
                        if self.same_value_zero(&item, &target) {
                            found = true;
                            break;
                        }
                    }
                    (found, steps)
                };
                self.meter.tick_raw(ARRAY_INCLUDES_FRAME_METERING);
                self.meter.tick_raw(steps * ARRAY_INCLUDES_PER_STEP);
                Slot::boolean(found)
            }
            // `Array.prototype.lastIndexOf(value[, from])` — dense fast path.
            // Scan backward from the end by strict equality; the last matching
            // index, or `-1`.
            NativeMethod::ArrayLastIndexOf => {
                let inst = match self.dense_array_this(this) {
                    Some(i) => i,
                    None => return Err(Halt::Unsupported("lastIndexOf:non-dense-array")),
                };
                let target = arg0;
                let length = self.arrays[&inst].length;
                let (found, steps) = {
                    let a = &self.arrays[&inst];
                    let mut found: i32 = -1;
                    let mut steps: u64 = 0;
                    let mut i = length;
                    while i > 0 {
                        i -= 1;
                        steps += 1;
                        if let Some(item) = a.items.get(&i) {
                            if self.strict_equal(item, &target) {
                                found = i as i32;
                                break;
                            }
                        }
                    }
                    (found, steps)
                };
                self.meter.tick_raw(ARRAY_LASTINDEXOF_FRAME_METERING);
                self.meter.tick_raw(steps * ARRAY_LASTINDEXOF_PER_STEP);
                Slot::integer(found)
            }
            // `Array.prototype.fill(value[, start[, end]])` — dense fast path.
            // Set `[start, end)` to `value` and return the array. A full fill
            // (`start == 0 && end == length`) reallocs the item chunk
            // (`fxSetIndexSize`); each written element meters `mxMeterSome(5)`.
            NativeMethod::ArrayFill => {
                let inst = match self.dense_array_this(this) {
                    Some(i) => i,
                    None => return Err(Halt::Unsupported("fill:non-dense-array")),
                };
                let value = if argc > 0 { arg0 } else { Slot::undefined() };
                let length = self.arrays[&inst].length;
                let start = self.arg_to_index(base, 1, 0, length);
                let end = self.arg_to_index(base, 2, length, length);
                self.meter.tick_raw(ARRAY_FILL_FRAME_METERING);
                // A full fill runs `fxSetIndexSize(length)`, but for an
                // already-dense array the chunk is already that size, so the
                // resize is a no-op and meters nothing.
                let _ = (start, end, length);
                let mut v = value;
                v.id = 0;
                v.next = crate::value::SlotIndex::NULL;
                for i in start..end {
                    self.arrays.get_mut(&inst).unwrap().items.insert(i, v);
                    self.meter.tick_builtin_some(5);
                }
                this
            }
            // `Array.prototype.reverse()` — reverse the elements in place and
            // return the array. XS reverses via the generic `mxHasAt`/`mxGetAt`/
            // `mxSetAt` path; metering is a frame constant plus a per-swap cost
            // (`length/2` swaps), calibrated against the pin.
            NativeMethod::ArrayReverse => {
                let inst = match self.dense_array_this(this) {
                    Some(i) => i,
                    None => return Err(Halt::Unsupported("reverse:non-dense-array")),
                };
                let length = self.arrays[&inst].length;
                self.meter.tick_raw(ARRAY_REVERSE_FRAME_METERING);
                let swaps = (length / 2) as u64;
                self.meter.tick_raw(swaps * ARRAY_REVERSE_PER_SWAP_METERING);
                let a = self.arrays.get_mut(&inst).unwrap();
                let mut lo = 0u32;
                let mut hi = length.saturating_sub(1);
                while lo < hi {
                    let l = a.items.remove(&lo);
                    let h = a.items.remove(&hi);
                    if let Some(h) = h {
                        a.items.insert(lo, h);
                    }
                    if let Some(l) = l {
                        a.items.insert(hi, l);
                    }
                    lo += 1;
                    hi -= 1;
                }
                this
            }
            // `Array.prototype.slice([start[, end]])` — dense fast path. A new
            // array with the elements of `[start, end)`. Metering: a frame
            // constant, plus (when the slice is non-empty) the result chunk
            // and `mxMeterSome(count*10)`, plus a closing `mxMeterSome(3)`.
            NativeMethod::ArraySlice => {
                let inst = match self.dense_array_this(this) {
                    Some(i) => i,
                    None => return Err(Halt::Unsupported("slice:non-dense-array")),
                };
                let length = self.arrays[&inst].length;
                let start = self.arg_to_index(base, 0, 0, length);
                let end = self.arg_to_index(base, 1, length, length);
                let count = end.saturating_sub(start);
                self.meter.tick_raw(ARRAY_SLICE_FRAME_METERING);
                let result = self.new_array_unmetered();
                if count > 0 {
                    self.meter.tick_raw(self.array_chunk_size_metering(count));
                    self.meter.tick_builtin_some((count as u64) * 10);
                    let items: Vec<(u32, Slot)> = {
                        let a = &self.arrays[&inst];
                        (0..count)
                            .filter_map(|i| a.items.get(&(start + i)).map(|s| (i, *s)))
                            .collect()
                    };
                    let a = self.arrays.get_mut(&result).unwrap();
                    for (i, s) in items {
                        a.items.insert(i, Slot::of(s.kind, s.value));
                    }
                    a.length = count;
                }
                self.meter.tick_builtin_some(3);
                Slot::of(Kind::Reference, Payload::Reference(result))
            }
            // `Array.prototype.concat(...args)` — dense fast path. A new array
            // of the receiver's elements followed by each argument: an array
            // argument (concat-spreadable) contributes its elements, any other
            // value is appended as one element. Dense receivers/array-args only
            // (a hole self-names — the uninitialized-slot accounting is a later
            // increment). Metering models `fxNewInstance` (the list) + a
            // Symbol.isConcatSpreadable check per reference operand + a key slot
            // and `mxMeterSome(2)` per spread element + a key slot and
            // `mxMeterSome(4)` per appended value + the result chunk +
            // `mxMeterSome(3)`, plus a frame constant.
            NativeMethod::ArrayConcat => {
                let recv = match this.value {
                    Payload::Reference(i) if self.arrays.contains_key(&i) => i,
                    _ => return Err(Halt::Unsupported("concat:non-array-receiver")),
                };
                // Collect the operands: the receiver, then each argument.
                let mut operands: Vec<Slot> = vec![this];
                for i in 0..argc {
                    operands.push(
                        self.stack
                            .get(base + 4 + i)
                            .copied()
                            .unwrap_or_else(Slot::undefined),
                    );
                }
                self.meter.tick_raw(ARRAY_CONCAT_FRAME_METERING);
                self.meter.tick_slot_alloc(); // `fxNewInstance` (the list)
                let result = self.new_array_unmetered();
                let mut out: Vec<Slot> = Vec::new();
                for op in operands {
                    // Every reference operand runs the `Symbol.isConcatSpreadable`
                    // check.
                    let is_array = matches!(op.value, Payload::Reference(r) if self.arrays.contains_key(&r));
                    if let Payload::Reference(_) = op.value {
                        self.meter.tick_raw(ARRAY_CONCAT_CHECK_METERING);
                    }
                    if is_array {
                        let r = match op.value {
                            Payload::Reference(r) => r,
                            _ => unreachable!(),
                        };
                        // Dense array only (a hole needs the uninitialized-slot
                        // path).
                        let (len, dense) = {
                            let a = &self.arrays[&r];
                            (a.length, a.items.len() as u32 == a.length)
                        };
                        if !dense {
                            return Err(Halt::Unsupported("concat:sparse-arg"));
                        }
                        for i in 0..len {
                            let s = self.arrays[&r].items.get(&i).copied().unwrap_or_else(Slot::undefined);
                            self.meter.tick_slot_alloc();
                            self.meter.tick_builtin_some(2);
                            self.meter.tick_raw(ARRAY_CONCAT_SPREAD_EXTRA_METERING);
                            out.push(Slot::of(s.kind, s.value));
                        }
                    } else {
                        // A non-array value is appended as a single element.
                        self.meter.tick_slot_alloc();
                        self.meter.tick_builtin_some(4);
                        self.meter.tick_raw(ARRAY_CONCAT_PRIM_EXTRA_METERING);
                        out.push(op);
                    }
                }
                let total = out.len() as u32;
                if total > 0 {
                    self.meter.tick_raw(self.array_chunk_size_metering(total));
                }
                {
                    let a = self.arrays.get_mut(&result).unwrap();
                    for (i, s) in out.into_iter().enumerate() {
                        a.items.insert(i as u32, s);
                    }
                    a.length = total;
                }
                self.meter.tick_builtin_some(3);
                let _ = recv;
                Slot::of(Kind::Reference, Payload::Reference(result))
            }
            // `Array.prototype.at(index)` — dense fast path. Relative index
            // (negative counts from the end); the element there, or
            // `undefined`. Metering: a frame constant, plus (when in range) the
            // element read (`mxGetAt`).
            NativeMethod::ArrayAt => {
                let inst = match self.dense_array_this(this) {
                    Some(i) => i,
                    None => return Err(Halt::Unsupported("at:non-dense-array")),
                };
                self.meter.tick_raw(ARRAY_AT_FRAME_METERING);
                let length = self.arrays[&inst].length as i64;
                let raw = match numeric_of(&arg0) {
                    Some(n) if !n.is_nan() => n.trunc() as i64,
                    _ => 0,
                };
                let idx = if raw < 0 { length + raw } else { raw };
                let result = if idx >= 0 && idx < length {
                    self.meter.tick_raw(ARRAY_AT_READ_METERING);
                    self.arrays
                        .get(&inst)
                        .and_then(|a| a.items.get(&(idx as u32)).copied())
                        .map(|s| Slot::of(s.kind, s.value))
                        .unwrap_or_else(Slot::undefined)
                } else {
                    Slot::undefined()
                };
                result
            }
            // `Array.prototype.shift()` — dense fast path. Remove and return
            // the first element, shifting the rest down and shrinking the item
            // chunk. Metering: `mxMeterSome(2 + 3 + 3 + 4)` when non-empty
            // (else 2+4), the shrink chunk, and `mxMeterSome((length-1)*10)`.
            NativeMethod::ArrayShift => {
                let inst = match self.dense_array_this(this) {
                    Some(i) => i,
                    None => return Err(Halt::Unsupported("shift:non-dense-array")),
                };
                let length = self.arrays[&inst].length;
                self.meter.tick_builtin_some(2);
                let result = if length > 0 {
                    self.meter.tick_builtin_some(3);
                    let new_len = length - 1;
                    let removed = {
                        let a = self.arrays.get_mut(&inst).unwrap();
                        let first = a.items.remove(&0).unwrap_or_else(Slot::undefined);
                        let mut shifted = std::collections::BTreeMap::new();
                        for (&k, &v) in a.items.iter() {
                            shifted.insert(k - 1, v);
                        }
                        a.items = shifted;
                        a.length = new_len;
                        first
                    };
                    self.meter.tick_raw(self.array_chunk_size_metering(new_len));
                    self.meter.tick_builtin_some((new_len as u64) * 10);
                    self.meter.tick_builtin_some(3);
                    Slot::of(removed.kind, removed.value)
                } else {
                    Slot::undefined()
                };
                self.meter.tick_builtin_some(4);
                result
            }
            // `Array.prototype.unshift(...items)` — dense fast path. Prepend the
            // arguments, shifting existing elements up, and return the new
            // length. Metering: the grow chunk, `mxMeterSome(length*10)` for the
            // shift, `mxMeterSome(4)` per inserted argument, `mxMeterSome(2)`.
            NativeMethod::ArrayUnshift => {
                let inst = match self.dense_array_this(this) {
                    Some(i) => i,
                    None => return Err(Halt::Unsupported("unshift:non-dense-array")),
                };
                let length = self.arrays[&inst].length;
                let c = argc as u32;
                let args: Vec<Slot> = (0..argc)
                    .map(|i| {
                        self.stack
                            .get(base + 4 + i)
                            .copied()
                            .unwrap_or_else(Slot::undefined)
                    })
                    .collect();
                self.meter.tick_raw(ARRAY_UNSHIFT_FRAME_METERING);
                if c > 0 {
                    self.meter
                        .tick_raw(self.array_chunk_size_metering(length + c));
                    self.meter.tick_builtin_some((length as u64) * 10);
                    let a = self.arrays.get_mut(&inst).unwrap();
                    let mut shifted = std::collections::BTreeMap::new();
                    for (&k, &v) in a.items.iter() {
                        shifted.insert(k + c, v);
                    }
                    for (i, mut v) in args.into_iter().enumerate() {
                        v.id = 0;
                        v.next = crate::value::SlotIndex::NULL;
                        shifted.insert(i as u32, v);
                        self.meter.tick_builtin_some(4);
                    }
                    a.items = shifted;
                    a.length = length + c;
                }
                self.meter.tick_builtin_some(2);
                Slot::integer((length + c) as i32)
            }
            // `Array.prototype.copyWithin(target[, start[, end]])` — dense fast
            // path. Copy the block `[start, end)` (clamped to fit) to `target`
            // in place. Metering: a frame constant + `mxMeterSome(count*10)`.
            NativeMethod::ArrayCopyWithin => {
                let inst = match self.dense_array_this(this) {
                    Some(i) => i,
                    None => return Err(Halt::Unsupported("copyWithin:non-dense-array")),
                };
                let length = self.arrays[&inst].length;
                let to = self.arg_to_index(base, 0, 0, length);
                let from = self.arg_to_index(base, 1, 0, length);
                let end = self.arg_to_index(base, 2, length, length);
                let mut count = end.saturating_sub(from);
                if count > length - to {
                    count = length - to;
                }
                self.meter.tick_raw(ARRAY_COPYWITHIN_FRAME_METERING);
                if count > 0 {
                    self.meter.tick_builtin_some((count as u64) * 10);
                    // Snapshot the source range, then write to the destination
                    // (memmove semantics — overlapping ranges are handled by the
                    // snapshot).
                    let src: Vec<Option<Slot>> = (0..count)
                        .map(|i| self.arrays[&inst].items.get(&(from + i)).copied())
                        .collect();
                    let a = self.arrays.get_mut(&inst).unwrap();
                    for (i, s) in src.into_iter().enumerate() {
                        let dst = to + i as u32;
                        match s {
                            Some(v) => {
                                a.items.insert(dst, v);
                            }
                            None => {
                                a.items.remove(&dst);
                            }
                        }
                    }
                }
                this
            }
            // `Array.prototype.with(index, value)` — a new array copying the
            // receiver with `index` replaced by `value`. Out-of-range index is
            // a RangeError (self-named). Metering: a frame constant + a
            // per-element copy cost over the generic `mxGetAt`/`mxDefineAt`
            // path, calibrated against the pin.
            NativeMethod::ArrayWith => {
                let inst = match self.dense_array_this(this) {
                    Some(i) => i,
                    None => return Err(Halt::Unsupported("with:non-dense-array")),
                };
                let length = self.arrays[&inst].length;
                let raw = match numeric_of(&arg0) {
                    Some(n) if !n.is_nan() => n.trunc() as i64,
                    _ => 0,
                };
                let index = if raw < 0 { length as i64 + raw } else { raw };
                if index < 0 || index >= length as i64 {
                    // RangeError — its abort-value/metering is a later increment.
                    return Err(Halt::Unsupported("with:range"));
                }
                let value = self.stack.get(base + 4 + 1).copied().unwrap_or_else(Slot::undefined);
                self.meter.tick_raw(ARRAY_WITH_FRAME_METERING);
                self.meter.tick_raw((length as u64) * ARRAY_WITH_PER_ELEM_METERING);
                let result = self.new_array_unmetered();
                if length > 0 {
                    self.meter.tick_raw(self.array_chunk_size_metering(length));
                    let items: Vec<Slot> = (0..length)
                        .map(|i| {
                            if i as i64 == index {
                                value
                            } else {
                                self.arrays[&inst]
                                    .items
                                    .get(&i)
                                    .copied()
                                    .unwrap_or_else(Slot::undefined)
                            }
                        })
                        .collect();
                    let a = self.arrays.get_mut(&result).unwrap();
                    for (i, s) in items.into_iter().enumerate() {
                        a.items.insert(i as u32, Slot::of(s.kind, s.value));
                    }
                    a.length = length;
                }
                Slot::of(Kind::Reference, Payload::Reference(result))
            }
            // `Array.prototype.forEach(callback[, thisArg])` — dense fast path.
            // Call `callback(item, index, array)` for each present element (via
            // the re-entrant [`Self::run_callback`]); returns `undefined`. The
            // callback body's own opcodes are metered by the nested dispatch;
            // this adds the per-element `fxCallThisItem` overhead
            // (`mxGetIndex` + the call frame setup) and the frame constant.
            NativeMethod::ArrayForEach => {
                let inst = match self.dense_array_this(this) {
                    Some(i) => i,
                    None => return Err(Halt::Unsupported("forEach:non-dense-array")),
                };
                let callback = arg0;
                let this_arg = self
                    .stack
                    .get(base + 4 + 1)
                    .copied()
                    .unwrap_or_else(Slot::undefined);
                let length = self.arrays[&inst].length;
                self.meter.tick_raw(ARRAY_FOREACH_FRAME_METERING);
                for i in 0..length {
                    let item = self.arrays[&inst].items.get(&i).copied();
                    if let Some(item) = item {
                        self.meter.tick_raw(ARRAY_FOREACH_PER_ELEM_METERING);
                        let cb_args = [item, Slot::integer(i as i32), this];
                        self.run_callback(code, callback, this_arg, &cb_args)?;
                    }
                }
                Slot::undefined()
            }
            // `Array.prototype.map` — a new array of the callback results.
            // Per element: the `fxCallThisItem` overhead + the callback body +
            // `mxMeterSome(2)` (the result store); plus the result chunk.
            NativeMethod::ArrayMap => {
                let inst = match self.dense_array_this(this) {
                    Some(i) => i,
                    None => return Err(Halt::Unsupported("map:non-dense-array")),
                };
                let callback = arg0;
                let this_arg = self.stack.get(base + 4 + 1).copied().unwrap_or_else(Slot::undefined);
                let length = self.arrays[&inst].length;
                self.meter.tick_raw(ARRAY_MAP_FRAME_METERING);
                let result = self.new_array_unmetered();
                if length > 0 {
                    self.meter.tick_raw(self.array_chunk_size_metering(length));
                }
                for i in 0..length {
                    let item = self.arrays[&inst].items.get(&i).copied();
                    if let Some(item) = item {
                        self.meter.tick_raw(ARRAY_FOREACH_PER_ELEM_METERING);
                        let cb_args = [item, Slot::integer(i as i32), this];
                        let r = self.run_callback(code, callback, this_arg, &cb_args)?;
                        self.meter.tick_builtin_some(2);
                        let mut v = r;
                        v.id = 0;
                        v.next = crate::value::SlotIndex::NULL;
                        self.arrays.get_mut(&result).unwrap().items.insert(i, v);
                    }
                }
                self.arrays.get_mut(&result).unwrap().length = length;
                Slot::of(Kind::Reference, Payload::Reference(result))
            }
            // `Array.prototype.some`/`every` — short-circuiting boolean folds.
            // Per element: the `fxCallThisItem` overhead + the callback body +
            // the `fxToBoolean` of its result.
            NativeMethod::ArraySome | NativeMethod::ArrayEvery => {
                let is_every = m == NativeMethod::ArrayEvery;
                let inst = match self.dense_array_this(this) {
                    Some(i) => i,
                    None => return Err(Halt::Unsupported("some/every:non-dense-array")),
                };
                let callback = arg0;
                let this_arg = self.stack.get(base + 4 + 1).copied().unwrap_or_else(Slot::undefined);
                let length = self.arrays[&inst].length;
                self.meter.tick_raw(ARRAY_SOMEEVERY_FRAME_METERING);
                let mut answer = is_every;
                for i in 0..length {
                    let item = self.arrays[&inst].items.get(&i).copied();
                    if let Some(item) = item {
                        self.meter.tick_raw(ARRAY_FOREACH_PER_ELEM_METERING);
                        let cb_args = [item, Slot::integer(i as i32), this];
                        let r = self.run_callback(code, callback, this_arg, &cb_args)?;
                        self.meter.tick_raw(ARRAY_PREDICATE_TOBOOL_METERING);
                        let truthy = self.truthy(&r);
                        if is_every && !truthy {
                            answer = false;
                            break;
                        }
                        if !is_every && truthy {
                            answer = true;
                            break;
                        }
                    }
                }
                Slot::boolean(answer)
            }
            // `Array.prototype.find`/`findIndex` — the first element/index whose
            // callback is truthy. `fxFindThisItem` calls the callback for EVERY
            // index (holes yield `undefined`), so the receiver need not be
            // dense; the per-element cost is the find overhead + callback body.
            NativeMethod::ArrayFind | NativeMethod::ArrayFindIndex => {
                let want_index = m == NativeMethod::ArrayFindIndex;
                let inst = match self.dense_array_this(this) {
                    Some(i) => i,
                    None => return Err(Halt::Unsupported("find:non-dense-array")),
                };
                let callback = arg0;
                let this_arg = self.stack.get(base + 4 + 1).copied().unwrap_or_else(Slot::undefined);
                let length = self.arrays[&inst].length;
                self.meter.tick_raw(ARRAY_FIND_FRAME_METERING);
                if !want_index {
                    // `find` (not `findIndex`) allocates a temporary for the
                    // element result (`mxTemporary(item)`): a fixed 2<<14 over
                    // `findIndex`, independent of the match.
                    self.meter.tick_raw(2 << 14);
                }
                let mut found: Option<(u32, Slot)> = None;
                for i in 0..length {
                    let item = self
                        .arrays[&inst]
                        .items
                        .get(&i)
                        .copied()
                        .unwrap_or_else(Slot::undefined);
                    self.meter.tick_raw(ARRAY_FIND_PER_ELEM_METERING);
                    let cb_args = [item, Slot::integer(i as i32), this];
                    let r = self.run_callback(code, callback, this_arg, &cb_args)?;
                    self.meter.tick_raw(ARRAY_PREDICATE_TOBOOL_METERING);
                    if self.truthy(&r) {
                        found = Some((i, item));
                        break;
                    }
                }
                match found {
                    Some((i, item)) => {
                        if want_index {
                            Slot::integer(i as i32)
                        } else {
                            item
                        }
                    }
                    None => {
                        if want_index {
                            Slot::integer(-1)
                        } else {
                            Slot::undefined()
                        }
                    }
                }
            }
            // `Array.prototype.filter` — a new array of the truthy-callback
            // elements. Per element: the `fxCallThisItem` overhead + the
            // callback body + `fxToBoolean`; a kept element appends (a slot +
            // `mxMeterSome`). The result chunk is sized to the kept count.
            NativeMethod::ArrayFilter => {
                let inst = match self.dense_array_this(this) {
                    Some(i) => i,
                    None => return Err(Halt::Unsupported("filter:non-dense-array")),
                };
                let callback = arg0;
                let this_arg = self.stack.get(base + 4 + 1).copied().unwrap_or_else(Slot::undefined);
                let length = self.arrays[&inst].length;
                self.meter.tick_raw(ARRAY_FILTER_FRAME_METERING);
                let mut kept: Vec<Slot> = Vec::new();
                for i in 0..length {
                    let item = self.arrays[&inst].items.get(&i).copied();
                    if let Some(item) = item {
                        self.meter.tick_raw(ARRAY_FOREACH_PER_ELEM_METERING);
                        let cb_args = [item, Slot::integer(i as i32), this];
                        let r = self.run_callback(code, callback, this_arg, &cb_args)?;
                        self.meter.tick_raw(ARRAY_PREDICATE_TOBOOL_METERING);
                        if self.truthy(&r) {
                            self.meter.tick_raw(ARRAY_FILTER_KEEP_METERING);
                            kept.push(item);
                        }
                    }
                }
                let result = self.new_array_unmetered();
                let total = kept.len() as u32;
                if total > 0 {
                    self.meter.tick_raw(self.array_chunk_size_metering(total));
                }
                {
                    let a = self.arrays.get_mut(&result).unwrap();
                    for (i, mut v) in kept.into_iter().enumerate() {
                        v.id = 0;
                        v.next = crate::value::SlotIndex::NULL;
                        a.items.insert(i as u32, v);
                    }
                    a.length = total;
                }
                Slot::of(Kind::Reference, Payload::Reference(result))
            }
            // `Array.prototype.reduce`/`reduceRight` — fold with
            // `callback(acc, item, index, array)` (`this` = undefined). With no
            // initial value the first (or last, for `reduceRight`) present
            // element seeds the accumulator; an empty array with no initial is
            // a TypeError (self-named). Per element: the `fxReduceThisItem`
            // 4-arg-callback overhead + the callback body.
            NativeMethod::ArrayReduce | NativeMethod::ArrayReduceRight => {
                let right = m == NativeMethod::ArrayReduceRight;
                let inst = match self.dense_array_this(this) {
                    Some(i) => i,
                    None => return Err(Halt::Unsupported("reduce:non-dense-array")),
                };
                let callback = arg0;
                let length = self.arrays[&inst].length;
                self.meter.tick_raw(ARRAY_REDUCE_FRAME_METERING);
                // The present indices in fold order.
                let order: Vec<u32> = if right {
                    (0..length).rev().filter(|i| self.arrays[&inst].items.contains_key(i)).collect()
                } else {
                    (0..length).filter(|i| self.arrays[&inst].items.contains_key(i)).collect()
                };
                let mut it = order.into_iter();
                let mut acc = if argc >= 2 {
                    self.stack.get(base + 4 + 1).copied().unwrap_or_else(Slot::undefined)
                } else {
                    match it.next() {
                        Some(i) => {
                            // The seed-finding scan (one iteration for a dense
                            // array — the first/last present element).
                            self.meter.tick_raw(ARRAY_REDUCE_INIT_SCAN_METERING);
                            self.arrays[&inst].items[&i]
                        }
                        None => return Err(Halt::Unsupported("reduce:empty-no-initial")),
                    }
                };
                for i in it {
                    let item = self.arrays[&inst].items[&i];
                    self.meter.tick_raw(ARRAY_REDUCE_PER_ELEM_METERING);
                    let cb_args = [acc, item, Slot::integer(i as i32), this];
                    acc = self.run_callback(code, callback, Slot::undefined(), &cb_args)?;
                }
                acc
            }
            // `Array.prototype.findLast`/`findLastIndex` — the last element/
            // index whose callback is truthy, scanning backward. Like
            // `find`/`findIndex` but reversed.
            NativeMethod::ArrayFindLast | NativeMethod::ArrayFindLastIndex => {
                let want_index = m == NativeMethod::ArrayFindLastIndex;
                let inst = match self.dense_array_this(this) {
                    Some(i) => i,
                    None => return Err(Halt::Unsupported("findLast:non-dense-array")),
                };
                let callback = arg0;
                let this_arg = self.stack.get(base + 4 + 1).copied().unwrap_or_else(Slot::undefined);
                let length = self.arrays[&inst].length;
                self.meter.tick_raw(ARRAY_FIND_FRAME_METERING);
                // The `findLast`/`findLastIndex` backward-scan setup, a fixed
                // cost over the forward `find`/`findIndex`.
                self.meter.tick_raw(ARRAY_FINDLAST_EXTRA_METERING);
                if !want_index {
                    self.meter.tick_raw(2 << 14);
                }
                let mut found: Option<(u32, Slot)> = None;
                for i in (0..length).rev() {
                    let item = self
                        .arrays[&inst]
                        .items
                        .get(&i)
                        .copied()
                        .unwrap_or_else(Slot::undefined);
                    self.meter.tick_raw(ARRAY_FIND_PER_ELEM_METERING);
                    let cb_args = [item, Slot::integer(i as i32), this];
                    let r = self.run_callback(code, callback, this_arg, &cb_args)?;
                    self.meter.tick_raw(ARRAY_PREDICATE_TOBOOL_METERING);
                    if self.truthy(&r) {
                        found = Some((i, item));
                        break;
                    }
                }
                match found {
                    Some((i, item)) => {
                        if want_index {
                            Slot::integer(i as i32)
                        } else {
                            item
                        }
                    }
                    None => {
                        if want_index {
                            Slot::integer(-1)
                        } else {
                            Slot::undefined()
                        }
                    }
                }
            }
            // `Array.prototype.toReversed()` — a new array with the elements
            // reversed (non-mutating), copied over the generic
            // `mxGetAt`/`mxDefineAt` path. Metering reuses `with`'s frame +
            // per-element constants (same copy loop) + the result chunk.
            NativeMethod::ArrayToReversed => {
                let inst = match self.dense_array_this(this) {
                    Some(i) => i,
                    None => return Err(Halt::Unsupported("toReversed:non-dense-array")),
                };
                let length = self.arrays[&inst].length;
                self.meter.tick_raw(ARRAY_TOREVERSED_FRAME_METERING);
                self.meter.tick_raw((length as u64) * ARRAY_WITH_PER_ELEM_METERING);
                let result = self.new_array_unmetered();
                if length > 0 {
                    self.meter.tick_raw(self.array_chunk_size_metering(length));
                    let items: Vec<Slot> = (0..length)
                        .map(|to| {
                            let from = length - 1 - to;
                            self.arrays[&inst]
                                .items
                                .get(&from)
                                .copied()
                                .unwrap_or_else(Slot::undefined)
                        })
                        .collect();
                    let a = self.arrays.get_mut(&result).unwrap();
                    for (to, s) in items.into_iter().enumerate() {
                        a.items.insert(to as u32, Slot::of(s.kind, s.value));
                    }
                    a.length = length;
                }
                Slot::of(Kind::Reference, Payload::Reference(result))
            }
            // `Array.prototype.splice(start[, deleteCount, ...items])` — dense
            // fast path. Remove `deleteCount` elements at `start` and insert
            // `items`, returning a new array of the removed elements. Metering
            // models the result chunk + `mxMeterSome(deletions*10 + 4)`, the
            // tail shift + array resize, `mxMeterSome(5)` per inserted item, and
            // a closing `mxMeterSome(4)`, plus a frame constant.
            NativeMethod::ArraySplice => {
                let inst = match self.dense_array_this(this) {
                    Some(i) => i,
                    None => return Err(Halt::Unsupported("splice:non-dense-array")),
                };
                let length = self.arrays[&inst].length;
                let start = self.arg_to_index(base, 0, 0, length);
                let (insertions, deletions): (u32, u32) = if argc == 0 {
                    (0, 0)
                } else if argc == 1 {
                    (0, length - start)
                } else {
                    let ins = (argc - 2) as u32;
                    // deleteCount clamped to [0, length - start].
                    let dc = match numeric_of(&self.stack.get(base + 4 + 1).copied().unwrap_or_else(Slot::undefined)) {
                        Some(n) if n.is_nan() || n < 0.0 => 0,
                        Some(n) if n > (length - start) as f64 => length - start,
                        Some(n) => n.trunc() as u32,
                        None => 0,
                    };
                    (ins, dc)
                };
                self.meter.tick_raw(ARRAY_SPLICE_FRAME_METERING);
                // The removed-elements result array.
                let result = self.new_array_unmetered();
                if deletions > 0 {
                    self.meter.tick_raw(self.array_chunk_size_metering(deletions));
                }
                self.meter.tick_builtin_some((deletions as u64) * 10);
                self.meter.tick_builtin_some(4);
                let tail_len = length - (start + deletions);
                if insertions < deletions {
                    self.meter.tick_builtin_some((tail_len as u64) * 10);
                    self.meter.tick_builtin_some(((deletions - insertions) as u64) * 4);
                    let new_len = length - (deletions - insertions);
                    if new_len > 0 {
                        self.meter.tick_raw(self.array_chunk_size_metering(new_len));
                    }
                } else if insertions > deletions {
                    let new_len = length + (insertions - deletions);
                    self.meter.tick_raw(self.array_chunk_size_metering(new_len));
                    self.meter.tick_builtin_some((tail_len as u64) * 10);
                }
                for _ in 0..insertions {
                    self.meter.tick_builtin_some(5);
                }
                self.meter.tick_builtin_some(4);
                // Perform the splice on a dense element vector.
                let cur: Vec<Slot> = (0..length)
                    .map(|i| self.arrays[&inst].items.get(&i).copied().unwrap_or_else(Slot::undefined))
                    .collect();
                let removed: Vec<Slot> = cur[start as usize..(start + deletions) as usize].to_vec();
                let inserted: Vec<Slot> = (0..insertions)
                    .map(|k| self.stack.get(base + 4 + 2 + k as usize).copied().unwrap_or_else(Slot::undefined))
                    .collect();
                let mut rebuilt: Vec<Slot> = Vec::new();
                rebuilt.extend_from_slice(&cur[..start as usize]);
                rebuilt.extend(inserted);
                rebuilt.extend_from_slice(&cur[(start + deletions) as usize..]);
                {
                    let a = self.arrays.get_mut(&inst).unwrap();
                    a.items.clear();
                    for (i, s) in rebuilt.into_iter().enumerate() {
                        a.items.insert(i as u32, Slot::of(s.kind, s.value));
                    }
                    a.length = length - deletions + insertions;
                }
                {
                    let a = self.arrays.get_mut(&result).unwrap();
                    for (i, s) in removed.into_iter().enumerate() {
                        a.items.insert(i as u32, Slot::of(s.kind, s.value));
                    }
                    a.length = deletions;
                }
                Slot::of(Kind::Reference, Payload::Reference(result))
            }
            // `Array.prototype.toSpliced(start, deleteCount, ...items)` — a
            // non-mutating splice: build a NEW array `head ++ inserted ++ tail`
            // and leave the receiver untouched. XS meters the head copy at
            // `start * 10`, each insertion at `5`, the tail copy at `rest * 10`,
            // plus a trailing `mxMeterSome(4)` and the result item chunk.
            NativeMethod::ArrayToSpliced => {
                let inst = match self.dense_array_this(this) {
                    Some(i) => i,
                    None => return Err(Halt::Unsupported("toSpliced:non-dense-array")),
                };
                let length = self.arrays[&inst].length;
                let start = self.arg_to_index(base, 0, 0, length);
                let (insertions, skip): (u32, u32) = if argc == 0 {
                    (0, 0)
                } else if argc == 1 {
                    (0, length - start)
                } else {
                    let ins = (argc - 2) as u32;
                    let dc = match numeric_of(&self.stack.get(base + 4 + 1).copied().unwrap_or_else(Slot::undefined)) {
                        Some(n) if n.is_nan() || n < 0.0 => 0,
                        Some(n) if n > (length - start) as f64 => length - start,
                        Some(n) => n.trunc() as u32,
                        None => 0,
                    };
                    (ins, dc)
                };
                let result_len = length + insertions - skip;
                let rest = length - (start + skip);
                self.meter.tick_raw(ARRAY_TOSPLICED_FRAME_METERING);
                if result_len > 0 {
                    self.meter.tick_raw(self.array_chunk_size_metering(result_len));
                }
                self.meter.tick_builtin_some((start as u64) * 10);
                for _ in 0..insertions {
                    self.meter.tick_builtin_some(5);
                }
                self.meter.tick_builtin_some((rest as u64) * 10);
                self.meter.tick_builtin_some(4);
                // Build the result densely; the receiver stays untouched.
                let cur: Vec<Slot> = (0..length)
                    .map(|i| self.arrays[&inst].items.get(&i).copied().unwrap_or_else(Slot::undefined))
                    .collect();
                let inserted: Vec<Slot> = (0..insertions)
                    .map(|k| self.stack.get(base + 4 + 2 + k as usize).copied().unwrap_or_else(Slot::undefined))
                    .collect();
                let mut rebuilt: Vec<Slot> = Vec::new();
                rebuilt.extend_from_slice(&cur[..start as usize]);
                rebuilt.extend(inserted);
                rebuilt.extend_from_slice(&cur[(start + skip) as usize..]);
                let result = self.new_array_unmetered();
                {
                    let a = self.arrays.get_mut(&result).unwrap();
                    for (i, s) in rebuilt.into_iter().enumerate() {
                        a.items.insert(i as u32, Slot::of(s.kind, s.value));
                    }
                    a.length = result_len;
                }
                Slot::of(Kind::Reference, Payload::Reference(result))
            }
            // `Array.prototype.flat([depth])` — a new array with sub-array
            // elements flattened to `depth` (default 1). XS's `flatAux` visits
            // each source index, recursing into array elements (up to `depth`)
            // and appending leaves via `mxDefineIndex` (which grows the result
            // item chunk one slot at a time). Metering models the per-visit
            // read, the per-array-element length read, and the per-appended
            // element chunk growth, plus a frame constant.
            NativeMethod::ArrayFlat => {
                let inst = match self.dense_array_this(this) {
                    Some(i) => i,
                    None => return Err(Halt::Unsupported("flat:non-dense-array")),
                };
                let depth = if argc >= 1 {
                    match numeric_of(&arg0) {
                        Some(n) if n.is_nan() || n < 0.0 => 0,
                        Some(n) => n.trunc() as u32,
                        None => 0,
                    }
                } else {
                    1
                };
                let length = self.arrays[&inst].length;
                self.meter.tick_raw(ARRAY_FLAT_FRAME_METERING);
                let mut out: Vec<Slot> = Vec::new();
                self.flat_into(inst, length, depth, &mut out);
                let result = self.new_array_unmetered();
                let total = out.len() as u32;
                {
                    let a = self.arrays.get_mut(&result).unwrap();
                    for (i, s) in out.into_iter().enumerate() {
                        a.items.insert(i as u32, Slot::of(s.kind, s.value));
                    }
                    a.length = total;
                }
                Slot::of(Kind::Reference, Payload::Reference(result))
            }
            // `Array.prototype.flatMap(callback[, thisArg])` — call
            // `callback(item, index, array)` per element, then flatten the
            // results by one level. Re-entrant (uses `run_callback`); the
            // result flattening reuses `flat`'s per-leaf/per-array constants,
            // plus a per-source callback overhead.
            NativeMethod::ArrayFlatMap => {
                let inst = match self.dense_array_this(this) {
                    Some(i) => i,
                    None => return Err(Halt::Unsupported("flatMap:non-dense-array")),
                };
                let callback = arg0;
                let this_arg = self.stack.get(base + 4 + 1).copied().unwrap_or_else(Slot::undefined);
                let length = self.arrays[&inst].length;
                self.meter.tick_raw(ARRAY_FLAT_FRAME_METERING);
                let mut out: Vec<Slot> = Vec::new();
                for i in 0..length {
                    let item = self.arrays[&inst].items.get(&i).copied();
                    if let Some(item) = item {
                        self.meter.tick_raw(ARRAY_FLATMAP_CALLBACK_METERING);
                        let cb_args = [item, Slot::integer(i as i32), this];
                        let r = self.run_callback(code, callback, this_arg, &cb_args)?;
                        // Flatten the result by one level.
                        let is_array = matches!(r.value, Payload::Reference(x) if self.arrays.contains_key(&x));
                        if is_array {
                            let sub = match r.value {
                                Payload::Reference(x) => x,
                                _ => unreachable!(),
                            };
                            self.meter.tick_raw(ARRAY_FLAT_PER_ARRAY_METERING);
                            let sub_len = self.arrays[&sub].length;
                            for k in 0..sub_len {
                                if let Some(e) = self.arrays[&sub].items.get(&k).copied() {
                                    self.meter.tick_raw(ARRAY_FLAT_PER_LEAF_METERING);
                                    self.meter.tick_raw(self.array_item_grow_metering(out.len() as u64));
                                    out.push(e);
                                }
                            }
                        } else {
                            self.meter.tick_raw(ARRAY_FLAT_PER_LEAF_METERING);
                            self.meter.tick_raw(self.array_item_grow_metering(out.len() as u64));
                            out.push(r);
                        }
                    }
                }
                let result = self.new_array_unmetered();
                let total = out.len() as u32;
                {
                    let a = self.arrays.get_mut(&result).unwrap();
                    for (i, s) in out.into_iter().enumerate() {
                        a.items.insert(i as u32, Slot::of(s.kind, s.value));
                    }
                    a.length = total;
                }
                Slot::of(Kind::Reference, Payload::Reference(result))
            }
            // `Array.prototype.join([sep])` — dense fast path. Each element is
            // ToString'd into a key slot, the pieces joined by `sep` (default
            // ","), and the result materialized into one final chunk. Metering
            // models `fxNewInstance` (the key list) + a key slot per element
            // and per separator + each element's `fxToString` (a number renders
            // to a fresh chunk + a built-in step) + the final `fxNewChunk`.
            NativeMethod::ArrayJoin => {
                let inst = match self.dense_array_this(this) {
                    Some(i) => i,
                    None => return Err(Halt::Unsupported("join:non-dense-array")),
                };
                let sep: Vec<u8> = if argc == 0 || arg0.kind == Kind::Undefined {
                    b",".to_vec()
                } else if arg0.kind == Kind::String {
                    match arg0.value {
                        Payload::String(off) => self.str_content(off).to_vec(),
                        _ => b",".to_vec(),
                    }
                } else {
                    return Err(Halt::Unsupported("join:non-string-separator"));
                };
                let length = self.arrays[&inst].length;
                let items: Vec<Option<Slot>> = {
                    let a = &self.arrays[&inst];
                    (0..length).map(|i| a.items.get(&i).copied()).collect()
                };
                self.meter.tick_raw(ARRAY_JOIN_FRAME_METERING);
                self.meter.tick_slot_alloc(); // `fxNewInstance` (the key list)
                let mut out: Vec<u8> = Vec::new();
                for (i, item) in items.into_iter().enumerate() {
                    // Every index is read (`mxGetIndex`) regardless of type.
                    self.meter.tick_raw(ARRAY_JOIN_PER_ELEMENT_METERING);
                    if i > 0 {
                        self.meter.tick_slot_alloc(); // the separator key slot
                        out.extend_from_slice(&sep);
                    }
                    match item {
                        Some(s) if s.kind != Kind::Undefined && s.kind != Kind::Null => {
                            if s.kind == Kind::Reference {
                                return Err(Halt::Unsupported("join:reference-element"));
                            }
                            self.meter.tick_slot_alloc(); // the element key slot
                            let bytes = self.to_string_bytes_metered(s);
                            out.extend_from_slice(&bytes);
                        }
                        _ => {}
                    }
                }
                self.meter.tick_chunk_new((out.len() + 1) as u64);
                let off = self.chunks.alloc(&out);
                Slot::of(Kind::String, Payload::String(off))
            }
            // `Array.prototype.toString()` delegates to `this.join()` with the
            // default separator: it meters a small prelude (the `join` lookup +
            // the `mxRunCount(0)` call-frame setup) and then the identical join
            // body (frame + per-element read + the result chunk). Modeled by
            // running the default-separator join and adding the prelude.
            NativeMethod::ArrayToString => {
                let inst = match self.dense_array_this(this) {
                    Some(i) => i,
                    None => return Err(Halt::Unsupported("toString:non-dense-array")),
                };
                self.meter.tick_raw(ARRAY_TOSTRING_PRELUDE_METERING);
                let length = self.arrays[&inst].length;
                let items: Vec<Option<Slot>> = {
                    let a = &self.arrays[&inst];
                    (0..length).map(|i| a.items.get(&i).copied()).collect()
                };
                self.meter.tick_raw(ARRAY_JOIN_FRAME_METERING);
                self.meter.tick_slot_alloc();
                let mut out: Vec<u8> = Vec::new();
                for (i, item) in items.into_iter().enumerate() {
                    self.meter.tick_raw(ARRAY_JOIN_PER_ELEMENT_METERING);
                    if i > 0 {
                        self.meter.tick_slot_alloc();
                        out.push(b',');
                    }
                    match item {
                        Some(s) if s.kind != Kind::Undefined && s.kind != Kind::Null => {
                            if s.kind == Kind::Reference {
                                return Err(Halt::Unsupported("toString:reference-element"));
                            }
                            self.meter.tick_slot_alloc();
                            let bytes = self.to_string_bytes_metered(s);
                            out.extend_from_slice(&bytes);
                        }
                        _ => {}
                    }
                }
                self.meter.tick_chunk_new((out.len() + 1) as u64);
                let off = self.chunks.alloc(&out);
                Slot::of(Kind::String, Payload::String(off))
            }
            // Recognized-but-unimplemented Array methods and statics: honest
            // NAMED skips, so a reference is `Halt::Unsupported` (never a
            // completion divergence or a wrong value). See each variant's doc.
            NativeMethod::ArraySort => {
                let _ = (base, argc);
                return Err(Halt::Unsupported("Array.prototype.sort:data-dependent-comparison-metering"));
            }
            NativeMethod::ArrayToSorted => {
                let _ = (base, argc);
                return Err(Halt::Unsupported("Array.prototype.toSorted:data-dependent-comparison-metering"));
            }
            NativeMethod::ArrayToLocaleString => {
                let _ = (base, argc);
                return Err(Halt::Unsupported("Array.prototype.toLocaleString:locale-stringification"));
            }
            NativeMethod::ArrayFrom => {
                let _ = (base, argc);
                return Err(Halt::Unsupported("Array.from:iterator-protocol-metering"));
            }
            NativeMethod::ArrayFromAsync => {
                let _ = (base, argc);
                return Err(Halt::Unsupported("Array.fromAsync:async-iteration"));
            }
            // `Array.isArray(v)`: whether `v` is an array exotic object.
            NativeMethod::ArrayIsArray => {
                self.meter.tick_raw(ARRAY_ISARRAY_METERING);
                let r = match arg0.value {
                    Payload::Reference(r) => self.arrays.contains_key(&r),
                    _ => false,
                };
                Slot::boolean(r)
            }
            // `Array.of(...items)` (`fx_Array_of`): its per-element metering
            // (a first-element chunk-transition outlier plus a residual over
            // `mxMeterSome(4)`) does not calibrate to a clean per-element
            // constant, so this self-names an honest skip rather than shipping
            // a divergent meter (a "within reach" stretch static).
            NativeMethod::ArrayOf => {
                let _ = base;
                return Err(Halt::Unsupported("Array.of:metering"));
            }
            // `Array.prototype.values()`/`keys()`/`entries()`: build an Array
            // Iterator over the receiver.
            NativeMethod::ArrayValues | NativeMethod::ArrayKeys | NativeMethod::ArrayEntries => {
                let arr = match this.value {
                    Payload::Reference(i) if self.arrays.contains_key(&i) => i,
                    _ => return Err(Halt::Unsupported("array-iterator:non-array")),
                };
                let kind = match m {
                    NativeMethod::ArrayValues => 0u8,
                    NativeMethod::ArrayKeys => 1u8,
                    _ => 2u8,
                };
                self.make_array_iterator(arr, kind)
            }
            // `%ArrayIteratorPrototype%.next()`.
            NativeMethod::ArrayIteratorNext => {
                let iter = match this.value {
                    Payload::Reference(i) if self.iterators.contains_key(&i) => i,
                    _ => return Err(Halt::Unsupported("array-iterator-next:non-iterator")),
                };
                self.array_iterator_next(iter)?
            }
        };
        self.stack.truncate(base);
        self.push(result);
        Ok(())
    }

    /// Build an Array Iterator over `arr` with the given `kind` (0 values, 1
    /// keys, 2 entries): `fxNewIteratorInstance` — allocate the iterator
    /// instance (chained to `%Array Iterator.prototype%`) and its reused
    /// `{value, done}` result object, and record the [`IterState`]. Meters the
    /// creation cluster ([`ARRAY_ITERATOR_CREATE_METERING`]).
    fn make_array_iterator(&mut self, arr: crate::value::SlotIndex, kind: u8) -> Slot {
        self.meter.tick_raw(ARRAY_ITERATOR_CREATE_METERING);
        // The reused result object `{ value: undefined, done: false }`.
        let result = self.slots.alloc(Slot::instance(self.object_proto));
        if let Some(vid) = self.value_id {
            self.set_own_unmetered(result, vid, Slot::undefined());
        }
        if let Some(did) = self.done_id {
            self.set_own_unmetered(result, did, Slot::boolean(false));
        }
        let iter = self.slots.alloc(Slot::instance(self.array_iterator_proto));
        self.iterators.insert(
            iter,
            IterState {
                iterable: arr,
                index: 0,
                kind,
                result,
                done: false,
                enum_keys: Vec::new(),
            },
        );
        Slot::of(Kind::Reference, Payload::Reference(iter))
    }

    /// Build a for-in enumerator over `obj` (XS's `fx_Enumerator`): collect the
    /// object's enumerable own-then-inherited string keys in XS enumeration
    /// order (integer indices ascending, then string keys in insertion order,
    /// per prototype level, skipping shadowed keys), and record them as an
    /// enumerator [`IterState`] (kind 3) whose `next()` yields each as a
    /// string. Meters the creation cluster ([`FOR_IN_ENUMERATOR_METERING`]);
    /// each yielded key's string allocation is metered in `next()`.
    fn make_enumerator(&mut self, obj: crate::value::SlotIndex) -> Slot {
        self.meter.tick_raw(FOR_IN_ENUMERATOR_METERING);
        if self.arrays.contains_key(&obj) {
            self.meter.tick_raw(ARRAY_FOR_IN_EXTRA_METERING);
        }
        let keys = self.enumerable_keys(obj);
        let result = self.slots.alloc(Slot::instance(self.object_proto));
        if let Some(vid) = self.value_id {
            self.set_own_unmetered(result, vid, Slot::undefined());
        }
        if let Some(did) = self.done_id {
            self.set_own_unmetered(result, did, Slot::boolean(false));
        }
        let iter = self.slots.alloc(Slot::instance(self.array_iterator_proto));
        self.iterators.insert(
            iter,
            IterState {
                iterable: obj,
                index: 0,
                kind: 3,
                result,
                done: false,
                enum_keys: keys,
            },
        );
        Slot::of(Kind::Reference, Payload::Reference(iter))
    }

    /// The enumerable own-then-inherited string keys of `obj` in XS for-in
    /// order, as `(id, index)` pairs (`id == XS_NO_ID` ⇒ an array index). For
    /// an array: the present item indices ascending. For an ordinary object:
    /// its own string-named properties in insertion order. The prototype chain
    /// is walked (skipping already-seen keys), but the covered grammar's
    /// prototypes (`%Object.prototype%` / `%Array.prototype%`) carry no
    /// enumerable data properties, so only own keys appear.
    fn enumerable_keys(&self, obj: crate::value::SlotIndex) -> Vec<(u16, u32)> {
        let mut out: Vec<(u16, u32)> = Vec::new();
        let mut seen: std::collections::HashSet<(u16, u32)> = std::collections::HashSet::new();
        let mut cur = obj;
        while !cur.is_null() {
            // Array index keys first (ascending), then string keys.
            if let Some(a) = self.arrays.get(&cur) {
                let mut idxs: Vec<u32> = a.items.keys().copied().collect();
                idxs.sort_unstable();
                for i in idxs {
                    let k = (crate::value::XS_NO_ID, i);
                    if seen.insert(k) {
                        out.push(k);
                    }
                }
            }
            // Own string-named properties, in insertion order. The property
            // list is prepend-ordered (newest first), so collect and reverse.
            let mut names: Vec<(u16, u32)> = Vec::new();
            let mut p = self.slots.get(cur).next;
            while !p.is_null() {
                let s = self.slots.get(p);
                if s.id != crate::value::XS_NO_ID {
                    names.push((s.id, 0));
                }
                p = s.next;
            }
            names.reverse();
            for k in names {
                if seen.insert(k) {
                    out.push(k);
                }
            }
            cur = self.instance_prototype(cur);
        }
        out
    }

    /// `fx_ArrayIterator_prototype_next`: advance the iterator, mutate its
    /// reused result object's `value`/`done`, and return that object. Meters
    /// [`ARRAY_ITERATOR_NEXT_METERING`]; an `entries` element allocates a fresh
    /// `[index, value]` pair (its own array-create metering).
    fn array_iterator_next(&mut self, iter: crate::value::SlotIndex) -> Result<Slot, Halt> {
        if self.iterators[&iter].kind == 3 {
            return Ok(self.enumerator_next(iter));
        }
        let st = self.iterators[&iter].clone();
        let result = st.result;
        let (new_value, new_done, next_index): (Slot, bool, u32) = if st.done {
            // An already-exhausted iterator: `next()` does the minimal work
            // (no yield), metering only its dispatch.
            (Slot::undefined(), true, st.index)
        } else {
            let length = self.arrays.get(&st.iterable).map(|a| a.length).unwrap_or(0);
            if st.index < length {
                // A yielding `next()`: the base result-object mutation cost,
                // plus (for `values`/`entries`) the array-element read
                // (`mxGetIndex`) `keys` does not do.
                self.meter.tick_raw(ARRAY_ITERATOR_NEXT_METERING);
                if st.kind == 0 || st.kind == 2 {
                    self.meter.tick_raw(ARRAY_ITERATOR_ELEMENT_READ);
                }
                let v = match st.kind {
                    0 => self
                        .arrays
                        .get(&st.iterable)
                        .and_then(|a| a.items.get(&st.index).copied())
                        .map(|s| Slot::of(s.kind, s.value))
                        .unwrap_or_else(Slot::undefined),
                    1 => Slot::integer(st.index as i32),
                    _ => {
                        // entries: a fresh `[index, arr[index]]` pair array.
                        let elem = self
                            .arrays
                            .get(&st.iterable)
                            .and_then(|a| a.items.get(&st.index).copied())
                            .map(|s| Slot::of(s.kind, s.value))
                            .unwrap_or_else(Slot::undefined);
                        let pair = self.new_array();
                        let a = self.arrays.get_mut(&pair).unwrap();
                        a.length = 2;
                        a.items.insert(0, Slot::integer(st.index as i32));
                        a.items.insert(1, elem);
                        self.meter.tick_raw(self.array_chunk_size_metering(2));
                        Slot::of(Kind::Reference, Payload::Reference(pair))
                    }
                };
                (v, false, st.index + 1)
            } else {
                (Slot::undefined(), true, st.index)
            }
        };
        // Update the iterator state and the reused result object.
        if let Some(s) = self.iterators.get_mut(&iter) {
            s.index = next_index;
            s.done = new_done;
        }
        if let Some(vid) = self.value_id {
            self.set_own_unmetered(result, vid, Slot::of(new_value.kind, new_value.value));
        }
        if let Some(did) = self.done_id {
            self.set_own_unmetered(result, did, Slot::boolean(new_done));
        }
        Ok(Slot::of(Kind::Reference, Payload::Reference(result)))
    }

    /// `fx_Enumerator_prototype_next` for a for-in enumerator: yield the next
    /// enumerable key as a string, mutating and returning the reused result
    /// object. Meters the per-`next()` base plus the yielded key's string
    /// allocation.
    fn enumerator_next(&mut self, iter: crate::value::SlotIndex) -> Slot {
        let st = self.iterators[&iter].clone();
        let result = st.result;
        let (new_value, new_done, next_index): (Slot, bool, u32) =
            if st.done || (st.index as usize) >= st.enum_keys.len() {
                (Slot::undefined(), true, st.index)
            } else {
                self.meter.tick_raw(ENUMERATOR_NEXT_METERING);
                let (id, idx) = st.enum_keys[st.index as usize];
                // The key string: an array index renders as a fresh decimal
                // (`fxKeyAt` allocates it, metered per byte + NUL); a named key
                // reuses its interned symbol name (no run-time allocation in
                // XS, so endor allocates the chunk it needs to produce the
                // value but does NOT meter it).
                let bytes: Vec<u8> = if id == crate::value::XS_NO_ID {
                    let b = number_to_ecma_string(idx as f64).into_bytes();
                    self.meter.tick_chunk_new((b.len() + 1) as u64);
                    b
                } else {
                    self.symbol_names
                        .get(id as usize - 1)
                        .cloned()
                        .unwrap_or_default()
                        .into_bytes()
                };
                let off = self.chunks.alloc(&bytes);
                (
                    Slot::of(Kind::String, Payload::String(off)),
                    false,
                    st.index + 1,
                )
            };
        if let Some(s) = self.iterators.get_mut(&iter) {
            s.index = next_index;
            s.done = new_done;
        }
        if let Some(vid) = self.value_id {
            self.set_own_unmetered(result, vid, Slot::of(new_value.kind, new_value.value));
        }
        if let Some(did) = self.done_id {
            self.set_own_unmetered(result, did, Slot::boolean(new_done));
        }
        Slot::of(Kind::Reference, Payload::Reference(result))
    }

    /// The array instance behind `this` **iff** it is a dense array (every
    /// index in `[0, length)` present, no holes) — the receiver shape XS's
    /// `fxCheckArray` fast path accepts. A sparse array (holes) or a
    /// non-array returns `None`, and the caller self-names an honest skip
    /// (XS's generic slow path is a later increment).
    fn dense_array_this(&self, this: Slot) -> Option<crate::value::SlotIndex> {
        let inst = match this.value {
            Payload::Reference(i) => i,
            _ => return None,
        };
        let a = self.arrays.get(&inst)?;
        if a.items.len() as u32 == a.length {
            Some(inst)
        } else {
            None
        }
    }

    /// The raw 16.16 metering of an array item-chunk (re)size to `slots`
    /// item slots (XS's `fxSetIndexSize`/`fxNewChunk`/`fxRenewChunk` of a
    /// `slots * sizeof(txSlot)` chunk): the adjusted chunk size
    /// `round_up_8(slots*32) + sizeof(txChunk)` = `slots*32 + 16` (payload
    /// already 8-aligned). Zero slots allocate nothing.
    fn array_chunk_size_metering(&self, slots: u32) -> u64 {
        if slots == 0 {
            0
        } else {
            (slots as u64) * 32 + 16
        }
    }

    /// Strict equality (`===`) with chunk-aware string comparison: two heap
    /// strings are equal iff their CESU-8 content matches (the free
    /// [`strict_equals`] compares only primitive/reference kinds and treats
    /// two strings as unequal because it cannot see the chunk arena).
    fn strict_equal(&self, a: &Slot, b: &Slot) -> bool {
        match (a.value, b.value) {
            (Payload::String(x), Payload::String(y)) => {
                self.str_content(x) == self.str_content(y)
            }
            _ => strict_equals(a, b),
        }
    }

    /// SameValueZero (`includes`): strict equality except `NaN` equals `NaN`
    /// (and `+0`/`-0` are equal, which strict equality already gives).
    fn same_value_zero(&self, a: &Slot, b: &Slot) -> bool {
        if let (Some(x), Some(y)) = (numeric_of(a), numeric_of(b)) {
            if x.is_nan() && y.is_nan() {
                return true;
            }
        }
        self.strict_equal(a, b)
    }

    /// `fxArgToIndex`: the argument at `base`+`argi` coerced to a relative
    /// index in `[0, length]` (negative counts from the end, clamped). Absent
    /// or `undefined` uses `default`. The covered grammar passes small
    /// non-negative integers.
    fn arg_to_index(&self, base: usize, argi: usize, default: u32, length: u32) -> u32 {
        let a = self.stack.get(base + 4 + argi).copied();
        let n = match a {
            None => return default,
            Some(s) if s.kind == Kind::Undefined => return default,
            Some(s) => match numeric_of(&s) {
                Some(n) => n,
                None => return default,
            },
        };
        if n.is_nan() {
            return 0;
        }
        let t = n.trunc();
        if t < 0.0 {
            let from_end = length as f64 + t;
            if from_end < 0.0 {
                0
            } else {
                from_end as u32
            }
        } else if t > length as f64 {
            length
        } else {
            t as u32
        }
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
        // The suspended caller is resumed: release its accounted frame
        // slots (the inverse of the `enter_call` accrual).
        self.frame_slots = self
            .frame_slots
            .saturating_sub(FRAME_OVERHEAD_SLOTS + caller.args.len() + caller.locals.len());
        self.locals = caller.locals;
        self.id_map = caller.id_map;
        self.result = caller.result;
        self.strict = caller.strict;
        self.args = caller.args;
        self.this_val = caller.this_val;
        self.cur_func = caller.cur_func;
        self.cur_target = caller.cur_target;
        caller.ret_pc
    }

    /// `fxJump`: unwind to the innermost jump-buffer entry (XS's
    /// `the->firstJump`), restoring exactly what the `c_setjmp` restore in
    /// `CATCH` restores — the call frames back to the establishing frame,
    /// then that frame's value-stack and scope cuts — and returning the
    /// target pc to resume at. Returns `None` when the chain is empty (the
    /// throw escapes every JS handler and reaches the host boundary), so
    /// the caller yields `Halt::Throw`.
    fn unwind_to_jump(&mut self) -> Option<usize> {
        let jump = self.jumps.pop()?;
        // Pop any callee activations opened since the catch was
        // established (a throw crossing called functions), restoring the
        // establishing frame's saved activation each time (XS restores
        // `mxFrame`). Discard the callee results — the throw abandons them.
        while self.call_stack.len() > jump.call_depth {
            let _ = self.leave_call();
        }
        // Restore the establishing frame's value-stack and scope cuts
        // (XS's `mxStack = jump->stack; mxScope = jump->scope`) and the
        // exact environment name map at catch time.
        self.stack.truncate(jump.stack_len);
        self.locals.truncate(jump.locals_len);
        self.id_map = jump.id_map;
        let _ = jump.flag; // every endor jump is a JS jump (flag == 1)
        Some(jump.target_pc)
    }

    /// Adjust the meter for an uncaught throw escaping to the host: the
    /// escaping opcode's dispatch metering (added at the top of the loop)
    /// is removed — C-XS never meters it, its `mxBreak` bypassed by the
    /// longjmp — and the fixed host-boundary constant
    /// [`THROW_HOST_ESCAPE_METERING`] is accrued instead.
    #[inline]
    fn meter_host_escape(&mut self) {
        self.meter.untick_code();
        self.meter.tick_raw(THROW_HOST_ESCAPE_METERING);
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

    /// Point closure scope slot `k` at a different heap cell (XS's
    /// `slot->value.closure = variable`), preserving the slot's `Closure`
    /// kind and binding id. Used by `reset_closure`/`refresh_closure` to
    /// give a per-iteration `let` binding a fresh cell.
    fn repoint_closure(&mut self, k: usize, cell: crate::value::SlotIndex) {
        if let Some(i) = self.local_index(k) {
            self.locals[i].kind = Kind::Closure;
            self.locals[i].value = Payload::Reference(cell);
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
    /// A top-level script program's `this` binding: the realm global
    /// object (`fxRunProgram` binds the program frame's `this` to the
    /// realm global for a script; only an ES module binds `undefined`, and
    /// modules are structurally skipped). Set once at program entry so a
    /// top-level `this` opcode observes the global rather than the default
    /// `undefined`.
    fn bind_program_this(&mut self) {
        self.this_val = Slot::of(Kind::Reference, Payload::Reference(self.global_obj));
    }

    /// `fxRunConstructor` (driven by `begin` in a construct frame): allocate
    /// the fresh `this` instance the constructor populates, and bind the
    /// frame's `this` to it. XS reads the prototype from the constructor's
    /// `.prototype` (defaulting to `%Object.prototype%`); the covered grammar
    /// reads only own properties of `this`, so endor allocates the instance
    /// with a null prototype and leaves the intrinsic-prototype wiring to the
    /// object-model stage. Meters the single instance `fxNewSlot`
    /// ([`crate::meter::SLOT_ALLOCATION_METERING`], 256 raw) exactly where
    /// `fxNewHostInstance` allocates it — measured against the pin as the
    /// whole construct overhead over a plain call.
    fn run_constructor(&mut self) {
        // `fxRunConstructor` runs `fxBeginHost`/`fxEndHost` around
        // `fxGetPrototypeFromConstructor` and then `fxNewHostInstance`. Beyond
        // the instance `fxNewSlot` ([`crate::meter::SLOT_ALLOCATION_METERING`],
        // 256 raw), the host-frame entry/exit accrues a fixed two code units
        // ([`CONSTRUCTOR_HOST_FRAME_METERING`]) — measured against the pin as
        // exactly the gap between `new f()` and a plain `f()` (131072 raw =
        // 2 × `XS_CODE_METERING`), independent of the constructor's body.
        self.meter.tick_slot_alloc();
        self.meter.tick_raw(CONSTRUCTOR_HOST_FRAME_METERING);
        // The new `this` chains to the constructor's `.prototype`
        // (fxGetPrototypeFromConstructor), defaulting to %Object.prototype% —
        // so `(new F()) instanceof F` holds. Reading the prototype is a
        // property get (unmetered), already folded into the measured cost.
        let proto = self.prototype_of(self.cur_func).unwrap_or(self.object_proto);
        let inst = self.slots.alloc(Slot::instance(proto));
        self.this_val = Slot::of(Kind::Reference, Payload::Reference(inst));
    }

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
    /// Allocate a fresh empty exotic array (`fxNewArray(the, 0)` →
    /// `fxNewArrayInstance`): a real arena instance chained to
    /// `%Array.prototype%`, registered in [`Self::arrays`] with length 0 and
    /// no items. Meters [`ARRAY_CREATE_METERING`] (the instance + internal
    /// array-behavior slot allocations).
    fn new_array(&mut self) -> crate::value::SlotIndex {
        self.meter.tick_raw(ARRAY_CREATE_METERING);
        let inst = self.slots.alloc(Slot::instance(self.array_proto));
        self.arrays.insert(inst, ArrayData::default());
        inst
    }

    /// XS's `flatAux`: visit each index of `src` (length `len`), recursing into
    /// array elements while `depth > 0` and appending leaves to `out`. Meters
    /// the per-visit read, the per-array-element length read, and each
    /// appended leaf's `mxDefineIndex` chunk growth as it goes.
    fn flat_into(&mut self, src: crate::value::SlotIndex, len: u32, depth: u32, out: &mut Vec<Slot>) {
        for index in 0..len {
            let item = match self.arrays.get(&src).and_then(|a| a.items.get(&index).copied()) {
                Some(it) => it,
                None => continue, // a hole is skipped (fxHasIndex false)
            };
            let is_array = matches!(item.value, Payload::Reference(r) if self.arrays.contains_key(&r));
            if depth > 0 && is_array {
                let sub = match item.value {
                    Payload::Reference(r) => r,
                    _ => unreachable!(),
                };
                self.meter.tick_raw(ARRAY_FLAT_PER_ARRAY_METERING);
                let sub_len = self.arrays[&sub].length;
                self.flat_into(sub, sub_len, depth - 1, out);
            } else {
                // Append the leaf: the per-leaf cost plus the `mxDefineIndex`
                // chunk growth to `out.len() + 1` slots.
                self.meter.tick_raw(ARRAY_FLAT_PER_LEAF_METERING);
                self.meter
                    .tick_raw(self.array_item_grow_metering(out.len() as u64));
                out.push(item);
            }
        }
    }

    /// Allocate an empty array instance **without** charging the standalone
    /// `ARRAY_CREATE_METERING` — for callers (e.g. `slice`) whose own frame
    /// constant already folds in the result-array construction cost.
    fn new_array_unmetered(&mut self) -> crate::value::SlotIndex {
        let inst = self.slots.alloc(Slot::instance(self.array_proto));
        self.arrays.insert(inst, ArrayData::default());
        inst
    }

    /// Convert a stack value into an `XS_AT_KIND` computed key (XS's
    /// `XS_CODE_AT_ALL`): an integer/number that is a valid array index
    /// becomes an index key (`id == XS_NO_ID`); a symbol or a program-known
    /// string name becomes a named key. Returns `None` for a key endor does
    /// not yet model (a non-index string absent from the symbol table, an
    /// object needing `ToPrimitive`), so the caller self-names an honest skip.
    fn to_at_key(&self, key: Slot) -> Option<Slot> {
        match key.kind {
            Kind::Integer => {
                let i = match key.value {
                    Payload::Integer(i) => i,
                    _ => return None,
                };
                if i >= 0 {
                    Some(Slot::of(Kind::At, Payload::At(crate::value::XS_NO_ID, i as u32)))
                } else {
                    // A negative integer is not an array index; it names a
                    // string property key ("-1"). Resolve it as a name id.
                    let name = number_to_ecma_string(i as f64);
                    self.symbol_ids
                        .get(&name)
                        .map(|&id| Slot::of(Kind::At, Payload::At(id, 0)))
                }
            }
            Kind::Number => {
                let n = match key.value {
                    Payload::Number(n) => n,
                    _ => return None,
                };
                // A non-negative integral number within the index range is an
                // index key; anything else names a string key.
                if n >= 0.0 && n.fract() == 0.0 && n < 4294967295.0 {
                    Some(Slot::of(Kind::At, Payload::At(crate::value::XS_NO_ID, n as u32)))
                } else {
                    let name = number_to_ecma_string(n);
                    self.symbol_ids
                        .get(&name)
                        .map(|&id| Slot::of(Kind::At, Payload::At(id, 0)))
                }
            }
            Kind::Symbol => {
                // A symbol key: XS uses the symbol's own id. endor models the
                // symbol keys the program names via the symbol table; a bare
                // Symbol value key is out of the covered grammar.
                None
            }
            Kind::String => {
                // A string key names a property; resolve it to the program's
                // symbol id (an index-valued string routes to the array item).
                let content = match key.value {
                    Payload::String(off) => self.str_content(off).to_vec(),
                    _ => return None,
                };
                let s = String::from_utf8_lossy(&content);
                if let Some(idx) = string_to_index(&s) {
                    Some(Slot::of(Kind::At, Payload::At(crate::value::XS_NO_ID, idx)))
                } else {
                    self.symbol_ids
                        .get(s.as_ref())
                        .map(|&id| Slot::of(Kind::At, Payload::At(id, 0)))
                }
            }
            _ => None,
        }
    }

    /// Read a computed (`AT`-key) property (`GET_PROPERTY_AT`): an array index
    /// reads the item (or `undefined` for a hole / past the end); a named key
    /// reads the (own-or-inherited) property. Meters no built-in step, like
    /// `GET_PROPERTY`.
    fn property_at_get(&mut self, obj: Slot, key: Slot) -> Result<Slot, Halt> {
        let inst = match obj.value {
            Payload::Reference(i) => i,
            _ => return Ok(Slot::undefined()),
        };
        let (id, index) = match key.value {
            Payload::At(id, index) => (id, index),
            _ => return Err(Halt::Unsupported("get_property_at")),
        };
        if id == crate::value::XS_NO_ID {
            // An index key.
            if self.arrays.contains_key(&inst) {
                Ok(self
                    .arrays
                    .get(&inst)
                    .and_then(|a| a.items.get(&index).copied())
                    .map(|s| Slot::of(s.kind, s.value))
                    .unwrap_or_else(Slot::undefined))
            } else {
                // A non-array object indexed numerically stores its items in a
                // separate index chunk (XS's `fxSetIndexProperty` on an
                // ordinary object) whose allocation metering endor does not yet
                // model — honest skip rather than a wrong/meter-divergent value.
                let _ = index;
                Err(Halt::Unsupported("get_property_at"))
            }
        } else if Some(id) == self.length_id && self.arrays.contains_key(&inst) {
            self.meter.tick_raw(ARRAY_LENGTH_GET_METERING);
            Ok(Slot::integer(self.arrays[&inst].length as i32))
        } else {
            Ok(self.instance_get(inst, id))
        }
    }

    /// Write a computed (`AT`-key) property. `define` distinguishes
    /// `NEW_PROPERTY_AT` (a literal/`Object.defineProperty`-style define,
    /// which meters one extra built-in step) from `SET_PROPERTY_AT` (a plain
    /// assignment). An array index grows/overwrites the item chunk; a named
    /// key routes to the ordinary property store or the array length.
    fn property_at_set(
        &mut self,
        obj: Slot,
        key: Slot,
        value: Slot,
        define: bool,
    ) -> Result<(), Halt> {
        let inst = match obj.value {
            Payload::Reference(i) => i,
            _ => return Ok(()),
        };
        let (id, index) = match key.value {
            Payload::At(id, index) => (id, index),
            _ => return Err(Halt::Unsupported("set_property_at")),
        };
        if id == crate::value::XS_NO_ID {
            if self.arrays.contains_key(&inst) {
                self.array_item_set(inst, index, value, define);
                Ok(())
            } else {
                // Numeric index on a non-array object uses the ordinary-object
                // index chunk (see [`Self::property_at_get`]); its metering is
                // not yet modeled, so this is an honest skip rather than a
                // wrong/meter-divergent store.
                let _ = (index, value, define);
                Err(Halt::Unsupported("set_property_at"))
            }
        } else if Some(id) == self.length_id && self.arrays.contains_key(&inst) {
            self.array_set_length(inst, value);
            Ok(())
        } else {
            self.instance_put(inst, id, value);
            if define {
                self.meter.tick_builtin();
            }
            Ok(())
        }
    }

    /// Set array item `index = value` (XS's `fxSetIndexProperty` +
    /// `fxRunDefine`). Grows the item chunk when the index is new (metered by
    /// [`Self::array_item_grow_metering`]) and bumps `length` when the index
    /// reaches past the end; `define` adds the `fxRunDefine` built-in step.
    fn array_item_set(
        &mut self,
        inst: crate::value::SlotIndex,
        index: u32,
        value: Slot,
        define: bool,
    ) {
        let is_new = !self
            .arrays
            .get(&inst)
            .map(|a| a.items.contains_key(&index))
            .unwrap_or(false);
        if is_new {
            let present = self.arrays[&inst].items.len() as u64;
            self.meter.tick_raw(self.array_item_grow_metering(present));
        }
        if define {
            self.meter.tick_raw(ARRAY_ITEM_DEFINE_STEP_METERING);
        }
        if let Some(a) = self.arrays.get_mut(&inst) {
            let mut v = value;
            v.id = 0;
            v.next = crate::value::SlotIndex::NULL;
            a.items.insert(index, v);
            if index + 1 > a.length {
                a.length = index + 1;
            }
        }
    }

    /// The raw 16.16 chunk-growth cost of appending one item to an array that
    /// already holds `present` items (XS's `fxNewChunk`/`fxRenewChunk` of the
    /// item chunk to `present + 1` slots). XS meters the *adjusted* requested
    /// size: `fxAdjustChunkSize((present+1) * sizeof(txSlot))` with
    /// `sizeof(txSlot) == 32` on the 64-bit oracle target, i.e.
    /// `round_up_8((present+1)*32) + sizeof(txChunk)` = `(present+1)*32 + 16`
    /// (the payload is already 8-aligned). Verified against the pin: an
    /// N-element literal's per-element chunk cost is 48, 80, 112, 144, ….
    ///
    /// Known sub-computron residual: a *spread* segment appending into an
    /// already-populated array carries a −8-raw-per-segment gap (endor
    /// over-charges by 8) versus XS's item-chunk over-allocation
    /// (`fxNewGrowableChunk`/`fxSizeToCapacity`) growth path. It is well under
    /// one computron and never crosses a `>> 16` boundary in a bounded program,
    /// so the computron-level bar (and every corpus/fuzz/test262 check, which
    /// compare `meterIndex >> 16`) stays exact; modeling the over-allocation
    /// capacity to close the raw gap is a later refinement.
    fn array_item_grow_metering(&self, present: u64) -> u64 {
        let bytes = (present + 1) * 32;
        // round up to 8 (already a multiple of 8) + 16-byte chunk header.
        ((bytes + 7) & !7) + 16
    }

    /// Set an array's `length` (XS's `fxArrayLengthSetter` → `fxSetArrayLength`).
    /// Growing past the current length adds holes; shrinking drops the items at
    /// or above the new length. Meters the accessor-setter call machinery
    /// ([`ARRAY_LENGTH_SET_METERING`]); the literal's length prelude sets it
    /// before any item exists, so no chunk realloc is metered there.
    fn array_set_length(&mut self, inst: crate::value::SlotIndex, value: Slot) {
        self.meter.tick_raw(ARRAY_LENGTH_SET_METERING);
        let new_len = self.to_length_u32(value);
        if let Some(a) = self.arrays.get_mut(&inst) {
            if new_len < a.length {
                let drop: Vec<u32> = a
                    .items
                    .range(new_len..)
                    .map(|(&k, _)| k)
                    .collect();
                for k in drop {
                    a.items.remove(&k);
                }
            }
            a.length = new_len;
        }
    }

    /// ToUint32-ish length coercion for `arr.length = v` (the covered grammar
    /// uses integer/number lengths; `fxCheckArrayLength` throws a RangeError on
    /// a non-integer, which is out of the covered set and left to a later
    /// increment).
    fn to_length_u32(&self, value: Slot) -> u32 {
        match value.value {
            Payload::Integer(i) if i >= 0 => i as u32,
            Payload::Number(n) if n >= 0.0 && n.fract() == 0.0 && n <= 4294967295.0 => n as u32,
            _ => 0,
        }
    }

    /// A valid array length (`fxCheckArrayLength`): a non-negative integer in
    /// `[0, 2^32-1]`. Returns `None` for a fractional or out-of-range number
    /// (XS throws a `RangeError` there — out of the covered set).
    fn checked_array_length(&self, value: Slot) -> Option<u32> {
        match value.value {
            Payload::Integer(i) if i >= 0 => Some(i as u32),
            Payload::Number(n) if n >= 0.0 && n.fract() == 0.0 && n <= 4294967295.0 => {
                Some(n as u32)
            }
            _ => None,
        }
    }

    fn new_object(&mut self) -> crate::value::SlotIndex {
        self.meter.tick_builtin();
        self.meter.tick_slot_alloc();
        // Ordinary objects chain to %Object.prototype% (the payload holds the
        // prototype). Property lookup stays own-only, so this is invisible to
        // reads; it exists for the `instanceof` prototype-chain walk.
        self.slots.alloc(Slot::instance(self.object_proto))
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

    /// Delete own property `id` from instance `inst` (XS's
    /// `mxBehaviorDeleteProperty` for an ordinary object): unlink the
    /// property slot from the owner's `next`-linked list and free it.
    /// Returns `true` when the property was configurable-and-removed or was
    /// absent (both are `true` for `delete`); the covered grammar creates
    /// only configurable own data properties, so this is always `true`. No
    /// allocation, so — like XS's ordinary delete — it meters only its
    /// dispatch.
    fn delete_own_property(&mut self, inst: crate::value::SlotIndex, id: u16) -> bool {
        let mut prev = inst;
        let mut cur = self.slots.get(inst).next;
        while !cur.is_null() {
            let s = *self.slots.get(cur);
            if s.id == id {
                // Unlink `cur` from the chain and free its slot.
                self.slots.get_mut(prev).next = s.next;
                self.slots.free(cur);
                return true;
            }
            prev = cur;
            cur = s.next;
        }
        true
    }

    /// Read own property `id` of instance `inst` (or `undefined` when
    /// absent — the covered grammar has a null prototype, so there is no
    /// prototype walk yet).
    fn instance_get(&self, inst: crate::value::SlotIndex, id: u16) -> Slot {
        // Walk the prototype chain (XS's `mxBehaviorGetProperty`): own first,
        // then each prototype, to the root. Metering is unchanged — a chain
        // walk meters no built-in step, exactly as an own read. The prototype
        // objects carry data only for names the program references (the
        // linked intrinsic methods), so this stays invisible to reads of
        // ordinary objects with no matching inherited property.
        let mut cur = inst;
        while !cur.is_null() {
            if let Some(p) = self.find_property(cur, id) {
                let s = self.slots.get(p);
                return Slot::of(s.kind, s.value);
            }
            cur = self.instance_prototype(cur);
        }
        Slot::undefined()
    }

    /// Read a `*_LOCAL_*` opcode's 1-based scope-index operand: a `u8` for
    /// the `_1` variant (`size == 2`), a little-endian `u16` for `_2`
    /// (`size == 3`) — the wide-index form the compiler emits once a frame
    /// declares more than 255 scope slots (XS's `mxRunU1`/`mxRunU2`).
    #[inline]
    fn local_operand(&self, op: Opcode, code: &[u8], pc: usize) -> usize {
        if op.size() == 3 {
            u16::from_le_bytes([code[pc + 1], code[pc + 2]]) as usize
        } else {
            code[pc + 1] as usize
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

    /// ToBoolean with chunk access: a heap string is truthy iff its
    /// content is non-empty (XS's `mxStringLength != 0`); every other kind
    /// defers to the pure [`to_boolean`]. The empty-string case is why this
    /// must route through the machine — a bare `to_boolean` cannot see the
    /// chunk and would call `""` truthy.
    #[inline]
    fn truthy(&self, s: &Slot) -> bool {
        match s.value {
            Payload::String(off) => !self.str_content(off).is_empty(),
            _ => to_boolean(s),
        }
    }

    // Binary numeric arithmetic, ported from the xsRun.c integer fast
    // paths with checked-overflow promotion to f64. A string operand needs
    // `ToNumber(string)` (string→number parsing), outside the covered
    // primitive subset, so it returns `Err` and the caller self-names
    // unsupported rather than producing a spurious `NaN`. (A reference
    // operand ToPrimitives to `NaN` for a plain object, which matches C-XS,
    // so it is left on the numeric path.)
    fn binary_arith(&mut self, op: ArithOp) -> Result<(), ()> {
        let b = self.pop();
        let a = self.pop();
        if a.kind == Kind::String || b.kind == Kind::String {
            return Err(());
        }
        self.push(apply_arith(op, &a, &b));
        Ok(())
    }

    fn binary_bit(&mut self, op: BitOp) -> Result<(), ()> {
        let b = self.pop();
        let a = self.pop();
        if a.kind == Kind::String || b.kind == Kind::String {
            return Err(());
        }
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
                return Ok(());
            }
        }
        self.push(Slot::integer(r));
        Ok(())
    }

    /// Relational comparison (`<`/`<=`/`>`/`>=`). Two strings compare
    /// lexicographically by CESU-8 byte (== UTF-16 code-unit order, XS's
    /// `c_strcmp`, and the ECMAScript abstract relational comparison on
    /// strings); two numerics compare as `f64` with NaN → false. A mixed
    /// string/numeric pair needs `ToNumber(string)` (or `ToPrimitive` of a
    /// reference), outside the covered subset, so it returns `Err` and the
    /// caller self-names unsupported.
    fn relational(&mut self, op: RelOp) -> Result<(), ()> {
        let b = self.pop();
        let a = self.pop();
        if a.kind == Kind::String && b.kind == Kind::String {
            if let (Payload::String(x), Payload::String(y)) = (a.value, b.value) {
                let r = {
                    let (ca, cb) = (self.str_content(x), self.str_content(y));
                    match op {
                        RelOp::Less => ca < cb,
                        RelOp::LessEqual => ca <= cb,
                        RelOp::More => ca > cb,
                        RelOp::MoreEqual => ca >= cb,
                    }
                };
                self.push(Slot::boolean(r));
                return Ok(());
            }
        }
        if a.kind == Kind::String || b.kind == Kind::String {
            return Err(());
        }
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
        Ok(())
    }

    /// Equality (`===`/`!==`/`==`/`!=`). String↔string compares content
    /// bytes; string↔{null,undefined} is unequal on both operators;
    /// string↔{number,boolean,reference} is unequal under `===` (a type
    /// mismatch) but needs `ToNumber(string)` under `==`, so the loose case
    /// returns `Err` (the caller self-names unsupported). Non-string kinds
    /// keep the existing primitive/reference-identity comparison.
    fn equality(&mut self, strict: bool, negate: bool) -> Result<(), ()> {
        let b = self.pop();
        let a = self.pop();
        let eq = match (a.kind, b.kind) {
            (Kind::String, Kind::String) => match (a.value, b.value) {
                (Payload::String(x), Payload::String(y)) => {
                    self.str_content(x) == self.str_content(y)
                }
                _ => false,
            },
            // A string is never `==`/`===` to null/undefined.
            (Kind::String, Kind::Null)
            | (Kind::String, Kind::Undefined)
            | (Kind::Null, Kind::String)
            | (Kind::Undefined, Kind::String) => false,
            (Kind::String, _) | (_, Kind::String) => {
                if strict {
                    false // `===` across types is false without coercion
                } else {
                    return Err(()); // `==` needs ToNumber(string)
                }
            }
            _ => {
                if strict {
                    strict_equals(&a, &b)
                } else {
                    loose_equals(&a, &b)
                }
            }
        };
        self.push(Slot::boolean(eq ^ negate));
        Ok(())
    }

    /// `XS_CODE_ADD` with the string/reference cases (xsRun.c's
    /// `XS_CODE_ADD_GENERAL`): a reference operand needs `ToPrimitive`
    /// (unsupported); a string operand means concatenation
    /// ([`Self::concat_add`]); otherwise the numeric fast path
    /// ([`Self::binary_arith`]).
    fn op_add(&mut self) -> Result<(), ()> {
        let n = self.stack.len();
        if n < 2 {
            return self.binary_arith(ArithOp::Add);
        }
        let a = self.stack[n - 2];
        let b = self.stack[n - 1];
        if a.kind == Kind::Reference || b.kind == Kind::Reference {
            return Err(());
        }
        if a.kind == Kind::String || b.kind == Kind::String {
            self.stack.truncate(n - 2);
            self.concat_add(a, b);
            Ok(())
        } else {
            self.binary_arith(ArithOp::Add)
        }
    }

    /// String `+`: `ToString` both operands and concatenate, metering
    /// exactly at XS's sites — a `ToString` of a number allocates its
    /// rendered chunk (`tick_chunk_new(len+1)`; `ToString` of a
    /// string/boolean/null/undefined is an interned or identity no-op with
    /// no allocation), and `fxConcatString` allocates the joined chunk
    /// `fxNewChunk(aSize + bSize + 1)`. The result is a new heap String.
    fn concat_add(&mut self, a: Slot, b: Slot) {
        let sa = self.to_string_bytes_metered(a);
        let sb = self.to_string_bytes_metered(b);
        // fxConcatString: one fxNewChunk(aSize + bSize + 1).
        self.meter.tick_chunk_new((sa.len() + sb.len() + 1) as u64);
        let mut joined = Vec::with_capacity(sa.len() + sb.len() + 1);
        joined.extend_from_slice(&sa);
        joined.extend_from_slice(&sb);
        joined.push(0); // C NUL terminator, as XS stores
        let off = self.chunks.alloc(&joined);
        self.push(Slot::of(Kind::String, Payload::String(off)));
    }

    /// `ToString` of a primitive to its content bytes (no NUL), metering
    /// the allocation XS's `fxToString` performs: a number renders to a
    /// fresh chunk (`fxNumberToString` → `tick_chunk_new(len+1)`); a string
    /// is identity and a boolean/null/undefined is an interned string, both
    /// allocation-free.
    fn to_string_bytes_metered(&mut self, s: Slot) -> Vec<u8> {
        match s.value {
            Payload::String(off) => self.str_content(off).to_vec(),
            Payload::Integer(i) => {
                let r = i.to_string().into_bytes();
                // `fxToString`/`fxNumberToString` on a number renders into a
                // fresh chunk (`tick_chunk_new(len+1)`) and meters one
                // built-in step (`mxMeterOne`) for the conversion — measured
                // against the pin as exactly `XS_BUILTIN_METERING` over the
                // allocation.
                self.meter.tick_builtin();
                self.meter.tick_chunk_new((r.len() + 1) as u64);
                r
            }
            Payload::Number(n) => {
                let r = number_to_ecma_string(n).into_bytes();
                self.meter.tick_builtin();
                self.meter.tick_chunk_new((r.len() + 1) as u64);
                r
            }
            Payload::Boolean(bv) => if bv { b"true".to_vec() } else { b"false".to_vec() },
            Payload::None => match s.kind {
                Kind::Null => b"null".to_vec(),
                _ => b"undefined".to_vec(),
            },
            Payload::Reference(_) => Vec::new(), // unreachable: op_add rejects references
            Payload::At(..) => Vec::new(),        // unreachable: not a primitive value
        }
    }
}

/// The primitive value globals XS's realm exposes by name (non-writable,
/// non-configurable): a reference reads the value with no allocation, so
/// binding them is metering-neutral. Returns the slot value for a known
/// name, else `None` (leaving the name to resolve as an ordinary global).
fn value_global(name: &str) -> Option<Slot> {
    match name {
        "undefined" => Some(Slot::undefined()),
        "NaN" => Some(Slot::number(f64::NAN)),
        "Infinity" => Some(Slot::number(f64::INFINITY)),
        _ => None,
    }
}

/// A `&'static str` naming an unmodeled native **call** for
/// [`Halt::Unsupported`], so the differential runner records the skip
/// attributed to the specific built-in (never a silent mis-execution).
fn native_unsupported_name(native: Native) -> &'static str {
    match native {
        Native::Object => "native-call:Object",
        Native::Function => "native-call:Function",
        Native::Boolean => "native-call:Boolean",
        Native::Symbol => "native-call:Symbol",
        Native::Number => "native-call:Number",
        Native::String => "native-call:String",
        Native::Array => "native-call:Array",
        Native::Error => "native-call:Error",
        Native::EvalError => "native-call:EvalError",
        Native::RangeError => "native-call:RangeError",
        Native::ReferenceError => "native-call:ReferenceError",
        Native::SyntaxError => "native-call:SyntaxError",
        Native::TypeError => "native-call:TypeError",
        Native::URIError => "native-call:URIError",
        Native::AggregateError => "native-call:AggregateError",
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
        Payload::At(..) => true, // a transient key is never ToBoolean'd
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
        // Reference identity: two references are `===` iff they name the
        // same arena instance (XS compares `value.reference` pointers).
        (Payload::Reference(x), Payload::Reference(y)) => x == y,
        _ => false,
    }
}

/// `fx_pow` (xsMath.c:552): the `**` / `Math.pow` core. ECMAScript's
/// exponentiation returns NaN when the base's magnitude is 1 and the
/// exponent is non-finite; otherwise it is C `pow`, which Rust's
/// `f64::powf` lowers to the same libm call, so the result is bit-exact
/// with the oracle.
fn fx_pow(x: f64, y: f64) -> f64 {
    if !y.is_finite() && x.abs() == 1.0 {
        return f64::NAN;
    }
    x.powf(y)
}

/// The `f64` value of a primitive numeric slot (integer or number), or
/// `None` for any other kind — the fast-path guard the numeric opcodes
/// share, mirroring XS's `XS_INTEGER_KIND`/`XS_NUMBER_KIND` discrimination
/// before its general (ToNumeric/BigInt) fallback.
#[inline]
fn numeric_of(s: &Slot) -> Option<f64> {
    match (s.kind, s.value) {
        (Kind::Integer, Payload::Integer(i)) => Some(i as f64),
        (Kind::Number, Payload::Number(n)) => Some(n),
        _ => None,
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
    fn every_opcode_decodes_and_dispatches_without_panic_or_decode_error() {
        // Full 245-opcode decode + dispatch coverage (the stage-2 bar's
        // "full opcode coverage, built-ins stubbed"): every opcode byte
        // must (a) decode (`from_u8` is dense), (b) resolve an instruction
        // length on a well-formed instruction, and (c) DISPATCH to a
        // defined effect — either it executes (the implemented subset and
        // the pure stubs) or it halts `Halt::Unsupported` naming itself.
        // It must NEVER panic and NEVER fall through to `Halt::Decode` on a
        // well-formed single instruction: a stubbed opcode either steps
        // with faithful stack/frame/meter effects (where its semantics need
        // no built-in) or self-names as unsupported (where they do), so a
        // future grammar reaching an unmodeled opcode gets an honest
        // "implement me", never a silent mis-execution.
        for raw in 0..=crate::opcode::XS_CODE_COUNT as u16 - 1 {
            let byte = raw as u8;
            let op = Opcode::from_u8(byte).expect("opcode table is dense over 0..=245");

            // A well-formed program: enter a sloppy frame, then the opcode
            // under test with zeroed operands (16 pad bytes cover every
            // fixed operand width; length-prefixed opcodes read a zero
            // length). The trailing pad is `XS_NO_CODE`, which self-names as
            // unsupported, so after a *successful* dispatch the run halts
            // there — never on a decode error attributable to the opcode.
            let mut code = vec![b(Opcode::XS_CODE_BEGIN_SLOPPY), 0x00, byte];
            code.extend_from_slice(&[0u8; 16]);

            let out = Interp::new().run(&code);
            if let Halt::Decode(msg) = &out.halt {
                // A decode error is only acceptable if it is NOT about the
                // opcode under test — i.e. the opcode dispatched fine and
                // the walk later tripped on the pad. In practice the pad is
                // NO_CODE (unsupported, not a decode error), so any Decode
                // here is a real gap.
                panic!(
                    "opcode {:#04x} ({}) produced a decode error: {}",
                    byte,
                    op.name(),
                    msg
                );
            }
            // The halt must be one of the defined outcomes; `Unsupported`
            // must name the opcode under test (or `XS_NO_CODE`/a downstream
            // opcode reached after a clean dispatch), never be empty-by-bug.
            match out.halt {
                Halt::Return
                | Halt::Throw(_)
                | Halt::MeterAbort
                | Halt::Unsupported(_)
                | Halt::StackOverflow(_) => {}
                Halt::Decode(_) => unreachable!("handled above"),
            }
        }
    }

    #[test]
    fn caught_throw_runs_and_meters_bit_exact() {
        // The exact C-XS bytecode for `try { throw 7 } catch (e) { e }`
        // (captured from the oracle), run oracle-free: `catch` pushes a
        // jump, `throw` unwinds to it restoring the stack/scope cuts,
        // `exception` binds the thrown 7 into `e`, and the completion is
        // `7` with the C-XS computron count `38` — a standing lock on the
        // jump-chain semantics and dispatch-only exception metering.
        let code: [u8; 59] = [
            0x0b, 0x00, 0x4b, 0x9e, 0x04, 0x8b, 0x8b, 0x8b, 0x72, 0x00, 0xb5, 0x02, 0x92, 0x29,
            0x08, 0xe0, 0xbb, 0x72, 0x07, 0xd7, 0x16, 0x11, 0xdf, 0x29, 0x14, 0xe0, 0xbb, 0x86,
            0x01, 0x00, 0x4f, 0x7a, 0x04, 0x92, 0x5c, 0x04, 0xbb, 0xe2, 0x01, 0x72, 0x02, 0xb5,
            0x02, 0x92, 0xdf, 0x4f, 0xb5, 0x01, 0x92, 0x5c, 0x02, 0x22, 0x03, 0x5c, 0x01, 0xd7,
            0xe2, 0x03, 0xa9,
        ];
        let out = Interp::new().run(&code);
        assert_eq!(out.halt, Halt::Return);
        assert!(out.completed);
        assert_eq!(out.result, "7", "the catch binds and returns the thrown value");
        assert_eq!(out.computrons, 38, "bit-exact computrons vs C-XS");
    }

    #[test]
    fn uncaught_throw_escapes_to_host_and_meters_bit_exact() {
        // The exact C-XS bytecode for `throw 7` (captured from the oracle),
        // run oracle-free: with no handler on the jump chain the throw
        // escapes to the host as `Halt::Throw("7")`, and the computron
        // count is C-XS's `6` — the escaping opcode is un-metered and the
        // host-boundary constant `THROW_HOST_ESCAPE_METERING` is accrued
        // (`begin`, `eval_environment`, `integer` = 3 metered opcodes plus
        // the 3-dispatch invocation baseline, the escaping `throw` dropped).
        let code: [u8; 7] = [0x0b, 0x00, 0x4b, 0x72, 0x07, 0xd7, 0xa9];
        let out = Interp::new().run(&code);
        assert_eq!(out.halt, Halt::Throw("7".into()), "no handler ⇒ escape to host");
        assert!(!out.completed);
        assert_eq!(out.computrons, 6, "bit-exact host-escape computrons vs C-XS");
    }

    #[test]
    fn bare_intrinsic_reference_renders_as_native_function() {
        // The exact C-XS bytecode for `Boolean` (captured from the oracle):
        // begin_sloppy, eval_environment, eval_reference #1,
        // get_variable #1, set_result, return. With the intrinsic linked to
        // symbol id 1 the completion is the native function, rendered by
        // Function.prototype.toString's host-function form, at C-XS's 9
        // computrons (pure dispatch + program setup).
        let code: [u8; 11] = [
            0x0b, 0x00, 0x4b, 0x4d, 0x01, 0x00, 0x67, 0x01, 0x00, 0xbb, 0xa9,
        ];
        let mut interp = Interp::new();
        interp.link_intrinsics(&["Boolean".to_string()]);
        let out = interp.run(&code);
        assert_eq!(out.halt, Halt::Return);
        assert!(out.completed);
        assert_eq!(out.result, "function [\"Boolean\"] (){[native code]}");
        assert_eq!(out.computrons, 9, "bit-exact computrons vs C-XS");
    }

    #[test]
    fn native_boolean_call_coerces_and_meters_bit_exact() {
        // The exact C-XS bytecode for `Boolean(1)` (captured from the
        // oracle): the native call path runs ToBoolean and returns `true`
        // at C-XS's 13 computrons — the native adds no metering beyond the
        // call's dispatch.
        let code: [u8; 17] = [
            0x0b, 0x00, 0x4b, 0xe0, 0x4d, 0x01, 0x00, 0x66, 0x01, 0x00, 0x28, 0x72, 0x01, 0xab,
            0x01, 0xbb, 0xa9,
        ];
        let mut interp = Interp::new();
        interp.link_intrinsics(&["Boolean".to_string()]);
        let out = interp.run(&code);
        assert_eq!(out.halt, Halt::Return);
        assert!(out.completed);
        assert_eq!(out.result, "true");
        assert_eq!(out.computrons, 13, "bit-exact computrons vs C-XS");
    }

    #[test]
    fn user_constructor_new_runs_and_meters_bit_exact() {
        // The exact C-XS bytecode for `function F(a){this.x=a}; (new F(5)).x`
        // (captured from the oracle): the construct path — `new` reshaping the
        // frame with the uninitialized `this` placeholder, `begin`'s
        // fxRunConstructor allocating the fresh instance, the body setting
        // `this.x`, `end` returning `this` — yields `5` at C-XS's 43
        // computrons, a standing lock on the construct frame geometry and its
        // fixed host-frame metering without linking C.
        let code: [u8; 69] = [
            0x0b, 0x00, 0x9e, 0x01, 0x86, 0x01, 0x00, 0x8e, 0xe6, 0x01, 0x92, 0x4b, 0x4d, 0x01,
            0x00, 0x38, 0x01, 0x00, 0x2e, 0x14, 0x0b, 0x01, 0x9e, 0x01, 0x86, 0x02, 0x00, 0x02,
            0x00, 0xe6, 0x01, 0x92, 0xd6, 0x5c, 0x01, 0xb9, 0x03, 0x00, 0x92, 0x44, 0x58, 0x92,
            0x42, 0xe0, 0x89, 0x04, 0x00, 0x72, 0x04, 0xbf, 0x01, 0x00, 0x92, 0x4d, 0x01, 0x00,
            0x67, 0x01, 0x00, 0x84, 0x72, 0x05, 0xab, 0x01, 0x60, 0x03, 0x00, 0xbb, 0xa9,
        ];
        let out = Interp::new().run(&code);
        assert_eq!(out.halt, Halt::Return);
        assert!(out.completed);
        assert_eq!(out.result, "5", "new F(5).x reads the constructed property");
        assert_eq!(out.computrons, 43, "bit-exact computrons vs C-XS");
    }

    #[test]
    fn symbol_create_and_typeof_meters_bit_exact() {
        // The exact C-XS bytecode for `typeof Symbol()` (captured from the
        // oracle): `Symbol()` creates a fresh symbol primitive, `typeof`
        // reads "symbol", at C-XS's 13 computrons (the symbol-creation cost
        // plus dispatch).
        let code: [u8; 16] = [
            0x0b, 0x00, 0x4b, 0xe0, 0x4d, 0x01, 0x00, 0x66, 0x01, 0x00, 0x28, 0xab, 0x00, 0xde,
            0xbb, 0xa9,
        ];
        let mut interp = Interp::new();
        interp.link_intrinsics(&["Symbol".to_string()]);
        let out = interp.run(&code);
        assert_eq!(out.halt, Halt::Return);
        assert!(out.completed);
        assert_eq!(out.result, "symbol");
        assert_eq!(out.computrons, 13, "bit-exact computrons vs C-XS");
    }

    #[test]
    fn bare_symbol_completion_is_a_typeerror_abort() {
        // A program whose completion value is a Symbol aborts: the harness's
        // `String(result)` throws (a symbol cannot coerce to a string). The
        // exact C-XS bytecode for `Symbol()` (captured from the oracle).
        let code: [u8; 15] = [
            0x0b, 0x00, 0x4b, 0xe0, 0x4d, 0x01, 0x00, 0x66, 0x01, 0x00, 0x28, 0xab, 0x00, 0xbb,
            0xa9,
        ];
        let mut interp = Interp::new();
        interp.link_intrinsics(&["Symbol".to_string()]);
        let out = interp.run(&code);
        assert_eq!(
            out.halt,
            Halt::Throw("TypeError: cannot coerce symbol to string".into())
        );
        assert!(!out.completed);
    }

    #[test]
    fn object_prototype_method_dispatch_meters_bit_exact() {
        // The exact C-XS bytecode for `({a:1}).hasOwnProperty('a')` (captured
        // from the oracle): `.hasOwnProperty` resolves up the prototype chain
        // to %Object.prototype%'s native method, which is dispatched with the
        // object as receiver and answers `true` at C-XS's 21 computrons.
        let code: [u8; 33] = [
            0x0b, 0x00, 0x4b, 0x9e, 0x01, 0x8b, 0x90, 0xb5, 0x01, 0x5c, 0x01, 0x72, 0x01, 0x89,
            0x01, 0x00, 0x72, 0x00, 0xe2, 0x01, 0x42, 0x60, 0x02, 0x00, 0x28, 0xc9, 0x02, 0x61,
            0x00, 0xab, 0x01, 0xbb, 0xa9,
        ];
        let mut interp = Interp::new();
        interp.link_intrinsics(&["a".to_string(), "hasOwnProperty".to_string()]);
        let out = interp.run(&code);
        assert_eq!(out.halt, Halt::Return);
        assert!(out.completed);
        assert_eq!(out.result, "true");
        assert_eq!(out.computrons, 21, "bit-exact computrons vs C-XS");
    }

    #[test]
    fn instanceof_prototype_chain_walk_meters_bit_exact() {
        // The exact C-XS bytecode for `({}) instanceof Object` (captured from
        // the oracle): the object's prototype chain reaches %Object.prototype%
        // = Object.prototype, so the result is `true` at C-XS's 19 computrons
        // (the fxOrdinaryHasInstance host-frame call + the object-chain walk,
        // 4 computrons over the dispatch).
        let code: [u8; 20] = [
            0x0b, 0x00, 0x4b, 0x9e, 0x01, 0x8b, 0x90, 0xb5, 0x01, 0xe2, 0x01, 0x4d, 0x01, 0x00,
            0x67, 0x01, 0x00, 0x70, 0xbb, 0xa9,
        ];
        let mut interp = Interp::new();
        interp.link_intrinsics(&["Object".to_string()]);
        let out = interp.run(&code);
        assert_eq!(out.halt, Halt::Return);
        assert!(out.completed);
        assert_eq!(out.result, "true");
        assert_eq!(out.computrons, 19, "bit-exact computrons vs C-XS");
    }

    #[test]
    fn new_error_constructs_renders_and_meters_bit_exact() {
        // The exact C-XS bytecode for `new Error('boom')` (captured from the
        // oracle): the native Error constructor builds an error object whose
        // completion stringifies `Error: boom` (Error.prototype.toString) at
        // C-XS's 13 computrons.
        let code: [u8; 21] = [
            0x0b, 0x00, 0x4b, 0x4d, 0x01, 0x00, 0x67, 0x01, 0x00, 0x84, 0xc9, 0x05, 0x62, 0x6f,
            0x6f, 0x6d, 0x00, 0xab, 0x01, 0xbb, 0xa9,
        ];
        let mut interp = Interp::new();
        interp.link_intrinsics(&["Error".to_string()]);
        let out = interp.run(&code);
        assert_eq!(out.halt, Halt::Return);
        assert!(out.completed);
        assert_eq!(out.result, "Error: boom");
        assert_eq!(out.computrons, 13, "bit-exact computrons vs C-XS");
    }

    #[test]
    fn uncaught_thrown_error_escapes_with_real_error_value_bit_exact() {
        // The exact C-XS bytecode for `throw new TypeError('nope')` (captured
        // from the oracle): an uncaught real Error escapes to the host as
        // `TypeError: nope` (graduating abort-value parity from primitive
        // throws) at C-XS's 12 computrons.
        let code: [u8; 21] = [
            0x0b, 0x00, 0x4b, 0x4d, 0x01, 0x00, 0x67, 0x01, 0x00, 0x84, 0xc9, 0x05, 0x6e, 0x6f,
            0x70, 0x65, 0x00, 0xab, 0x01, 0xd7, 0xa9,
        ];
        let mut interp = Interp::new();
        interp.link_intrinsics(&["TypeError".to_string()]);
        let out = interp.run(&code);
        assert_eq!(out.halt, Halt::Throw("TypeError: nope".into()));
        assert!(!out.completed);
        assert_eq!(out.computrons, 12, "bit-exact host-escape computrons vs C-XS");
    }

    #[test]
    fn native_object_construct_allocates_and_meters_bit_exact() {
        // The exact C-XS bytecode for `new Object()` (captured from the
        // oracle): the native Object constructor allocates a fresh empty
        // object (rendered `[object Object]`) at C-XS's 11 computrons — one
        // fxNewObject plus one built-in step, the fractional gap over a bare
        // object literal.
        let code: [u8; 14] = [
            0x0b, 0x00, 0x4b, 0x4d, 0x01, 0x00, 0x67, 0x01, 0x00, 0x84, 0xab, 0x00, 0xbb, 0xa9,
        ];
        let mut interp = Interp::new();
        interp.link_intrinsics(&["Object".to_string()]);
        let out = interp.run(&code);
        assert_eq!(out.halt, Halt::Return);
        assert!(out.completed);
        assert_eq!(out.result, "[object Object]");
        assert_eq!(out.computrons, 11, "bit-exact computrons vs C-XS");
    }

    #[test]
    fn value_global_undefined_resolves_pure_dispatch() {
        // The exact C-XS bytecode for `undefined` (captured from the
        // oracle): the value global resolves to `undefined` at C-XS's 9
        // computrons (pure dispatch — a global read meters no built-in step).
        let code: [u8; 11] = [
            0x0b, 0x00, 0x4b, 0x4d, 0x01, 0x00, 0x67, 0x01, 0x00, 0xbb, 0xa9,
        ];
        let mut interp = Interp::new();
        interp.link_intrinsics(&["undefined".to_string()]);
        let out = interp.run(&code);
        assert_eq!(out.halt, Halt::Return);
        assert!(out.completed);
        assert_eq!(out.result, "undefined");
        assert_eq!(out.computrons, 9);
    }

    #[test]
    fn unlinked_intrinsic_name_still_misses() {
        // Without linking, `Boolean` is an ordinary undeclared global: the
        // reference misses and the run throws, exactly as before the
        // intrinsics seam (a program that references an unbound global is
        // an honest endor abort, not a silent completion).
        let code: [u8; 11] = [
            0x0b, 0x00, 0x4b, 0x4d, 0x01, 0x00, 0x67, 0x01, 0x00, 0xbb, 0xa9,
        ];
        let out = Interp::new().run(&code);
        assert!(matches!(out.halt, Halt::Throw(_)), "unbound global must miss");
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
        Payload::At(..) => String::new(), // a transient computed key, never rendered
    }
}

/// Parse a string that is a canonical array-index (a "CanonicalNumericIndex"
/// in `[0, 2^32-1)` with no leading zeros or sign), returning the index.
/// `"0"`, `"1"`, `"10"` are indices; `"01"`, `"-1"`, `"1.5"`, `"4294967295"`
/// (the max length, not an index), and `""` are not.
fn string_to_index(s: &str) -> Option<u32> {
    if s.is_empty() || s.len() > 10 {
        return None;
    }
    let bytes = s.as_bytes();
    if bytes[0] == b'0' && s.len() > 1 {
        return None; // no leading zeros
    }
    if !bytes.iter().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u64 = s.parse().ok()?;
    if n < 4294967295 {
        Some(n as u32)
    } else {
        None
    }
}
