#![forbid(unsafe_code)]
//! endor-262: the dual-run harness (design § test262 conformance;
//! requirement 6).
//!
//! For each program it executes the source on the C-XS oracle
//! (`endor-oracle`) to obtain `(bytecode, result, run-only computrons)`
//! and runs that exact bytecode on `endor-vm`, then records four-valued
//! agreement plus computron agreement. Matching the oracle's *fail*
//! vector matters as much as its pass vector: a program endor completes
//! that C-XS throws on (or vice versa) is a divergence, never a silent
//! improvement.
//!
//! Stage 1 ships a curated corpus under `corpora/` (arithmetic, logic,
//! control flow); it grows into whole-section runs in later stages.
//! Those whole-section runs draw from the monorepo's existing
//! `packages/test262-runner` test262 subset and its `ses-xs-parity`
//! feature markers -- the same tree that package uses to prove
//! XS<->Node HardenedJS parity -- rather than a separate pinned
//! test262 submodule (maintainer directive, PR #600, 2026-07-03;
//! design section "test262 conformance").

use endor_vm::{run_program_with_symbols, Halt, RunOutcome};

pub mod test262;

/// The four-valued completion agreement (design § test262 conformance).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agreement {
    /// Both engines completed normally.
    BothComplete,
    /// Both engines aborted (threw / failed to parse).
    BothAbort,
    /// endor completed where the oracle aborted.
    EndorOnlyComplete,
    /// The oracle completed where endor aborted.
    OracleOnlyComplete,
}

/// One program's dual-run record.
#[derive(Debug, Clone)]
pub struct DualRun {
    pub source: String,
    pub agreement: Agreement,
    /// Completion-value string agreement (only meaningful when both
    /// completed).
    pub result_agrees: bool,
    pub oracle_result: String,
    pub endor_result: String,
    /// Computron agreement (only meaningful when both completed).
    pub computrons_agree: bool,
    pub oracle_computrons: u64,
    pub endor_computrons: u64,
    /// Thrown-value agreement (only meaningful on a shared abort): the
    /// oracle's `String(exception)` versus endor's `Halt::Throw` string.
    pub error_agrees: bool,
    /// The oracle's thrown value coerced to `String()` (valid when the
    /// oracle aborted).
    pub oracle_error: String,
    /// endor's thrown value string, from a `Halt::Throw` halt (empty for
    /// any other halt).
    pub endor_error: String,
    /// Raw 16.16 meter indices, for calibrating fractional
    /// (allocation/built-in) metering on a divergence.
    pub oracle_meter_raw: u64,
    pub endor_meter_raw: u64,
    /// endor's raw dispatched-opcode count (before the invocation
    /// baseline), for isolating a metering divergence.
    pub endor_dispatched: u64,
    /// Why endor stopped, verbatim, so an unsupported opcode names
    /// itself.
    pub endor_halt: Halt,
    /// The exact bytecode C-XS emitted (for disassembly on divergence).
    pub bytecode: Vec<u8>,
}

impl DualRun {
    /// The acceptance-bar predicate for one program: same completion,
    /// same result string, same computrons.
    pub fn is_bit_exact(&self) -> bool {
        match self.agreement {
            Agreement::BothComplete => self.result_agrees && self.computrons_agree,
            // A shared abort is bit-exact only when endor aborted for a
            // reason the oracle can share: a JS-level `Throw`. An
            // `Unsupported` (opcode outside the subset) or `Decode`
            // (truncated/invalid bytecode) halt means endor bailed on
            // bytecode it cannot model — the oracle "also aborting"
            // (a parse error, a different throw) is not agreement and
            // must never pass silently.
            //
            // Now that 2b models real exceptions, the shared-abort arm is
            // tightened to the same standard as `BothComplete` (stage-2a
            // review observation 3): the thrown value must match (the
            // oracle's `String(exception)` == endor's `Halt::Throw`
            // string) AND the computrons must match — the uncaught-throw
            // host-escape path is metered exactly (`interp` §
            // `THROW_HOST_ESCAPE_METERING`), and the oracle shim now
            // records the run-only computron count at the throw. A `Throw`
            // whose value or computrons diverge is a divergence, not a
            // silent pass.
            Agreement::BothAbort => {
                matches!(self.endor_halt, Halt::Throw(_))
                    && self.error_agrees
                    && self.oracle_computrons == self.endor_computrons
            }
            _ => false,
        }
    }
}

/// Run one program on both engines and compare.
///
/// Returns `None` only if the oracle machine itself fails to start.
pub fn dual_run(source: &str) -> Option<DualRun> {
    let oracle = endor_oracle::run(source)?;

    // Pass the oracle's symbols atom so endor relinks the program's
    // intrinsic references (`Object`, `Boolean`, the Error hierarchy, …) to
    // its own intrinsics by name — the C-XS compiler numbers those symbols
    // program-locally, so the id→name table is what makes `Boolean` mean the
    // native `Boolean` and not an undefined variable (design § fundamentals).
    let endor: RunOutcome = run_program_with_symbols(&oracle.bytecode, &oracle.symbols);

    let agreement = match (oracle.completed, endor.completed) {
        (true, true) => Agreement::BothComplete,
        (false, false) => Agreement::BothAbort,
        (false, true) => Agreement::EndorOnlyComplete,
        (true, false) => Agreement::OracleOnlyComplete,
    };

    let result_agrees = oracle.completed && endor.completed && oracle.result == endor.result;
    let computrons_agree =
        oracle.completed && endor.completed && oracle.computrons == endor.computrons;

    // endor's thrown value string comes from a `Halt::Throw`; any other
    // halt yields no comparable error string.
    let endor_error = match &endor.halt {
        Halt::Throw(s) => s.clone(),
        _ => String::new(),
    };
    // The thrown value agrees only on a shared abort where endor threw a
    // JS-level exception (`Halt::Throw`): compare the oracle's
    // `String(exception)` against endor's throw string.
    let error_agrees = !oracle.completed
        && !endor.completed
        && matches!(endor.halt, Halt::Throw(_))
        && oracle.error == endor_error;

    Some(DualRun {
        source: source.to_string(),
        agreement,
        result_agrees,
        oracle_result: oracle.result,
        endor_result: endor.result,
        computrons_agree,
        oracle_computrons: oracle.computrons,
        endor_computrons: endor.computrons,
        error_agrees,
        oracle_error: oracle.error,
        endor_error,
        oracle_meter_raw: oracle.meter_raw as u64,
        endor_meter_raw: endor.meter_raw,
        endor_dispatched: endor.dispatched,
        endor_halt: endor.halt,
        bytecode: oracle.bytecode,
    })
}

/// Parse a corpus file: one program per non-empty, non-`//` line.
/// Keeping entries to a single line keeps the completion value (the
/// last expression) unambiguous for the harness.
pub fn parse_corpus(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with("//"))
        .map(|l| l.to_string())
        .collect()
}

/// The checked-in stage-1 corpus, embedded so tests and the harness
/// share one source of truth.
pub fn stage1_corpus() -> Vec<String> {
    let mut all = Vec::new();
    for text in [
        include_str!("../corpora/arithmetic.js"),
        include_str!("../corpora/logic.js"),
        include_str!("../corpora/control-flow.js"),
    ] {
        all.extend(parse_corpus(text));
    }
    all
}

/// The stage-2 corpus: programs that exercise the program frame, scope
/// slots, `var` bindings, backward-branch control flow (loops), and
/// object/property literals over compiler-emitted bytecode. As of stage
/// 2b these are **bit-exact** (result AND computron) against the oracle:
/// the allocation-faithful object heap reproduces the slot/chunk
/// allocation metering a run-time-allocating program accrues
/// (`endor_vm::interp` § Allocation-faithful metering), so the "16920
/// per var" the differential probe measured in 2a is now reproduced.
/// They **graduate** into the bit-exact bar alongside [`stage1_corpus`].
pub fn stage2_corpus() -> Vec<String> {
    let mut all = Vec::new();
    for text in [
        include_str!("../corpora/stage2-behavioral.js"),
        include_str!("../corpora/stage2-objects.js"),
    ] {
        all.extend(parse_corpus(text));
    }
    all
}

/// The stage-2b user-function corpus (child 2 of the stage-2b
/// orchestration): user functions end to end — definition
/// (`constructor_function`/`function` + `code` + `function_environment`),
/// `call`/`run` frame switching with `argument` binding, `end` popping
/// into the calling frame — over closures-free calls, recursion, nested
/// calls, multiple arguments, local variables, and functions called from
/// loops. Bit-exact (result AND computron) against the oracle: the call
/// machinery is stack-based (dispatch-metered), and the definition
/// allocations are metered at their faithful C-XS sites
/// (`endor_vm::interp` § the `FUNCTION_*` metering constants). The
/// meter-check placement matches C-XS's `mxFirstCode` sites (call entry,
/// return-into-a-JS-caller) with **no** check when the program exits to
/// the C caller (`return`).
pub fn stage2b_corpus() -> Vec<String> {
    parse_corpus(include_str!("../corpora/stage2b-functions.js"))
}

/// The stage-2b closure corpus (child 2 of the stage-2b orchestration):
/// closures via heap cells — capture AND mutation, across returned inner
/// functions, curried functions, captured parameters, multiple captured
/// cells, and independent cells per activation. Bit-exact (result AND
/// computron) against the oracle. The captured binding is a shared heap
/// cell (`new_closure` allocates it, `store` captures it into the closure
/// environment, `retrieve` imports it into the callee frame), so a
/// mutation persists across calls and is visible to every capturer, and
/// distinct activations get distinct cells (`endor_vm::interp` §
/// closures).
pub fn stage2b_closures_corpus() -> Vec<String> {
    parse_corpus(include_str!("../corpora/stage2b-closures.js"))
}

/// The stage-2b exception corpus (child 3 of the stage-2b orchestration):
/// exceptions as XS's jump-buffer chain — try/catch/finally, throw, nested
/// handlers, throws crossing call frames, throws from loops, and uncaught
/// propagation to the host boundary. Bit-exact against the oracle on BOTH
/// axes and both completion arms: a caught throw completes and agrees on
/// (result, computron); an uncaught throw is a shared abort and agrees on
/// (thrown-value string, computron) under the tightened
/// [`DualRun::is_bit_exact`] (observation 3). `catch`/`uncatch`/`throw`/
/// `exception`/`rethrow` are dispatch-metered (the jump `c_malloc` and
/// `fxJump` longjmp are unmetered); the uncaught host-escape carries the
/// measured `endor_vm::interp::THROW_HOST_ESCAPE_METERING` constant.
pub fn stage2b_exceptions_corpus() -> Vec<String> {
    parse_corpus(include_str!("../corpora/stage2b-exceptions.js"))
}

/// The stage-3 child-1 (language) corpus: the language opcodes and
/// chunk-backed CESU-8 string *values* this child adds — string literals,
/// concatenation (ToString + `fxConcatString` chunk metering), string
/// equality/relational comparison, `typeof` over every covered kind, the
/// numeric opcodes `increment`/`decrement`/`to_numeric`/exponentiation,
/// `this`, `let`/`const` closures (including a loop body's per-iteration
/// reset/refresh cells), and the `??`/`?.` chaining branches. Bit-exact
/// (result AND computron) against the oracle.
pub fn stage3_language_corpus() -> Vec<String> {
    parse_corpus(include_str!("../corpora/stage3-language.js"))
}

/// The stage-3 child-2 (fundamentals) corpus: the intrinsic constructors as
/// first-class global values (`Object`/`Boolean`/`Symbol`/`Number`/`String`/
/// `Function` and the Error hierarchy), `typeof` over them, and the
/// `Boolean` primitive coercion. Bit-exact (result AND computron) against
/// the oracle: a bare constructor reference resolves to endor's intrinsic
/// (relinked by the program's symbol id → name table) and stringifies as
/// `function ["name"] (){[native code]}`; `typeof` reads "function"; and
/// `Boolean(value)` runs the native ToBoolean with the metering-neutral cost
/// the pin measures (`endor_vm::interp` § the native call path). Built-in
/// construction (`new`), `instanceof`/`in`, and object-returning calls are
/// deferred to later increments and honestly skipped until then.
pub fn stage3_fundamentals_corpus() -> Vec<String> {
    parse_corpus(include_str!("../corpora/stage3-fundamentals.js"))
}

/// Stage-3 child-3 (arrays) curated corpus: the Array exotic object's
/// index/length semantics, array literals with holes, computed element
/// get/set, and item-chunk growth — bit-exact (result AND computron) against
/// the oracle.
pub fn stage3_arrays_corpus() -> Vec<String> {
    parse_corpus(include_str!("../corpora/stage3-arrays.js"))
}

/// Stage-3 child-4 (text-math-json) curated corpus: the `Math` namespace
/// object, its numeric constants, and the modeled `Math.*` statics, plus the
/// Number::toString fixed-vs-exponential rendering corners — bit-exact (result
/// AND computron) against the oracle.
pub fn stage3_math_corpus() -> Vec<String> {
    parse_corpus(include_str!("../corpora/stage3-math.js"))
}

/// Stage-3 child-4 (text-math-json) curated `String.prototype` corpus:
/// primitive string property/method access over the CESU-8 chunk
/// representation — indexing, `.length`, the slice/case/search families, and
/// string building in loops (the metering hot path) — bit-exact (result AND
/// computron) against the oracle.
pub fn stage3_string_corpus() -> Vec<String> {
    parse_corpus(include_str!("../corpora/stage3-string.js"))
}

/// UTF-16-swap child — surrogate-pair / index-heavy / lone-surrogate
/// differential fixtures whose completion is a **scalar** (number/boolean) so
/// the pin transports it faithfully. Asserts RESULT parity (not computron
/// equality — the recalibration re-bases string cost off code-unit length);
/// the storage-layer semantics for astral/lone-surrogate string VALUES (which
/// the CESU-8→UTF-8 shim decodes lossily) are proven in the endor-vm
/// `utf16_*` value-layer tests.
pub fn stage3_string_utf16_corpus() -> Vec<String> {
    parse_corpus(include_str!("../corpora/stage3-string-utf16.js"))
}

/// Stage-3 child-4 (text-math-json) curated Number corpus: the Number
/// statics/predicates, `Number.prototype.toString` (radix 10), `Number(...)`
/// coercion, and the numeric globals `parseInt`/`parseFloat`/`isNaN`/
/// `isFinite` — bit-exact (result AND computron) against the oracle.
pub fn stage3_number_corpus() -> Vec<String> {
    parse_corpus(include_str!("../corpora/stage3-number.js"))
}

/// Stage-3 child-4 (text-math-json) curated JSON corpus: `JSON.stringify` over
/// a top-level primitive (the JSON escaper and the value-independent metering
/// residuals) — bit-exact (result AND computron) against the oracle.
pub fn stage3_json_corpus() -> Vec<String> {
    parse_corpus(include_str!("../corpora/stage3-json.js"))
}

/// The stage-3b (json-metering) curated corpus: structured `JSON.stringify`
/// over object and array values — the recursive `fxStringifyJSONProperty`
/// per-node metering (the keys-list instance and its per-key AT slots, the
/// per-iteration bodies, each key's `fxPushKeyString` chunk, and the final
/// result `fxNewChunk`), bit-exact (result AND computron) against the oracle at
/// the pin `48ee02d8cfe0`. Every constant decomposes to whole `mxMeterOne`
/// steps plus the exact allocations, so the computrons track the node walk.
pub fn stage3b_json_metering_corpus() -> Vec<String> {
    parse_corpus(include_str!("../corpora/stage3b-json-metering.js"))
}

/// The stage-3 child-5 (collections) curated corpus: Map/Set/WeakMap/WeakSet
/// construction, `set`/`add`/`get`/`has`/`delete`/`size`, growth across the
/// hash-table rehash boundaries, SameValueZero key equality, and reference
/// keys — bit-exact (result AND computron) against the oracle. Every entry
/// allocation is `fxNewSlot`-visible and every rehash an `fxNewChunk`, so the
/// computron count tracks the exact allocation sequence.
pub fn stage3_collections_corpus() -> Vec<String> {
    parse_corpus(include_str!("../corpora/stage3-collections.js"))
}

/// The stage-3b (bigint) curated corpus: the BigInt primitive per the pin —
/// literals, the metered `+`/`-`/`*` (digit step over the trimmed result size
/// plus the allocation-faithful result chunk at XS's pre-trim `fxBigInt_alloc`
/// size), unary minus, strict/loose equality (including BigInt-vs-Number via
/// `fxNumberToBigInt`), relational order, `typeof "bigint"`, and decimal
/// completion rendering — bit-exact (completion value AND computron count)
/// against the C-XS oracle. The BigInt arithmetic is `mxMeter`-driven at the
/// digit-step granularity and otherwise allocation-driven, so the computrons
/// track the exact `fxNewChunk` sequence.
pub fn stage3_bigint_corpus() -> Vec<String> {
    parse_corpus(include_str!("../corpora/stage3-bigint.js"))
}

/// The stage-3b (binary-data) curated corpus, child 3/9: the ArrayBuffer
/// surface per the pin — `new ArrayBuffer(byteLength)` (the constant native
/// frame plus the 8-byte-aligned `fxNewChunk(byteLength)` backing store, all
/// zero-filled) and the `byteLength` accessor getter (which meters nothing
/// beyond its `GET_PROPERTY` dispatch) — bit-exact (completion value AND
/// computron count) against the C-XS oracle.
pub fn stage3b_binary_corpus() -> Vec<String> {
    parse_corpus(include_str!("../corpora/stage3b-binary.js"))
}

/// The stage-3b fundamentals-followup corpus (child 4/9): the post-arrays
/// fundamentals follow-up unblocked by the landed Array machinery — a user
/// function's `.length` (its declared arity, set from `begin`'s
/// parameter-count operand at the `code` opcode) and `.name` (its own name,
/// inferred for a `var f = function(){}` initializer) as first-class own
/// data-property reads; `Function.prototype.bind` (bound-function repr with the
/// bound `length`/`name` and the call trampoline); `Function.prototype.apply`
/// with a real (dense) array argument; `Symbol.prototype.toString`/`valueOf`,
/// `String(symbol)`
/// coercion, the `Symbol.for`/`keyFor` registry, and `AggregateError`
/// (base error + the `errors` Array from a dense-array argument). Bit-exact (result AND
/// computron) against the oracle: `.length`/`.name` are own properties
/// allocated at definition (folded into [`crate::interp`]'s
/// `FUNCTION_DEFINE_METERING`), so reading them meters nothing beyond the
/// `GET_PROPERTY` dispatch; the apply and Symbol paths carry constants
/// calibrated against the pin via the raw-gap.
pub fn stage3b_fundamentals_followup_corpus() -> Vec<String> {
    parse_corpus(include_str!("../corpora/stage3b-fundamentals-followup.js"))
}

/// The stage-3b object-statics + intern-table corpus (child 5/9): the global
/// runtime string→id intern table (XS's `fxNewNameX`/`fxAt`) reconciled with
/// the compiler's program symbols and XS's boot-time default keys, exercised
/// through `Object.prototype.hasOwnProperty` over own keys, genuinely-novel
/// keys (each interning one metered `fxNewSlot` key slot), and well-known
/// inherited names (interned without allocation, correctly not own). Bit-exact
/// (result AND computron) against the oracle: an already-interned name
/// resolves with no allocation, a novel one meters exactly one slot, and the
/// table is global and persistent so a repeated novel key never re-allocates.
pub fn stage3b_object_statics_corpus() -> Vec<String> {
    parse_corpus(include_str!("../corpora/stage3b-object-statics.js"))
}

/// The stage-4 object-integrity corpus (child 1/8): the property-attribute
/// integrity model (`Object.preventExtensions`/`seal`/`freeze` +
/// `isExtensible`/`isSealed`/`isFrozen`, the slot-arena `XS_DONT_PATCH_FLAG`
/// and the per-property `XS_DONT_DELETE_FLAG`/`XS_DONT_SET_FLAG` stamps, with
/// the sloppy-mode write/delete rejection those flags impose) plus the
/// descriptor-reflection surface (`Object.values`/`entries`/
/// `getOwnPropertyDescriptors` and `Object.prototype.propertyIsEnumerable`) —
/// harden's direct prerequisite. Bit-exact (result AND computron) against the
/// oracle.
pub fn stage4_object_integrity_corpus() -> Vec<String> {
    parse_corpus(include_str!("../corpora/stage4-object-integrity.js"))
}

/// The stage-4 `new.target` corpus (child 2/8, the landed slice): the
/// `XS_CODE_TARGET` opcode with real semantics — the target constructor inside
/// a construct frame, `undefined` inside a plain call, across the factory-guard
/// idiom and closure-captured constructors. Bit-exact (result AND computron)
/// against the oracle. The wider `class` family (definition/methods/extends/
/// super/private/static) is a reported scope fold that self-names honest skips.
pub fn stage4_new_target_corpus() -> Vec<String> {
    parse_corpus(include_str!("../corpora/stage4-new-target.js"))
}

/// The stage-4 generators corpus (child 3/8): generator functions and the
/// iteration protocol closure — `function*` declarations/expressions and
/// object-literal `*m()` methods, the suspend/resume of the interpreter
/// activation (`START_GENERATOR`/`YIELD`/the `BRANCH_STATUS` resume epilogue),
/// `%GeneratorPrototype%.next(v)`/`return(v)` with sent values and completion
/// results, and `for-of`/spread over a generator. Bit-exact (result AND
/// computron) against the pin. `yield*` delegation, `throw`/`return` into a
/// suspended body, a `yield` inside `try`, `new`-constructed generators, and
/// async generators are honest named skips (excluded from this corpus).
pub fn stage4_generators_corpus() -> Vec<String> {
    parse_corpus(include_str!("../corpora/stage4-generators.js"))
}

/// The stage-3b promises corpus (child 7/9): `Promise` construction + the
/// executor, `resolve`/`reject` settling, the `Promise.resolve`/`reject`
/// statics, `then`/`catch` reaction registration, and the microtask job queue
/// drained by the pump-loop latch — resolution chains, already-settled
/// promises, pass-through, and rejection routing — all bit-exact (result AND
/// computron) against the oracle. The reactions run at the host-driven drain
/// (mirrored in the oracle shim's post-`fxRunScript` `fxRunPromiseJobs` loop),
/// so the metered computrons include the whole crank. Thenable adoption, a
/// throwing/reference-returning handler, `.finally`, the combinators, and
/// async/await are honest named skips.
pub fn stage3b_promises_corpus() -> Vec<String> {
    parse_corpus(include_str!("../corpora/stage3b-promises.js"))
}

/// The stage-4 async/promise keystone corpus (child 4/8): the promise
/// native-handler double-settle calibration and the surfaces it unblocks —
/// thenable adoption (`Promise.resolve(thenable)` / an executor / a handler
/// resolving with a thenable), the two-level `[[AlreadyResolved]]` guard's
/// double-settle no-op (res twice, res+rej, rej+res), long `then`-chains, a
/// handler returning a thenable or a native promise, and the
/// `Promise.resolve(nativePromise)` identity fast path — all bit-exact (result
/// AND computron) against the oracle. A throwing handler/`then`,
/// `resolve(promise-itself)`, and the async-function surface are honest named
/// skips excluded from this corpus.
pub fn stage4_async_promises_corpus() -> Vec<String> {
    parse_corpus(include_str!("../corpora/stage4-async-promises.js"))
}

/// The stage-3b xsre-integration corpus (child 9/9): the JavaScript `RegExp`
/// surface over child 8's matcher — literal + constructor construction,
/// `source`/`flags`/per-flag accessor getters, `exec`/`test` (match, no-match,
/// captures, the stateful g/y drive) and `toString` — all bit-exact (result
/// AND computron) against the oracle. Construction is allocation-driven
/// (`fxNewRegExpInstance`) plus the `fxCompileRegExp` parse meter and a
/// calibrated ctor frame; `exec`/`test` carry the `fxMatchRegExp` step meter
/// plus the result-array clusters and a calibrated exec/test frame. A
/// RegExp-valued pattern arg, named groups, a syntax-error/unsupported pattern
/// feature, and a non-ASCII stateful subject are honest named skips, excluded
/// from the covered corpus.
pub fn stage3b_regexp_corpus() -> Vec<String> {
    parse_corpus(include_str!("../corpora/stage3b-regexp.js"))
}

/// A summary over a corpus run.
#[derive(Debug, Default, Clone)]
pub struct Summary {
    pub total: usize,
    pub bit_exact: usize,
    pub result_divergences: usize,
    pub computron_divergences: usize,
    pub completion_divergences: usize,
    pub unsupported: usize,
}

impl Summary {
    pub fn met_bar(&self) -> bool {
        self.total > 0 && self.bit_exact == self.total
    }
}

/// Run a whole corpus and summarize.
pub fn run_corpus(programs: &[String]) -> (Vec<DualRun>, Summary) {
    let mut runs = Vec::new();
    let mut s = Summary::default();
    for p in programs {
        if let Some(r) = dual_run(p) {
            s.total += 1;
            if r.is_bit_exact() {
                s.bit_exact += 1;
            } else {
                match r.agreement {
                    Agreement::BothComplete => {
                        if !r.result_agrees {
                            s.result_divergences += 1;
                        }
                        if !r.computrons_agree {
                            s.computron_divergences += 1;
                        }
                    }
                    // A non-bit-exact `BothAbort` is an endor
                    // `Unsupported`/`Decode` bail masquerading as a
                    // shared abort (finding 3): count it so it can never
                    // pass silently.
                    Agreement::BothAbort => s.unsupported += 1,
                    _ => s.completion_divergences += 1,
                }
                // An unsupported-opcode bail while the oracle diverged
                // the other way (e.g. `OracleOnlyComplete`); `BothAbort`
                // is already accounted for above.
                if matches!(r.endor_halt, Halt::Unsupported(_))
                    && !matches!(r.agreement, Agreement::BothAbort)
                {
                    s.unsupported += 1;
                }
            }
            runs.push(r);
        }
    }
    (runs, s)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A `DualRun` with the given agreement and endor halt. For a
    // `Halt::Throw`, the oracle is modeled as throwing the same value with
    // the same computrons (the agreeing case), so `is_bit_exact` turns on
    // the halt kind; a non-`Throw` halt never agrees.
    fn abort_run(agreement: Agreement, endor_halt: Halt) -> DualRun {
        let endor_error = match &endor_halt {
            Halt::Throw(s) => s.clone(),
            _ => String::new(),
        };
        let error_agrees = matches!(endor_halt, Halt::Throw(_));
        DualRun {
            source: String::new(),
            agreement,
            result_agrees: false,
            oracle_result: String::new(),
            endor_result: String::new(),
            computrons_agree: false,
            oracle_computrons: 0,
            endor_computrons: 0,
            error_agrees,
            oracle_error: endor_error.clone(),
            endor_error,
            oracle_meter_raw: 0,
            endor_meter_raw: 0,
            endor_dispatched: 0,
            endor_halt,
            bytecode: Vec::new(),
        }
    }

    #[test]
    fn both_abort_bit_exact_only_when_endor_throws() {
        // A matching JS-level throw is a genuine shared abort.
        let throwing = abort_run(Agreement::BothAbort, Halt::Throw("boom".into()));
        assert!(throwing.is_bit_exact(), "BothAbort with a Throw is bit-exact");

        // An `Unsupported` bail is not agreement even if the oracle also
        // aborted (finding 3): it must never pass silently.
        let unsupported = abort_run(Agreement::BothAbort, Halt::Unsupported("XS_CODE_CALL"));
        assert!(
            !unsupported.is_bit_exact(),
            "BothAbort with an Unsupported halt is not bit-exact"
        );

        // A `Decode` bail (truncated/invalid bytecode) is likewise not
        // agreement.
        let decode = abort_run(Agreement::BothAbort, Halt::Decode("truncated".into()));
        assert!(
            !decode.is_bit_exact(),
            "BothAbort with a Decode halt is not bit-exact"
        );
    }

    #[test]
    fn both_abort_throw_requires_error_and_computron_agreement() {
        // Observation 3: a shared `Throw` abort is bit-exact only when the
        // thrown value AND the computrons match, exactly like the
        // `BothComplete` arm — a matching halt kind alone is not enough.
        let mut r = abort_run(Agreement::BothAbort, Halt::Throw("7".into()));
        r.oracle_computrons = 6;
        r.endor_computrons = 6;
        assert!(r.is_bit_exact(), "matching value + computrons is bit-exact");

        // Divergent thrown value: the oracle threw "8" where endor threw "7".
        let mut wrong_value = r.clone();
        wrong_value.oracle_error = "8".into();
        wrong_value.error_agrees = false;
        assert!(!wrong_value.is_bit_exact(), "a divergent thrown value is not bit-exact");

        // Divergent computrons on an otherwise-matching throw.
        let mut wrong_cost = r.clone();
        wrong_cost.endor_computrons = 7;
        assert!(!wrong_cost.is_bit_exact(), "a divergent computron count is not bit-exact");
    }

    #[test]
    fn non_throw_both_abort_is_counted_not_silent() {
        // The summary must count a non-`Throw` `BothAbort` (here under
        // `unsupported`) rather than let it slip through as bit-exact.
        let runs = [
            abort_run(Agreement::BothAbort, Halt::Unsupported("XS_CODE_CALL")),
            abort_run(Agreement::BothAbort, Halt::Decode("truncated".into())),
        ];
        let mut s = Summary::default();
        for r in &runs {
            s.total += 1;
            if r.is_bit_exact() {
                s.bit_exact += 1;
            } else {
                match r.agreement {
                    Agreement::BothComplete => {}
                    Agreement::BothAbort => s.unsupported += 1,
                    _ => s.completion_divergences += 1,
                }
            }
        }
        assert_eq!(s.bit_exact, 0, "neither run may count as bit-exact");
        assert_eq!(s.unsupported, 2, "both non-Throw aborts are counted");
        assert!(!s.met_bar());
    }

    #[test]
    fn stage2_corpus_is_bit_exact_against_oracle() {
        // The graduation bar (stage 2b): every stage-2 program — var
        // bindings, loops, object/property literals — must agree with
        // C-XS on BOTH the completion value AND the computron count. The
        // computron half is what the allocation-faithful object heap
        // buys: a run-time-allocating program's count depends on its
        // exact slot/chunk allocations, which endor now reproduces
        // (the "16920 per var" is reproduced, not measured).
        let programs = stage2_corpus();
        assert!(!programs.is_empty(), "stage-2 corpus must be non-empty");
        let (runs, summary) = run_corpus(&programs);
        for r in &runs {
            if !r.is_bit_exact() {
                eprintln!(
                    "DIVERGENCE {:?}\n  agreement={:?} result oracle={:?} endor={:?}\n  computrons oracle={} endor={} (endor dispatched={})\n  endor halt={:?}\n  bytecode={:02x?}",
                    r.source, r.agreement, r.oracle_result, r.endor_result,
                    r.oracle_computrons, r.endor_computrons, r.endor_dispatched,
                    r.endor_halt, r.bytecode,
                );
            }
        }
        assert!(
            summary.met_bar(),
            "stage-2 bit-exact bar: {}/{} (result_div={}, computron_div={}, completion_div={}, unsupported={})",
            summary.bit_exact, summary.total, summary.result_divergences,
            summary.computron_divergences, summary.completion_divergences, summary.unsupported,
        );
    }

    // Arm a fresh endor interpreter on the oracle's bytecode for `src`,
    // recording every computron value the meter host is consulted at and
    // whether the run was allowed to complete (`allow`) or refused at the
    // `refuse_at`-th consultation (1-based; 0 = never refuse). Returns
    // `(halt, completed, consulted_computrons)`.
    fn metered_run(
        src: &str,
        interval: u64,
        refuse_at: usize,
    ) -> (endor_vm::Halt, bool, Vec<u64>) {
        use endor_vm::Interp;
        use std::cell::RefCell;
        use std::rc::Rc;
        let oracle = endor_oracle::run(src).expect("oracle machine");
        let seen = Rc::new(RefCell::new(Vec::new()));
        let seen_cb = Rc::clone(&seen);
        let mut interp = Interp::new();
        interp.arm_meter(
            interval,
            Box::new(move |computrons| {
                let mut s = seen_cb.borrow_mut();
                s.push(computrons);
                refuse_at == 0 || s.len() < refuse_at
            }),
        );
        let out = interp.run(&oracle.bytecode);
        let consulted = seen.borrow().clone();
        (out.halt, out.completed, consulted)
    }

    #[test]
    fn no_meter_check_when_program_returns_to_c() {
        // A straight-line program (no backward branch, no call) has no
        // loop-closing point, so C-XS never checks the meter — its `return`
        // exits to the C caller unconditionally. An endor armed to refuse
        // immediately therefore still *completes*: the host is never
        // consulted, proving the exit-to-C `return` carries no
        // `mxFirstCode` check (stage-2a review finding 1).
        let (halt, completed, consulted) = metered_run("1 + 2 * 3", 1, 1);
        assert_eq!(halt, endor_vm::Halt::Return, "must complete: no check point");
        assert!(completed);
        assert!(consulted.is_empty(), "the exit-to-C return must not check the meter");
    }

    #[test]
    fn meter_checks_fire_at_call_entry_and_return_into_js() {
        // A single user-function call has exactly two `mxFirstCode` check
        // points: call entry (`run` installing the callee frame) and the
        // callee's `end` returning into the JS program frame. The
        // program's own final `return` (exit to C) does not check. So a
        // permissive armed run is consulted exactly twice and completes.
        let (halt, completed, consulted) =
            metered_run("(function(){return 1})()", 1, 0);
        assert_eq!(halt, endor_vm::Halt::Return);
        assert!(completed);
        assert_eq!(
            consulted.len(),
            2,
            "call entry + return-into-JS check; the exit-to-C return does not check (got {:?})",
            consulted,
        );
    }

    #[test]
    fn armed_meter_aborts_at_call_entry_not_at_program_exit() {
        // Refusing at the first consultation (the call-entry `mxFirstCode`)
        // aborts the crank there — before the callee body's completion is
        // observed — rather than letting the program run to its exit-to-C
        // `return`. This is the abort-point determinism the check-placement
        // fix exists to guarantee.
        let (halt, completed, consulted) =
            metered_run("(function(){return 1})()", 1, 1);
        assert_eq!(halt, endor_vm::Halt::MeterAbort, "must abort at the call-entry check");
        assert!(!completed, "the call must not complete once refused at entry");
        assert_eq!(consulted.len(), 1, "aborts on the first (call-entry) consultation");
    }

    #[test]
    fn armed_meter_aborts_at_backward_branch_in_a_loop() {
        // A loop's backward branch is a check point (as in stage 2a); a
        // function body containing a loop still aborts there under an armed
        // meter, never at the function's `end` exit or the program
        // `return`.
        let src = "var i=0; while(i<1000000){i=i+1} i";
        let (halt, completed, _consulted) = metered_run(src, 1, 3);
        assert_eq!(halt, endor_vm::Halt::MeterAbort, "the backward branch must abort");
        assert!(!completed);
    }

    #[test]
    fn stage2b_functions_corpus_is_bit_exact_against_oracle() {
        // The child-2 acceptance bar: every user-function program — IIFEs,
        // multi-argument calls, local variables, functions stored in vars,
        // named declarations, nested calls, and recursion (fib/fac/sum) —
        // must agree with C-XS on BOTH the completion value AND the
        // computron count. Results follow from the frame machinery
        // (`call`/`run`/`argument`/`end`); computrons follow from
        // dispatch-metered stack frames plus the faithful definition-site
        // allocation metering, with the meter check at C-XS's `mxFirstCode`
        // sites (call entry, return-into-JS) and none at the exit-to-C
        // `return`.
        let programs = stage2b_corpus();
        assert!(!programs.is_empty(), "stage-2b corpus must be non-empty");
        let (runs, summary) = run_corpus(&programs);
        for r in &runs {
            if !r.is_bit_exact() {
                eprintln!(
                    "DIVERGENCE {:?}\n  agreement={:?} result oracle={:?} endor={:?}\n  computrons oracle={} endor={} (endor dispatched={}) raw oracle={} endor={}\n  endor halt={:?}\n  bytecode={:02x?}",
                    r.source, r.agreement, r.oracle_result, r.endor_result,
                    r.oracle_computrons, r.endor_computrons, r.endor_dispatched,
                    r.oracle_meter_raw, r.endor_meter_raw,
                    r.endor_halt, r.bytecode,
                );
            }
        }
        assert!(
            summary.met_bar(),
            "stage-2b bit-exact bar: {}/{} (result_div={}, computron_div={}, completion_div={}, unsupported={})",
            summary.bit_exact, summary.total, summary.result_divergences,
            summary.computron_divergences, summary.completion_divergences, summary.unsupported,
        );
    }

    #[test]
    fn stage2b_closures_corpus_is_bit_exact_against_oracle() {
        // The child-2 closure acceptance bar: every closure program —
        // counters (capture + mutation), captured parameters, curried
        // functions, multiple captured cells, closures used within the
        // enclosing scope, and independent-activation counters that must
        // not alias — agrees with C-XS on BOTH the completion value AND the
        // computron count. The result follows from the shared-heap-cell
        // model (`new_closure`/`store`/`retrieve`/`get`/`pull_closure`); the
        // computrons follow from metering the cell `fxNewSlot`s at
        // `new_closure` and `store` where C-XS allocates them.
        let programs = stage2b_closures_corpus();
        assert!(!programs.is_empty(), "stage-2b closure corpus must be non-empty");
        let (runs, summary) = run_corpus(&programs);
        for r in &runs {
            if !r.is_bit_exact() {
                eprintln!(
                    "DIVERGENCE {:?}\n  agreement={:?} result oracle={:?} endor={:?}\n  computrons oracle={} endor={} (endor dispatched={}) raw oracle={} endor={}\n  endor halt={:?}\n  bytecode={:02x?}",
                    r.source, r.agreement, r.oracle_result, r.endor_result,
                    r.oracle_computrons, r.endor_computrons, r.endor_dispatched,
                    r.oracle_meter_raw, r.endor_meter_raw,
                    r.endor_halt, r.bytecode,
                );
            }
        }
        assert!(
            summary.met_bar(),
            "stage-2b closure bit-exact bar: {}/{} (result_div={}, computron_div={}, completion_div={}, unsupported={})",
            summary.bit_exact, summary.total, summary.result_divergences,
            summary.computron_divergences, summary.completion_divergences, summary.unsupported,
        );
    }

    #[test]
    fn stage2b_exceptions_corpus_is_bit_exact_against_oracle() {
        // The child-3 acceptance bar: every exception program — try with no
        // throw, catch binding the thrown value, try/finally and
        // try/catch/finally, nested handlers, throws crossing call frames,
        // throws from inside a loop, throws of heap values, and UNCAUGHT
        // throws propagating to the host — agrees with C-XS on BOTH the
        // completion (result for a caught throw, thrown-value string for an
        // uncaught one) AND the computron count. Caught throws are
        // dispatch-metered; the uncaught host-escape carries the measured
        // `THROW_HOST_ESCAPE_METERING`, so the shared-abort arm is bit-exact
        // under the tightened predicate.
        let programs = stage2b_exceptions_corpus();
        assert!(!programs.is_empty(), "stage-2b exception corpus must be non-empty");
        let (runs, summary) = run_corpus(&programs);
        for r in &runs {
            if !r.is_bit_exact() {
                eprintln!(
                    "DIVERGENCE {:?}\n  agreement={:?} result oracle={:?} endor={:?} error oracle={:?} endor={:?}\n  computrons oracle={} endor={} (endor dispatched={}) raw oracle={} endor={}\n  endor halt={:?}\n  bytecode={:02x?}",
                    r.source, r.agreement, r.oracle_result, r.endor_result,
                    r.oracle_error, r.endor_error,
                    r.oracle_computrons, r.endor_computrons, r.endor_dispatched,
                    r.oracle_meter_raw, r.endor_meter_raw,
                    r.endor_halt, r.bytecode,
                );
            }
        }
        assert!(
            summary.met_bar(),
            "stage-2b exception bit-exact bar: {}/{} (result_div={}, computron_div={}, completion_div={}, unsupported={})",
            summary.bit_exact, summary.total, summary.result_divergences,
            summary.computron_divergences, summary.completion_divergences, summary.unsupported,
        );
    }

    #[test]
    fn uncaught_throw_is_a_bit_exact_shared_abort() {
        // Behavioural spot-check decoupled from the corpus: an uncaught
        // throw is a shared abort whose thrown-value string and run-only
        // computron count both match the oracle (the host-escape metering
        // and the shim's abort-path computron capture together make the
        // shared-abort arm bit-exact, not merely "endor also threw").
        let r = dual_run("throw 7").expect("oracle");
        assert_eq!(r.agreement, Agreement::BothAbort);
        assert_eq!(r.endor_error, "7");
        assert_eq!(r.oracle_error, "7");
        assert_eq!(r.oracle_computrons, r.endor_computrons, "uncaught-throw computrons agree");
        assert!(r.is_bit_exact(), "an agreeing uncaught throw is bit-exact");

        // A caught throw completes; its result and computrons agree.
        let c = dual_run("try { throw 7 } catch (e) { e + 1 }").expect("oracle");
        assert_eq!(c.agreement, Agreement::BothComplete);
        assert_eq!(c.endor_result, "8");
        assert!(c.is_bit_exact());
    }

    #[test]
    fn closure_mutation_persists_and_activations_do_not_alias() {
        // Behavioural spot-checks decoupled from metering: a counter
        // closure's cell mutates across calls, and two counters built from
        // separate activations of the same factory keep independent cells.
        let one = dual_run(
            "var mk=function(){var c=0; return function(){c=c+1; return c}}; var f=mk(); f(); f()",
        )
        .expect("oracle");
        assert_eq!(one.endor_result, "2", "the shared cell mutates across calls");
        assert_eq!(one.oracle_result, "2");

        let two = dual_run(
            "var mk=function(){var n=0; return function(){return n=n+1}}; var a=mk(),b=mk(); a(); a(); b()",
        )
        .expect("oracle");
        assert_eq!(two.endor_result, "1", "b's cell is independent of a's");
        assert_eq!(two.oracle_result, "1");
    }

    #[test]
    fn stage3_language_corpus_is_bit_exact_against_oracle() {
        // The stage-3 child-1 acceptance bar: every language program —
        // string literals/concatenation/comparison, `typeof`,
        // increment/decrement/exponentiation, `this`, `let`/`const`
        // closures, and `??`/`?.` chaining — agrees with C-XS on BOTH the
        // completion value AND the computron count. Strings are chunk-backed
        // CESU-8 values metered at XS's `fxNewChunk`/`fxConcatString` sites;
        // the numeric and chaining opcodes are dispatch-metered; the closure
        // reset/refresh cells meter their `fxNewSlot`.
        let programs = stage3_language_corpus();
        assert!(!programs.is_empty(), "stage-3 language corpus must be non-empty");
        let (runs, summary) = run_corpus(&programs);
        for r in &runs {
            if !r.is_bit_exact() {
                eprintln!(
                    "DIVERGENCE {:?}\n  agreement={:?} result oracle={:?} endor={:?}\n  computrons oracle={} endor={} (endor dispatched={}) raw oracle={} endor={}\n  endor halt={:?}\n  bytecode={:02x?}",
                    r.source, r.agreement, r.oracle_result, r.endor_result,
                    r.oracle_computrons, r.endor_computrons, r.endor_dispatched,
                    r.oracle_meter_raw, r.endor_meter_raw,
                    r.endor_halt, r.bytecode,
                );
            }
        }
        assert!(
            summary.met_bar(),
            "stage-3 language bit-exact bar: {}/{} (result_div={}, computron_div={}, completion_div={}, unsupported={})",
            summary.bit_exact, summary.total, summary.result_divergences,
            summary.computron_divergences, summary.completion_divergences, summary.unsupported,
        );
    }

    #[test]
    fn stage3_fundamentals_corpus_is_bit_exact_against_oracle() {
        // The stage-3 child-2 acceptance bar: every fundamentals program —
        // the intrinsic constructors as first-class values, `typeof` over
        // them, and `Boolean` primitive coercion — agrees with C-XS on BOTH
        // the completion value AND the computron count. The constructors
        // relink from the program's symbol table to endor's intrinsics; the
        // bare reference renders through Function.prototype.toString's
        // host-function form; the `Boolean` native call is metering-neutral.
        let programs = stage3_fundamentals_corpus();
        assert!(!programs.is_empty(), "stage-3 fundamentals corpus must be non-empty");
        let (runs, summary) = run_corpus(&programs);
        for r in &runs {
            if !r.is_bit_exact() {
                eprintln!(
                    "DIVERGENCE {:?}\n  agreement={:?} result oracle={:?} endor={:?}\n  computrons oracle={} endor={} (endor dispatched={}) raw oracle={} endor={}\n  endor halt={:?}\n  bytecode={:02x?}",
                    r.source, r.agreement, r.oracle_result, r.endor_result,
                    r.oracle_computrons, r.endor_computrons, r.endor_dispatched,
                    r.oracle_meter_raw, r.endor_meter_raw,
                    r.endor_halt, r.bytecode,
                );
            }
        }
        assert!(
            summary.met_bar(),
            "stage-3 fundamentals bit-exact bar: {}/{} (result_div={}, computron_div={}, completion_div={}, unsupported={})",
            summary.bit_exact, summary.total, summary.result_divergences,
            summary.computron_divergences, summary.completion_divergences, summary.unsupported,
        );
    }

    #[test]
    fn stage3_arrays_corpus_is_bit_exact_against_oracle() {
        // The stage-3 child-3 acceptance bar: every arrays program — array
        // literals (with holes), the item chunk's index/length semantics,
        // computed element get/set, and length grow/shrink — agrees with C-XS
        // on BOTH the completion value AND the computron count. The array
        // instance is a real arena object; item-chunk growth meters the
        // faithful `fxNewChunk` sizes and `NEW_PROPERTY_AT`'s built-in step.
        let programs = stage3_arrays_corpus();
        assert!(!programs.is_empty(), "stage-3 arrays corpus must be non-empty");
        let (runs, summary) = run_corpus(&programs);
        for r in &runs {
            if !r.is_bit_exact() {
                eprintln!(
                    "DIVERGENCE {:?}\n  agreement={:?} result oracle={:?} endor={:?}\n  computrons oracle={} endor={} (endor dispatched={}) raw oracle={} endor={}\n  endor halt={:?}\n  bytecode={:02x?}",
                    r.source, r.agreement, r.oracle_result, r.endor_result,
                    r.oracle_computrons, r.endor_computrons, r.endor_dispatched,
                    r.oracle_meter_raw, r.endor_meter_raw,
                    r.endor_halt, r.bytecode,
                );
            }
        }
        assert!(
            summary.met_bar(),
            "stage-3 arrays bit-exact bar: {}/{} (result_div={}, computron_div={}, completion_div={}, unsupported={})",
            summary.bit_exact, summary.total, summary.result_divergences,
            summary.computron_divergences, summary.completion_divergences, summary.unsupported,
        );
    }

    #[test]
    fn stage3_math_corpus_is_bit_exact_against_oracle() {
        // The stage-3 child-4 Math acceptance bar: every Math program — the
        // namespace object, the numeric constants, and every modeled static
        // (including the NaN-canonicalization and ±0 determinism corners the
        // design flags consensus-critical) — agrees with C-XS on BOTH the
        // completion value AND the computron count.
        let programs = stage3_math_corpus();
        assert!(!programs.is_empty(), "stage-3 math corpus must be non-empty");
        let (runs, summary) = run_corpus(&programs);
        for r in &runs {
            if !r.is_bit_exact() {
                eprintln!(
                    "DIVERGENCE {:?}\n  agreement={:?} result oracle={:?} endor={:?}\n  computrons oracle={} endor={} (endor dispatched={}) raw oracle={} endor={}\n  endor halt={:?}\n  bytecode={:02x?}",
                    r.source, r.agreement, r.oracle_result, r.endor_result,
                    r.oracle_computrons, r.endor_computrons, r.endor_dispatched,
                    r.oracle_meter_raw, r.endor_meter_raw,
                    r.endor_halt, r.bytecode,
                );
            }
        }
        assert!(
            summary.met_bar(),
            "stage-3 math bit-exact bar: {}/{} (result_div={}, computron_div={}, completion_div={}, unsupported={})",
            summary.bit_exact, summary.total, summary.result_divergences,
            summary.computron_divergences, summary.completion_divergences, summary.unsupported,
        );
    }

    #[test]
    fn stage3_json_corpus_is_bit_exact_against_oracle() {
        // The stage-3 child-4 JSON acceptance bar: JSON.stringify over a
        // top-level primitive (escaper + metering residuals) agrees with C-XS
        // on BOTH the completion value AND the computron count.
        let programs = stage3_json_corpus();
        assert!(!programs.is_empty(), "stage-3 json corpus must be non-empty");
        let (runs, summary) = run_corpus(&programs);
        for r in &runs {
            if !r.is_bit_exact() {
                eprintln!(
                    "DIVERGENCE {:?}\n  agreement={:?} result oracle={:?} endor={:?}\n  computrons oracle={} endor={} (endor dispatched={}) raw oracle={} endor={}\n  endor halt={:?}\n  bytecode={:02x?}",
                    r.source, r.agreement, r.oracle_result, r.endor_result,
                    r.oracle_computrons, r.endor_computrons, r.endor_dispatched,
                    r.oracle_meter_raw, r.endor_meter_raw,
                    r.endor_halt, r.bytecode,
                );
            }
        }
        assert!(
            summary.met_bar(),
            "stage-3 json bit-exact bar: {}/{} (result_div={}, computron_div={}, completion_div={}, unsupported={})",
            summary.bit_exact, summary.total, summary.result_divergences,
            summary.computron_divergences, summary.completion_divergences, summary.unsupported,
        );
    }

    #[test]
    fn stage3b_json_metering_corpus_is_bit_exact_against_oracle() {
        // The stage-3b json-metering acceptance bar: structured JSON.stringify
        // over objects and arrays (flat, nested, holes/undefined, string
        // escapes) agrees with C-XS on BOTH the serialized value AND the
        // computron count — the recursive per-node metering reproduces the
        // pin's `fxStringifyJSONProperty` allocation walk exactly.
        let programs = stage3b_json_metering_corpus();
        assert!(!programs.is_empty(), "stage-3b json-metering corpus must be non-empty");
        let (runs, summary) = run_corpus(&programs);
        for r in &runs {
            if !r.is_bit_exact() {
                eprintln!(
                    "DIVERGENCE {:?}\n  agreement={:?} result oracle={:?} endor={:?}\n  computrons oracle={} endor={} (endor dispatched={}) raw oracle={} endor={}\n  endor halt={:?}\n  bytecode={:02x?}",
                    r.source, r.agreement, r.oracle_result, r.endor_result,
                    r.oracle_computrons, r.endor_computrons, r.endor_dispatched,
                    r.oracle_meter_raw, r.endor_meter_raw,
                    r.endor_halt, r.bytecode,
                );
            }
        }
        assert!(
            summary.met_bar(),
            "stage-3b json-metering bit-exact bar: {}/{} (result_div={}, computron_div={}, completion_div={}, unsupported={})",
            summary.bit_exact, summary.total, summary.result_divergences,
            summary.computron_divergences, summary.completion_divergences, summary.unsupported,
        );
    }

    #[test]
    fn stage3_collections_corpus_is_bit_exact_against_oracle() {
        // The stage-3 child-5 collections acceptance bar: every Map/Set/
        // WeakMap/WeakSet program — construction, set/add/get/has/delete/size,
        // growth over the hash-table rehash boundaries, SameValueZero key
        // equality, and reference keys — agrees with C-XS on BOTH the
        // completion value AND the computron count. Metering is purely
        // allocation-driven (xsMapSet.c calls no `mxMeter`): the entry
        // `fxNewSlot`s, the per-linked-slot residual, and the `fxResizeEntries`
        // rehash chunks are reproduced so the computrons agree bit-exactly.
        let programs = stage3_collections_corpus();
        assert!(!programs.is_empty(), "stage-3 collections corpus must be non-empty");
        let (runs, summary) = run_corpus(&programs);
        for r in &runs {
            if !r.is_bit_exact() {
                eprintln!(
                    "DIVERGENCE {:?}\n  agreement={:?} result oracle={:?} endor={:?}\n  computrons oracle={} endor={} (endor dispatched={}) raw oracle={} endor={}\n  endor halt={:?}\n  bytecode={:02x?}",
                    r.source, r.agreement, r.oracle_result, r.endor_result,
                    r.oracle_computrons, r.endor_computrons, r.endor_dispatched,
                    r.oracle_meter_raw, r.endor_meter_raw,
                    r.endor_halt, r.bytecode,
                );
            }
        }
        assert!(
            summary.met_bar(),
            "stage-3 collections bit-exact bar: {}/{} (result_div={}, computron_div={}, completion_div={}, unsupported={})",
            summary.bit_exact, summary.total, summary.result_divergences,
            summary.computron_divergences, summary.completion_divergences, summary.unsupported,
        );
    }

    #[test]
    fn stage3_bigint_corpus_is_bit_exact_against_oracle() {
        // The stage-3b bigint acceptance bar: every BigInt program — literals,
        // arithmetic (+/-/*), unary minus, strict/loose equality (including
        // BigInt-vs-Number), relational order, typeof, and decimal rendering —
        // agrees with C-XS on BOTH the completion value AND the computron
        // count. The arithmetic digit step is `mxBigInt_meter`-driven over the
        // trimmed result size; the result chunk is metered at XS's pre-trim
        // `fxBigInt_alloc` size (add max+1, sub max, mul a+b limbs), so the
        // computrons agree bit-exactly.
        let programs = stage3_bigint_corpus();
        assert!(!programs.is_empty(), "stage-3b bigint corpus must be non-empty");
        let (runs, summary) = run_corpus(&programs);
        for r in &runs {
            if !r.is_bit_exact() {
                eprintln!(
                    "DIVERGENCE {:?}\n  agreement={:?} result oracle={:?} endor={:?}\n  computrons oracle={} endor={} (endor dispatched={}) raw oracle={} endor={}\n  endor halt={:?}\n  bytecode={:02x?}",
                    r.source, r.agreement, r.oracle_result, r.endor_result,
                    r.oracle_computrons, r.endor_computrons, r.endor_dispatched,
                    r.oracle_meter_raw, r.endor_meter_raw,
                    r.endor_halt, r.bytecode,
                );
            }
        }
        assert!(
            summary.met_bar(),
            "stage-3b bigint bit-exact bar: {}/{} (result_div={}, computron_div={}, completion_div={}, unsupported={})",
            summary.bit_exact, summary.total, summary.result_divergences,
            summary.computron_divergences, summary.completion_divergences, summary.unsupported,
        );
    }

    #[test]
    fn stage3b_binary_corpus_is_bit_exact_against_oracle() {
        // The stage-3b binary-data acceptance bar (child 3/9): every
        // ArrayBuffer program — construct over a byteLength, the byteLength
        // accessor getter, the zero-fill, and buffers as first-class objects
        // — agrees with C-XS on BOTH the completion value AND the computron
        // count. The construct cost is a constant native frame
        // (`ARRAY_BUFFER_CTOR_FRAME_METERING`) plus the 8-byte-aligned
        // `fxNewChunk(byteLength)` backing store; the getter meters nothing
        // beyond its dispatch.
        let programs = stage3b_binary_corpus();
        assert!(!programs.is_empty(), "stage-3b binary corpus must be non-empty");
        let (runs, summary) = run_corpus(&programs);
        for r in &runs {
            if !r.is_bit_exact() {
                eprintln!(
                    "DIVERGENCE {:?}\n  agreement={:?} result oracle={:?} endor={:?}\n  computrons oracle={} endor={} (endor dispatched={}) raw oracle={} endor={}\n  endor halt={:?}\n  bytecode={:02x?}",
                    r.source, r.agreement, r.oracle_result, r.endor_result,
                    r.oracle_computrons, r.endor_computrons, r.endor_dispatched,
                    r.oracle_meter_raw, r.endor_meter_raw,
                    r.endor_halt, r.bytecode,
                );
            }
        }
        assert!(
            summary.met_bar(),
            "stage-3b binary bit-exact bar: {}/{} (result_div={}, computron_div={}, completion_div={}, unsupported={})",
            summary.bit_exact, summary.total, summary.result_divergences,
            summary.computron_divergences, summary.completion_divergences, summary.unsupported,
        );
    }

    #[test]
    fn stage3b_fundamentals_followup_corpus_is_bit_exact_against_oracle() {
        // The stage-3b fundamentals-followup acceptance bar (child 4/9):
        // every program reading a user function's `.length` (declared arity)
        // or `.name` (own name) agrees with C-XS on BOTH the completion value
        // AND the computron count. These are own data properties allocated at
        // definition (folded into `FUNCTION_DEFINE_METERING`), so reading them
        // meters nothing beyond the GET_PROPERTY dispatch.
        let programs = stage3b_fundamentals_followup_corpus();
        assert!(
            !programs.is_empty(),
            "stage-3b fundamentals-followup corpus must be non-empty"
        );
        let (runs, summary) = run_corpus(&programs);
        for r in &runs {
            if !r.is_bit_exact() {
                eprintln!(
                    "DIVERGENCE {:?}\n  agreement={:?} result oracle={:?} endor={:?}\n  computrons oracle={} endor={} (endor dispatched={}) raw oracle={} endor={}\n  endor halt={:?}\n  bytecode={:02x?}",
                    r.source, r.agreement, r.oracle_result, r.endor_result,
                    r.oracle_computrons, r.endor_computrons, r.endor_dispatched,
                    r.oracle_meter_raw, r.endor_meter_raw,
                    r.endor_halt, r.bytecode,
                );
            }
        }
        assert!(
            summary.met_bar(),
            "stage-3b fundamentals-followup bit-exact bar: {}/{} (result_div={}, computron_div={}, completion_div={}, unsupported={})",
            summary.bit_exact, summary.total, summary.result_divergences,
            summary.computron_divergences, summary.completion_divergences, summary.unsupported,
        );
    }

    #[test]
    fn bound_function_in_call_apply_position_self_names_never_diverges() {
        // The loaded gun (`FuncInfo::default().body_start = 0`): a bound
        // function reshaped through `.call`/`.apply` used to dispatch at pc 0
        // — a SILENT completion divergence (never an abort, so worse than a
        // crash for the never-a-wrong-value invariant). Exactness is not
        // affordable now (the correct trampoline stacks the `.call`/`.apply`
        // re-dispatch onto the bound re-dispatch, two calibrated overheads),
        // so each must self-name `Halt::Unsupported("bind:bound-callback")` —
        // an honest skip, never a wrong value and never a dispatch at pc 0.
        let programs = [
            "var b=function(v){return v;}.bind(null); b.call(null)",
            "var b=function(v){return 7;}.bind(null); b.apply(null,[])",
            "function s(a,b){return a+b} var b=s.bind(null,1); b.call(null,2)",
            "function s(a,b){return a+b} var b=s.bind(null,1); b.apply(null,[2])",
        ];
        for p in programs {
            let r = dual_run(p).expect("oracle machine available");
            assert!(
                matches!(&r.endor_halt, Halt::Unsupported(name) if *name == "bind:bound-callback"),
                "{p:?}: expected an honest bind:bound-callback skip (no abort, no wrong value), got halt={:?} result_agrees={} computrons_agree={}",
                r.endor_halt, r.result_agrees, r.computrons_agree,
            );
        }
    }

    #[test]
    fn stage3b_object_statics_corpus_is_bit_exact_against_oracle() {
        // The stage-3b object-statics + intern-table acceptance bar (child
        // 5/9): every `hasOwnProperty` over the global string→id intern table
        // agrees with C-XS on BOTH the completion value AND the computron
        // count. A program-symbol / boot-default-key name resolves with no
        // allocation; a genuinely-novel name meters exactly one `fxNewSlot`
        // key slot; the table is persistent so a repeated novel key never
        // re-allocates.
        let programs = stage3b_object_statics_corpus();
        assert!(
            !programs.is_empty(),
            "stage-3b object-statics corpus must be non-empty"
        );
        let (runs, summary) = run_corpus(&programs);
        for r in &runs {
            if !r.is_bit_exact() {
                eprintln!(
                    "DIVERGENCE {:?}\n  agreement={:?} result oracle={:?} endor={:?}\n  computrons oracle={} endor={} (endor dispatched={}) raw oracle={} endor={}\n  endor halt={:?}\n  bytecode={:02x?}",
                    r.source, r.agreement, r.oracle_result, r.endor_result,
                    r.oracle_computrons, r.endor_computrons, r.endor_dispatched,
                    r.oracle_meter_raw, r.endor_meter_raw,
                    r.endor_halt, r.bytecode,
                );
            }
        }
        assert!(
            summary.met_bar(),
            "stage-3b object-statics bit-exact bar: {}/{} (result_div={}, computron_div={}, completion_div={}, unsupported={})",
            summary.bit_exact, summary.total, summary.result_divergences,
            summary.computron_divergences, summary.completion_divergences, summary.unsupported,
        );
    }

    #[test]
    fn stage4_object_integrity_corpus_is_bit_exact_against_oracle() {
        // The stage-4 object-integrity acceptance bar (child 1/8): the
        // integrity levels (`preventExtensions`/`seal`/`freeze` +
        // `isExtensible`/`isSealed`/`isFrozen`) with the slot-arena flag
        // semantics XS implements — the instance `XS_DONT_PATCH_FLAG` and the
        // per-property `XS_DONT_DELETE_FLAG`/`XS_DONT_SET_FLAG` stamps — plus
        // the sloppy-mode write/delete rejection those flags impose, and the
        // descriptor-reflection surface (`values`/`entries`/
        // `getOwnPropertyDescriptors`/`propertyIsEnumerable`), all agree with
        // C-XS on BOTH the completion value AND the computron count. The
        // strict-mode integrity-violation *throw* (a catchable native
        // `TypeError`) is an honest named skip, excluded from this corpus.
        let programs = stage4_object_integrity_corpus();
        assert!(
            !programs.is_empty(),
            "stage-4 object-integrity corpus must be non-empty"
        );
        let (runs, summary) = run_corpus(&programs);
        for r in &runs {
            if !r.is_bit_exact() {
                eprintln!(
                    "DIVERGENCE {:?}\n  agreement={:?} result oracle={:?} endor={:?}\n  computrons oracle={} endor={} (endor dispatched={}) raw oracle={} endor={}\n  endor halt={:?}\n  bytecode={:02x?}",
                    r.source, r.agreement, r.oracle_result, r.endor_result,
                    r.oracle_computrons, r.endor_computrons, r.endor_dispatched,
                    r.oracle_meter_raw, r.endor_meter_raw,
                    r.endor_halt, r.bytecode,
                );
            }
        }
        assert!(
            summary.met_bar(),
            "stage-4 object-integrity bit-exact bar: {}/{} (result_div={}, computron_div={}, completion_div={}, unsupported={})",
            summary.bit_exact, summary.total, summary.result_divergences,
            summary.computron_divergences, summary.completion_divergences, summary.unsupported,
        );
    }

    #[test]
    fn stage4_new_target_corpus_is_bit_exact_against_oracle() {
        // The stage-4 `new.target` acceptance bar (child 2/8, the landed
        // slice): the `XS_CODE_TARGET` opcode reads the running frame's target
        // constructor inside a construct (`new f()`) and `undefined` inside a
        // plain call, across the factory-guard idiom and a closure-captured
        // constructor — all agreeing with C-XS on BOTH the completion value AND
        // the computron count. The opcode is pure dispatch (XS only allocs a
        // stack slot and advances), so the generic per-opcode `tick_code` is
        // the whole cost. The wider `class` family (definition/methods/extends/
        // super/private/static) is a reported scope fold that self-names honest
        // skips and is excluded from this corpus.
        let programs = stage4_new_target_corpus();
        assert!(
            !programs.is_empty(),
            "stage-4 new.target corpus must be non-empty"
        );
        let (runs, summary) = run_corpus(&programs);
        for r in &runs {
            if !r.is_bit_exact() {
                eprintln!(
                    "DIVERGENCE {:?}\n  agreement={:?} result oracle={:?} endor={:?}\n  computrons oracle={} endor={} (endor dispatched={}) raw oracle={} endor={}\n  endor halt={:?}\n  bytecode={:02x?}",
                    r.source, r.agreement, r.oracle_result, r.endor_result,
                    r.oracle_computrons, r.endor_computrons, r.endor_dispatched,
                    r.oracle_meter_raw, r.endor_meter_raw,
                    r.endor_halt, r.bytecode,
                );
            }
        }
        assert!(
            summary.met_bar(),
            "stage-4 new.target bit-exact bar: {}/{} (result_div={}, computron_div={}, completion_div={}, unsupported={})",
            summary.bit_exact, summary.total, summary.result_divergences,
            summary.computron_divergences, summary.completion_divergences, summary.unsupported,
        );
    }

    #[test]
    fn stage4_generators_corpus_is_bit_exact_against_oracle() {
        // The stage-4 generators acceptance bar (child 3/8): generator
        // functions and the iteration protocol — `function*` decl/expr and
        // object-literal `*m()`, the suspend/resume of the interpreter
        // activation (`START_GENERATOR` snapshots the fresh frame and returns
        // the instance; `YIELD` snapshots and unwinds to the `.next` driver via
        // `Halt::Yield`; `.next(v)` reinstalls the frame and runs a nested
        // dispatch through the `BRANCH_STATUS` resume epilogue to the next
        // `yield` or `END`), the sent value, completion `{value, done}`, and
        // `for-of`/spread over a generator — all agreeing with C-XS on BOTH the
        // completion value AND the computron count. Metering is allocation-
        // driven (calibrated frozen constants over the identical bytecode).
        // `yield*` delegation, `throw`/`return` into a suspended body, `yield`
        // inside `try`, `new`-constructed generators, and async generators are
        // the reported scope fold — honest named skips excluded from the corpus.
        let programs = stage4_generators_corpus();
        assert!(
            !programs.is_empty(),
            "stage-4 generators corpus must be non-empty"
        );
        let (runs, summary) = run_corpus(&programs);
        for r in &runs {
            if !r.is_bit_exact() {
                eprintln!(
                    "DIVERGENCE {:?}\n  agreement={:?} result oracle={:?} endor={:?}\n  computrons oracle={} endor={} (endor dispatched={}) raw oracle={} endor={}\n  endor halt={:?}\n  bytecode={:02x?}",
                    r.source, r.agreement, r.oracle_result, r.endor_result,
                    r.oracle_computrons, r.endor_computrons, r.endor_dispatched,
                    r.oracle_meter_raw, r.endor_meter_raw,
                    r.endor_halt, r.bytecode,
                );
            }
        }
        assert!(
            summary.met_bar(),
            "stage-4 generators bit-exact bar: {}/{} (result_div={}, computron_div={}, completion_div={}, unsupported={})",
            summary.bit_exact, summary.total, summary.result_divergences,
            summary.computron_divergences, summary.completion_divergences, summary.unsupported,
        );
    }

    #[test]
    fn stage3b_promises_corpus_is_bit_exact_against_oracle() {
        // The stage-3b promises acceptance bar (child 7/9): Promise
        // construction + the executor, resolve/reject settling, the statics,
        // then/catch reaction registration, and the microtask job queue drained
        // by the pump-loop latch — all agree with C-XS on BOTH the completion
        // value AND the computron count, INCLUDING the reactions run at the
        // drain (the consensus-relevant scheduling). Thenable adoption, a
        // throwing/reference-returning handler, `.finally`, the combinators, and
        // async/await are honest named skips, excluded from the covered corpus.
        let programs = stage3b_promises_corpus();
        assert!(
            !programs.is_empty(),
            "stage-3b promises corpus must be non-empty"
        );
        let (runs, summary) = run_corpus(&programs);
        for r in &runs {
            if !r.is_bit_exact() {
                eprintln!(
                    "DIVERGENCE {:?}\n  agreement={:?} result oracle={:?} endor={:?}\n  computrons oracle={} endor={} (endor dispatched={}) raw oracle={} endor={}\n  endor halt={:?}\n  bytecode={:02x?}",
                    r.source, r.agreement, r.oracle_result, r.endor_result,
                    r.oracle_computrons, r.endor_computrons, r.endor_dispatched,
                    r.oracle_meter_raw, r.endor_meter_raw,
                    r.endor_halt, r.bytecode,
                );
            }
        }
        assert!(
            summary.met_bar(),
            "stage-3b promises bit-exact bar: {}/{} (result_div={}, computron_div={}, completion_div={}, unsupported={})",
            summary.bit_exact, summary.total, summary.result_divergences,
            summary.computron_divergences, summary.completion_divergences, summary.unsupported,
        );
    }

    #[test]
    fn stage4_async_promises_corpus_is_bit_exact_against_oracle() {
        // The stage-4 async/promise keystone acceptance bar (child 4/8): the
        // promise native-handler double-settle calibration and the surfaces it
        // unblocks — thenable adoption, the two-level `[[AlreadyResolved]]`
        // guard's double-settle no-op, long `then`-chains, a handler returning a
        // thenable/native-promise, and the `Promise.resolve(nativePromise)`
        // identity — all agreeing with C-XS on BOTH the completion value AND the
        // computron count, INCLUDING the reactions and thenable jobs run at the
        // pump-loop drain. A throwing handler/`then`, `resolve(promise-itself)`,
        // and the async-function surface are honest named skips, excluded.
        let programs = stage4_async_promises_corpus();
        assert!(
            !programs.is_empty(),
            "stage-4 async/promise corpus must be non-empty"
        );
        let (runs, summary) = run_corpus(&programs);
        for r in &runs {
            if !r.is_bit_exact() {
                eprintln!(
                    "DIVERGENCE {:?}\n  agreement={:?} result oracle={:?} endor={:?}\n  computrons oracle={} endor={} (endor dispatched={}) raw oracle={} endor={}\n  endor halt={:?}\n  bytecode={:02x?}",
                    r.source, r.agreement, r.oracle_result, r.endor_result,
                    r.oracle_computrons, r.endor_computrons, r.endor_dispatched,
                    r.oracle_meter_raw, r.endor_meter_raw,
                    r.endor_halt, r.bytecode,
                );
            }
        }
        assert!(
            summary.met_bar(),
            "stage-4 async/promise bit-exact bar: {}/{} (result_div={}, computron_div={}, completion_div={}, unsupported={})",
            summary.bit_exact, summary.total, summary.result_divergences,
            summary.computron_divergences, summary.completion_divergences, summary.unsupported,
        );
    }

    #[test]
    fn stage3b_regexp_corpus_is_bit_exact_against_oracle() {
        // The stage-3b xsre-integration acceptance bar (child 9/9): the
        // JavaScript RegExp surface over child 8's matcher — construction (the
        // literal + constructor forms), the source/flags/per-flag accessor
        // getters, exec/test (match, no-match, captures, the stateful g/y
        // drive) and toString — all agree with C-XS on BOTH the completion
        // value AND the computron count. A RegExp-valued pattern arg, named
        // groups, a syntax-error/unsupported pattern feature, and a non-ASCII
        // stateful subject are honest named skips, excluded from the corpus.
        let programs = stage3b_regexp_corpus();
        assert!(
            !programs.is_empty(),
            "stage-3b regexp corpus must be non-empty"
        );
        let (runs, summary) = run_corpus(&programs);
        for r in &runs {
            if !r.is_bit_exact() {
                eprintln!(
                    "DIVERGENCE {:?}\n  agreement={:?} result oracle={:?} endor={:?}\n  computrons oracle={} endor={} (endor dispatched={}) raw oracle={} endor={}\n  endor halt={:?}\n  bytecode={:02x?}",
                    r.source, r.agreement, r.oracle_result, r.endor_result,
                    r.oracle_computrons, r.endor_computrons, r.endor_dispatched,
                    r.oracle_meter_raw, r.endor_meter_raw,
                    r.endor_halt, r.bytecode,
                );
            }
        }
        assert!(
            summary.met_bar(),
            "stage-3b regexp bit-exact bar: {}/{} (result_div={}, computron_div={}, completion_div={}, unsupported={})",
            summary.bit_exact, summary.total, summary.result_divergences,
            summary.computron_divergences, summary.completion_divergences, summary.unsupported,
        );
    }

    #[test]
    fn stage3_number_corpus_is_bit_exact_against_oracle() {
        // The stage-3 child-4 Number acceptance bar: the statics/predicates,
        // Number.prototype.toString (radix 10), Number(...) coercion, and the
        // numeric globals — all agree with C-XS on BOTH the completion value
        // AND the computron count.
        let programs = stage3_number_corpus();
        assert!(!programs.is_empty(), "stage-3 number corpus must be non-empty");
        let (runs, summary) = run_corpus(&programs);
        for r in &runs {
            if !r.is_bit_exact() {
                eprintln!(
                    "DIVERGENCE {:?}\n  agreement={:?} result oracle={:?} endor={:?}\n  computrons oracle={} endor={} (endor dispatched={}) raw oracle={} endor={}\n  endor halt={:?}\n  bytecode={:02x?}",
                    r.source, r.agreement, r.oracle_result, r.endor_result,
                    r.oracle_computrons, r.endor_computrons, r.endor_dispatched,
                    r.oracle_meter_raw, r.endor_meter_raw,
                    r.endor_halt, r.bytecode,
                );
            }
        }
        assert!(
            summary.met_bar(),
            "stage-3 number bit-exact bar: {}/{} (result_div={}, computron_div={}, completion_div={}, unsupported={})",
            summary.bit_exact, summary.total, summary.result_divergences,
            summary.computron_divergences, summary.completion_divergences, summary.unsupported,
        );
    }

    #[test]
    fn stage3_string_corpus_is_bit_exact_against_oracle() {
        // The stage-3 child-4 String acceptance bar: every String.prototype
        // program — primitive property/method access over the CESU-8 chunk,
        // the slice/case/search families, and string building in loops (the
        // metering hot path) — agrees with C-XS on BOTH the completion value
        // AND the computron count.
        let programs = stage3_string_corpus();
        assert!(!programs.is_empty(), "stage-3 string corpus must be non-empty");
        let (runs, summary) = run_corpus(&programs);
        for r in &runs {
            if !r.is_bit_exact() {
                eprintln!(
                    "DIVERGENCE {:?}\n  agreement={:?} result oracle={:?} endor={:?}\n  computrons oracle={} endor={} (endor dispatched={}) raw oracle={} endor={}\n  endor halt={:?}\n  bytecode={:02x?}",
                    r.source, r.agreement, r.oracle_result, r.endor_result,
                    r.oracle_computrons, r.endor_computrons, r.endor_dispatched,
                    r.oracle_meter_raw, r.endor_meter_raw,
                    r.endor_halt, r.bytecode,
                );
            }
        }
        assert!(
            summary.met_bar(),
            "stage-3 string bit-exact bar: {}/{} (result_div={}, computron_div={}, completion_div={}, unsupported={})",
            summary.bit_exact, summary.total, summary.result_divergences,
            summary.computron_divergences, summary.completion_divergences, summary.unsupported,
        );
    }

    #[test]
    fn stage3_string_utf16_result_parity_and_determinism() {
        // The governing check for the UTF-16 storage swap: every surrogate-
        // pair / index-heavy / lone-surrogate fixture agrees with C-XS on the
        // completion VALUE (divergent=0 on RESULTS). Each fixture completes
        // with a scalar, so the pin's result transports faithfully.
        //
        // Cross-engine computron equality is NOT asserted (the recalibration
        // re-bases string cost off code-unit length, so multi-unit cases shift
        // vs the pin's CESU-8 byte metering). The property that MUST hold —
        // determinism-per-release — is asserted directly: endor runs the same
        // bytecode twice and must return identical computrons AND result.
        let programs = stage3_string_utf16_corpus();
        assert!(!programs.is_empty(), "the UTF-16 fixture corpus must be non-empty");
        let mut result_divergences = Vec::new();
        let mut shifted = 0usize; // fixtures whose computrons differ from the pin
        for p in &programs {
            let oracle = match endor_oracle::run(p) {
                Some(o) => o,
                None => panic!("oracle machine failed to start for {p:?}"),
            };
            let a = run_program_with_symbols(&oracle.bytecode, &oracle.symbols);
            let b = run_program_with_symbols(&oracle.bytecode, &oracle.symbols);
            // Determinism-per-release: identical across repeated runs.
            assert_eq!(a.computrons, b.computrons, "endor computrons deterministic for {p:?}");
            assert_eq!(a.result, b.result, "endor result deterministic for {p:?}");
            assert!(a.completed, "endor completes the fixture {p:?} (halt={:?})", a.halt);
            assert!(oracle.completed, "the pin completes the fixture {p:?}");
            // RESULT parity — the governing check.
            if oracle.result != a.result {
                result_divergences.push(format!(
                    "{p:?}: oracle={:?} endor={:?}",
                    oracle.result, a.result
                ));
            }
            if oracle.computrons != a.computrons {
                shifted += 1;
            }
        }
        assert!(
            result_divergences.is_empty(),
            "RESULT divergence(s) on the UTF-16 fixtures (must be zero):\n  {}",
            result_divergences.join("\n  "),
        );
        // The recalibration must be LIVE: at least some multi-unit fixture
        // meters differently from the pin's CESU-8 byte cost (else the storage
        // swap would not have re-based anything). This guards against a silent
        // back-fit to the oracle's byte length.
        assert!(
            shifted > 0,
            "expected the UTF-16 recalibration to shift computrons on some \
             multi-unit fixture vs the CESU-8 pin; none shifted"
        );
    }

    #[test]
    fn utf16_meter_expectations_are_the_frozen_recalibrated_costs() {
        // The frozen recalibrated UTF-16 computron costs (the build's re-based
        // string metering), asserted against endor DIRECTLY — NOT back-fitted
        // to the pin's CESU-8 byte length nor to the oracle. This locks the
        // per-release determinism of the meter: these numbers are endor's own
        // UTF-16 cost and must not drift silently. Where a value differs from
        // the pin it is noted; the pin equality is neither required nor checked
        // here. If a legitimate metering change moves one, update it here
        // deliberately (that is the point of a frozen expectation).
        let cases: &[(&str, u64)] = &[
            // scalar reads that meter the same as CESU-8 for this content
            (r#""𝒜".length"#, 9),
            (r#""a𝒜b".codePointAt(1)"#, 13),
            (r#"[..."a𝒜b"].length"#, 93),
            // multi-unit cases whose cost is re-based off code-unit length
            // (these differ from the pin — the recalibration witnesses):
            (
                r#"var s0 = "a𝒜b"; var t0 = 0; for (var i = 0; i < s0.length; i++) { t0 += s0.charCodeAt(i); } t0"#,
                159,
            ),
            (
                r#"var s="";for(var i=0;i<3;i++){s=s.concat("𝒜")};s.length"#,
                105,
            ),
            (r#""a𝒜b".slice(1, 2).charCodeAt(0)"#, 19),
        ];
        for (src, expected) in cases {
            let oracle = endor_oracle::run(src).expect("oracle compiles the program");
            let a = run_program_with_symbols(&oracle.bytecode, &oracle.symbols);
            let b = run_program_with_symbols(&oracle.bytecode, &oracle.symbols);
            assert!(a.completed, "endor completes {src:?} (halt={:?})", a.halt);
            assert_eq!(a.computrons, b.computrons, "determinism-per-release for {src:?}");
            assert_eq!(
                a.computrons, *expected,
                "frozen UTF-16 computron cost for {src:?} (endor's recalibrated value)",
            );
        }
    }

    #[test]
    fn stage1_corpus_is_bit_exact_against_oracle() {
        let programs = stage1_corpus();
        assert!(!programs.is_empty(), "corpus must be non-empty");
        let (runs, summary) = run_corpus(&programs);
        for r in &runs {
            if !r.is_bit_exact() {
                eprintln!(
                    "DIVERGENCE {:?}\n  agreement={:?} result oracle={:?} endor={:?}\n  computrons oracle={} endor={} (endor dispatched={})\n  endor halt={:?}\n  bytecode={:02x?}",
                    r.source,
                    r.agreement,
                    r.oracle_result,
                    r.endor_result,
                    r.oracle_computrons,
                    r.endor_computrons,
                    r.endor_dispatched,
                    r.endor_halt,
                    r.bytecode,
                );
            }
        }
        assert!(
            summary.met_bar(),
            "stage-1 acceptance bar: {}/{} bit-exact (result divergences={}, computron divergences={}, completion divergences={}, unsupported={})",
            summary.bit_exact,
            summary.total,
            summary.result_divergences,
            summary.computron_divergences,
            summary.completion_divergences,
            summary.unsupported,
        );
    }
}
