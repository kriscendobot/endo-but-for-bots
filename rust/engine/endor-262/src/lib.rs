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

pub mod compile_diff;
pub mod frontmatter;
pub mod test262;
pub mod xst;

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

/// Which compiler produces the bytecode `endor-vm` executes in the
/// dual-run runner — the **pipeline seam** (stage-5 child 7). The oracle
/// (differential C-XS) compiler is the default and stays so until the
/// supervisor accepts stage 5; `Endor` selects the pure-Rust
/// `endor-compile` pipeline so later stages can flip the default with a
/// one-line change and no runner surgery (design § roadmap row 5).
///
/// In either mode the oracle is still consulted for the **reference**
/// result/computrons the run is compared against; the selection only
/// decides whose *bytecode* endor runs — its own, or the oracle's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Compiler {
    /// The differential C-XS oracle compiler (`endor_oracle::run`). The
    /// stage-≤4 default; the exact XS-emitted bytecode.
    #[default]
    Oracle,
    /// The pure-Rust `endor-compile` pipeline (lexer → parser → scoper →
    /// coder). While the stage-5 byte-identity bar holds, its bytes equal
    /// the oracle's, so the oracle's symbols atom pairs with them
    /// unchanged; a compile fold (parser/scoper reject or a coder panic)
    /// yields empty bytecode the runner treats as an endor abort.
    Endor,
}

/// Compile `source` to `(bytecode, symbols)` under the selected compiler,
/// given the oracle outcome already in hand (the reference). The endor
/// path is total over the coder's panics (`catch_unwind`); a fold returns
/// empty bytecode, which `endor-vm` decodes as an abort — the honest
/// "endor could not run its own output here" signal, never a harness
/// panic. The seam later stages flip lives entirely here.
fn compile_for(compiler: Compiler, source: &str, oracle: &endor_oracle::OracleOutcome) -> (Vec<u8>, Vec<u8>) {
    match compiler {
        Compiler::Oracle => (oracle.bytecode.clone(), oracle.symbols.clone()),
        Compiler::Endor => {
            let compiled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                endor_compile::compile(source)
            }));
            match compiled {
                Ok(Ok(bytes)) => (bytes, oracle.symbols.clone()),
                // A structured reject or a coder fold: empty bytecode →
                // endor-vm aborts on decode, mirroring "endor rejected".
                Ok(Err(_)) | Err(_) => (Vec::new(), Vec::new()),
            }
        }
    }
}

/// Run one program on both engines and compare, using the default
/// (oracle) compiler. Returns `None` only if the oracle machine fails to
/// start.
pub fn dual_run(source: &str) -> Option<DualRun> {
    dual_run_with(source, Compiler::default())
}

/// Run one program on both engines and compare, choosing which compiler
/// produces the bytecode endor executes (the pipeline seam). Returns
/// `None` only if the oracle machine itself fails to start.
pub fn dual_run_with(source: &str, compiler: Compiler) -> Option<DualRun> {
    let oracle = endor_oracle::run(source)?;

    // The pipeline seam: the bytecode endor runs comes from the selected
    // compiler. The default (oracle) path is the exact XS-emitted bytes;
    // the endor path is `endor-compile`'s own output.
    let (bytecode, symbols) = compile_for(compiler, source, &oracle);

    // Pass the symbols atom so endor relinks the program's intrinsic
    // references (`Object`, `Boolean`, the Error hierarchy, …) to its own
    // intrinsics by name — the C-XS compiler numbers those symbols
    // program-locally, so the id→name table is what makes `Boolean` mean the
    // native `Boolean` and not an undefined variable (design § fundamentals).
    let endor: RunOutcome = run_program_with_symbols(&bytecode, &symbols);

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
        bytecode,
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

/// The stage-4b lockdown/harden corpus (child 4/5): the Hardened-JavaScript
/// `harden(x)`/`petrify(x)` globals ported from `xsLockdown.c` — the transitive
/// freeze worklist (`harden`) and the single-object freeze (`petrify`). Asserted
/// RESULT-exact against the pin (the oracle shim installs the harden/lockdown/
/// petrify/mutabilities globals xst.c/xstFuzz.c install). `xsLockdown.c` calls
/// no `mxMeter`, so the metering is allocation-driven; computron parity over a
/// transitive harden is structurally unavailable because endor models
/// intrinsics sparsely (the freeze *result* is faithful, the transitive object
/// count is not). `lockdown()` (freezing the shared intrinsics + Date/Math
/// taming + the idempotence throw) and `mutabilities` (the mutable-residue
/// report) are the reported scope fold — a program referencing either
/// self-names an honest `Halt::Unsupported`, excluded from this corpus.
pub fn stage4_harden_corpus() -> Vec<String> {
    parse_corpus(include_str!("../corpora/stage4-harden.js"))
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

/// The stage-4b async-function-surface corpus (child 2/5): the
/// `XS_CODE_ASYNC_FUNCTION`/`START_ASYNC`/`AWAIT` opcode surface over the
/// promise keystone — the async function define, `START_ASYNC`'s result-promise
/// creation, `AWAIT`'s YIELD-shaped suspend, the `AsyncAwait` native-reaction
/// resume at the pump-loop drain, and `await_schedule`'s primitive/general and
/// native-promise fast paths — all bit-exact (result AND computron) against the
/// oracle, INCLUDING the reactions and async resumes run at the drain. `await`
/// inside a live `try` is the designated named skip (`await:await-in-try`);
/// async generators / `for-await-of` stay the scope fold — all excluded.
pub fn stage4_async_await_corpus() -> Vec<String> {
    parse_corpus(include_str!("../corpora/stage4-async-await.js"))
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

/// The stage-4b compartment differential corpus (child 3/5): programs
/// evaluated in TWO compartments over ONE machine's shared intrinsics
/// (`Compartment::evaluate_with_symbols`), certifying RESULT agreement
/// with the oracle (evaluate faithfulness + shared-intrinsics identity +
/// cross-compartment values). Programs that reference the `Compartment`
/// intrinsic itself are the recorded scope fold
/// (`compartment:intrinsic-surface`), excluded from this corpus.
pub fn stage4_compartment_corpus() -> Vec<String> {
    parse_corpus(include_str!("../corpora/stage4-compartment.js"))
}

/// The **committed** daemon boot-bundle sources the endor daemon evaluates
/// during its bootstrap (design `daemon-endor-architecture.md` § Unified
/// runner, steps 6–7; § Embedded JS bundles). Returned as `(label, source)`
/// pairs so the stage-4 boot-bundle bar can dual-run each against the pin.
///
/// **Provenance.** These two files are the checked-in sources embedded via
/// `include_str!` by `rust/endo/xsnap/src/lib.rs` (`POLYFILLS`,
/// `HOST_ALIASES`) — read here verbatim from the same paths, so the bar runs
/// the *actual* bytes the daemon boots, not a copy that could drift. The
/// third boot step — **`ses_boot.js`** (SES `lockdown()` + the HandledPromise
/// shim) — is **not committed**: it is a ~1 MB build artifact the daemon
/// bundler (`rollup` over `@endo/*`) generates into `src/ses_boot.js` before
/// the `include_str!`, absent in a fresh checkout. Bundling the full SES
/// distribution is out of this engine workspace's scope, so `ses_boot.js` is
/// a **named, ledgered boot-bundle gap** (`boot:ses-lockdown-bundle`), not
/// dual-run here. `host_aliases.js` is a self-contained `globalThis` IIFE
/// that aliases only host functions that exist, so with no host powers
/// registered it completes to `undefined` — safe to dual-run in the engine.
pub fn daemon_boot_bundle_sources() -> Vec<(&'static str, String)> {
    // Relative to this file (`rust/engine/endor-262/src/lib.rs`): up three to
    // `rust/`, then into the daemon crate's committed bundle sources.
    const POLYFILLS: &str = include_str!("../../../endo/xsnap/src/polyfills.js");
    const HOST_ALIASES: &str = include_str!("../../../endo/xsnap/src/host_aliases.js");
    vec![
        ("polyfills.js", POLYFILLS.to_string()),
        ("host_aliases.js", HOST_ALIASES.to_string()),
        // The boot prefix as the daemon evaluates it: polyfills, then the
        // host-alias shim, then a trailing sentinel so the completion value
        // is defined.
        (
            "boot-prefix (polyfills → host_aliases)",
            format!("{POLYFILLS}\n{HOST_ALIASES}\ntrue"),
        ),
    ]
}

/// One boot-bundle program's dual-run verdict, bucketed for the stage-4
/// boot-bundle bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootVerdict {
    /// endor ran the bundle end-to-end and agreed with the pin's completion
    /// value (the bar's green terminal state — reached once the ledgered
    /// engine gaps below land).
    Agrees,
    /// endor honestly aborted with a self-named halt before diverging: the
    /// bundle references an engine surface endor does not yet model. Carries
    /// the ledgered gap key. This is the doctrine's honest named skip — never
    /// a wrong value — and the current expected state of the committed
    /// bundle.
    NamedGap(String),
    /// endor produced a WRONG value, or accepted a program the pin rejected:
    /// a real divergence the bar forbids. Carries a human detail string.
    Divergent(String),
}

/// Dual-run one boot-bundle source against the pin and bucket it. The gap key
/// on an honest abort is derived from endor's self-named halt so a new
/// blocker names itself rather than hiding.
pub fn boot_bundle_verdict(source: &str) -> BootVerdict {
    let r = match dual_run(source) {
        Some(r) => r,
        None => return BootVerdict::NamedGap("oracle-machine-error".into()),
    };
    match r.agreement {
        Agreement::BothComplete => {
            if r.result_agrees {
                BootVerdict::Agrees
            } else {
                BootVerdict::Divergent(format!(
                    "endor completed with a WRONG value: oracle={:?} endor={:?}",
                    r.oracle_result, r.endor_result
                ))
            }
        }
        Agreement::BothAbort => {
            // Both threw: a shared abort is not a boot divergence (the pin
            // itself rejects the program), reported as the pin's reason.
            BootVerdict::NamedGap(format!("both-abort:{}", r.oracle_error))
        }
        // endor honestly aborted where the pin completed: the bundle hit an
        // engine surface endor does not model. Name the gap from the halt.
        Agreement::OracleOnlyComplete => BootVerdict::NamedGap(boot_gap_key(&r)),
        // endor completed a program the pin rejected: over-acceptance.
        Agreement::EndorOnlyComplete => BootVerdict::Divergent(format!(
            "endor completed a program the pin rejected: endor={:?} pin aborted={:?}",
            r.endor_result, r.oracle_error
        )),
    }
}

/// Map an honest endor abort on a boot-bundle program to a stable, ledgered
/// gap key (design's staged-roadmap follow-ups). An `Unsupported(op)` halt
/// self-names by opcode; a `Throw` on an unbound global names the missing
/// intrinsic; anything else carries its halt verbatim.
fn boot_gap_key(r: &DualRun) -> String {
    match &r.endor_halt {
        Halt::Unsupported(op) => format!("boot:unsupported:{op}"),
        Halt::Throw(msg) if msg.contains("undefined variable") => {
            // The committed bundle's first statement reads `globalThis`; endor
            // has no live global-object binding, so every bundle stops here.
            "boot:no-globalThis-global-object-binding".to_string()
        }
        Halt::Throw(msg) => format!("boot:throw:{msg}"),
        other => format!("boot:halt:{other:?}"),
    }
}

/// One program's compartment differential record: the oracle result and
/// the result each of two compartments over one shared-intrinsics machine
/// produced when it evaluated the oracle's exact bytecode.
#[derive(Debug, Clone)]
pub struct CompartmentDualRun {
    pub source: String,
    /// The oracle completed normally.
    pub oracle_completed: bool,
    pub oracle_result: String,
    /// Both compartments completed normally.
    pub both_completed: bool,
    /// Compartment A's completion value string.
    pub a_result: String,
    /// Compartment B's completion value string.
    pub b_result: String,
    /// The two compartments referenced the same machine intrinsics graph.
    pub shared_intrinsics: bool,
    /// Compartment A's computrons (same bytecode → same as the oracle's
    /// run-only count for a bit-exact program).
    pub a_computrons: u64,
    pub oracle_computrons: u64,
    pub a_halt: endor_vm::Halt,
}

impl CompartmentDualRun {
    /// RESULT agreement (the compartment acceptance bar): the oracle and
    /// BOTH compartments completed with the same completion value, over
    /// one shared intrinsics graph. A completion mismatch or a
    /// cross-compartment disagreement is a divergence, never a silent
    /// pass.
    pub fn result_agrees(&self) -> bool {
        self.oracle_completed
            && self.both_completed
            && self.shared_intrinsics
            && self.a_result == self.oracle_result
            && self.b_result == self.oracle_result
    }

    /// The same bytecode evaluated in a compartment reproduces the
    /// oracle's run-only computron count (stricter telemetry the branch
    /// runner still gates — the compartment evaluator seeds no globals
    /// here, so it is byte-identical to the top-level realm run).
    pub fn computrons_agree(&self) -> bool {
        self.oracle_completed && self.both_completed && self.a_computrons == self.oracle_computrons
    }
}

/// Compile `source` on the oracle, then evaluate its exact bytecode in
/// two compartments over one machine's shared intrinsics. Returns `None`
/// only if the oracle machine itself fails to start.
pub fn compartment_dual_run(source: &str) -> Option<CompartmentDualRun> {
    use endor_vm::Machine;

    let oracle = endor_oracle::run(source)?;
    let machine = Machine::new();
    let a = machine.new_compartment();
    let b = machine.new_compartment();
    let shared_intrinsics = std::rc::Rc::ptr_eq(a.intrinsics(), b.intrinsics());

    let ra = a.evaluate_with_symbols(&oracle.bytecode, &oracle.symbols);
    let rb = b.evaluate_with_symbols(&oracle.bytecode, &oracle.symbols);

    Some(CompartmentDualRun {
        source: source.to_string(),
        oracle_completed: oracle.completed,
        oracle_result: oracle.result,
        both_completed: ra.completed && rb.completed,
        a_result: ra.result,
        b_result: rb.result,
        shared_intrinsics,
        a_computrons: ra.computrons,
        oracle_computrons: oracle.computrons,
        a_halt: ra.halt,
    })
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

    #[test]
    fn compiler_seam_endor_matches_oracle_on_byte_identical_programs() {
        // The pipeline seam (stage-5 child 7): running the dual-run through
        // the `Endor` compiler must, on programs whose endor bytecode is
        // byte-identical to the oracle's, execute the *same* bytecode and
        // reach the *same* agreement/result as the default `Oracle` path.
        // This proves the seam actually flips compilers and the endor path
        // runs endor's own output — not a no-op that always uses the oracle.
        let programs = ["1 + 2 * 3", "if (1) { 2 } else { 3 }", "(function(a){ return a + 1 })(4)"];
        for src in programs {
            let oracle = dual_run_with(src, Compiler::Oracle).expect("oracle runs");
            let endor = dual_run_with(src, Compiler::Endor).expect("oracle reference runs");
            // The endor path compiled with endor-compile; its bytes must
            // equal the oracle's (the byte-identity bar) for these programs.
            assert_eq!(
                oracle.bytecode, endor.bytecode,
                "seam: endor-compile bytes must match the oracle's for {src:?}"
            );
            assert_eq!(
                oracle.agreement, endor.agreement,
                "seam: same agreement via either compiler for {src:?}"
            );
            assert_eq!(
                oracle.endor_result, endor.endor_result,
                "seam: same endor result via either compiler for {src:?}"
            );
        }
    }

    #[test]
    fn compiler_seam_endor_fold_is_a_clean_abort_not_a_panic() {
        // A construct the coder folds on must, through the `Endor` seam,
        // produce empty bytecode that endor-vm treats as an abort — never a
        // harness panic. Private member *reads/writes* and the `#x in o`
        // brand check now code byte-identically (this child), so this uses a
        // still-deferred class-tail construct: a `static { … }` block with
        // its own lexical declarations, whose field-function frame
        // reservation is the remaining fold.
        let src = "class C { static { let x = 1; } } new C()";
        let endor = dual_run_with(src, Compiler::Endor).expect("oracle reference runs");
        assert!(
            endor.bytecode.is_empty(),
            "an endor coder fold must yield empty bytecode via the seam"
        );
        assert_ne!(
            endor.agreement,
            Agreement::BothComplete,
            "an empty-bytecode endor run must not spuriously complete like the oracle"
        );
    }

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
    fn stage4_harden_corpus_agrees_on_results_against_oracle() {
        // The stage-4b lockdown/harden acceptance bar (child 4/5): the
        // Hardened-JavaScript `harden(x)`/`petrify(x)` globals from
        // `xsLockdown.c`. `harden` is the transitive freeze worklist (prevent
        // extensions + stamp every own data property non-writable/
        // non-configurable, then queue the prototype and every reference-valued
        // property, marking each reached instance `XS_DONT_MARSHALL_FLAG`);
        // `petrify` is the single-object freeze. Asserted RESULT-exact against
        // the pin: every program completes on BOTH engines to the same value
        // (freeze semantics — a sloppy write to a frozen property is a no-op, a
        // hardened object is `Object.isFrozen`, harden is transitive/idempotent
        // and returns its argument, a non-reference passes through, petrify is
        // non-transitive). Computron parity is NOT asserted: `xsLockdown.c`
        // calls no `mxMeter`, so harden's cost is allocation-driven, and a
        // transitive walk spills into endor's sparsely-modeled intrinsics, whose
        // object count diverges from the pin's full intrinsic graph — the same
        // structural sparse-intrinsics fact the module/compartment children
        // record. `lockdown()`/`mutabilities` are the reported scope fold
        // (honest `Halt::Unsupported`), excluded from this corpus.
        let programs = stage4_harden_corpus();
        assert!(
            !programs.is_empty(),
            "stage-4b harden corpus must be non-empty"
        );
        let mut checked = 0usize;
        for src in &programs {
            let run = dual_run(src).expect("oracle machine must start");
            assert!(
                matches!(run.agreement, Agreement::BothComplete),
                "harden program must complete on both engines: {:?}\n  agreement={:?} endor_halt={:?}",
                src,
                run.agreement,
                run.endor_halt,
            );
            assert!(
                run.result_agrees,
                "harden program result divergence: {:?}\n  oracle={:?} endor={:?}",
                src, run.oracle_result, run.endor_result,
            );
            checked += 1;
        }
        assert_eq!(checked, programs.len());
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
    fn stage4_async_await_corpus_is_bit_exact_against_oracle() {
        // The stage-4b async-function-surface acceptance bar (child 2/5): the
        // `ASYNC_FUNCTION`/`START_ASYNC`/`AWAIT` opcode surface over the promise
        // keystone — plain awaits, the native-promise fast path, nested async,
        // multi-await chains, await-in-loop, async arrows, thenable await, and
        // rejection paths — all agreeing with C-XS on BOTH the completion value
        // AND the computron count, INCLUDING the async resumes and reactions run
        // at the pump-loop drain. `await`-in-`try`, async generators, and
        // `for-await-of` are honest named skips, excluded.
        let programs = stage4_async_await_corpus();
        assert!(
            !programs.is_empty(),
            "stage-4b async-await corpus must be non-empty"
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
            "stage-4b async-await bit-exact bar: {}/{} (result_div={}, computron_div={}, completion_div={}, unsupported={})",
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
    fn stage4_compartment_corpus_agrees_across_two_compartments() {
        // The stage-4b compartment acceptance bar (child 3/5): each program
        // is compiled once on the oracle and evaluated in TWO compartments
        // over ONE machine's shared intrinsics
        // (`Compartment::evaluate_with_symbols`). The bar is RESULT agreement
        // (doctrine: the compartment differential certifies results): both
        // compartments must complete with the oracle's completion value,
        // over one shared intrinsics graph — evaluate faithfulness,
        // shared-intrinsics identity, and cross-compartment value agreement.
        // The same bytecode also reproduces the oracle's run-only computrons
        // (no globals seeded here), which we assert too. Programs that name
        // the `Compartment` intrinsic itself are the recorded scope fold
        // (`compartment:intrinsic-surface`), excluded from the corpus.
        let programs = stage4_compartment_corpus();
        assert!(
            !programs.is_empty(),
            "stage-4 compartment corpus must be non-empty"
        );
        let mut total = 0usize;
        let mut agree = 0usize;
        for p in &programs {
            let r = compartment_dual_run(p).expect("oracle machine available");
            total += 1;
            let ok = r.result_agrees() && r.computrons_agree();
            if ok {
                agree += 1;
            } else {
                eprintln!(
                    "COMPARTMENT DIVERGENCE {:?}\n  oracle(completed={} result={:?} computrons={})\n  A(result={:?} computrons={}) B(result={:?}) shared_intrinsics={} both_completed={}\n  A halt={:?}",
                    r.source, r.oracle_completed, r.oracle_result, r.oracle_computrons,
                    r.a_result, r.a_computrons, r.b_result, r.shared_intrinsics,
                    r.both_completed, r.a_halt,
                );
            }
        }
        assert_eq!(
            agree, total,
            "stage-4 compartment result-agreement bar: {agree}/{total} (every program must agree across both compartments and the oracle)"
        );
    }

    #[test]
    fn stage4_daemon_boot_bundle_never_diverges_and_names_its_gaps() {
        // The stage-4 closure boot-bundle bar (child 5/5; design
        // `daemon-endor-architecture.md` § Unified runner). Dual-run the
        // COMMITTED daemon boot-bundle sources (`polyfills.js`,
        // `host_aliases.js`, and the boot prefix as the daemon evaluates it)
        // against the pin. **Result agreement is the bar** — but the doctrine
        // (accuracy over parity) is what this test actually enforces: endor
        // must NEVER complete a boot-bundle program with a wrong value or
        // accept one the pin rejects. Every program either agrees with the
        // pin or aborts with a SELF-NAMED halt.
        //
        // Verdict at this closure point: the committed bundle does **not** run
        // identically on endor yet — its first statement reads `globalThis`,
        // and endor has no live global-object binding, so every bundle stops
        // there with an honest throw (`boot:no-globalThis-global-object-
        // binding`). That is a **named, ledgered post-stage-4 engine gap**
        // (with the downstream gaps the bundle would hit next — `Reflect`,
        // typed-array-from-iterable, symbol-keyed `defineProperty`,
        // class-instance construction — enumerated in the README stage-4
        // evidence block and reported to s10), NOT a divergence: endor never
        // lies about the boot bundle, it honestly declines it. This test is
        // the regression guard on that safety property AND the ledger anchor
        // that flips when the `globalThis` binding lands.
        let bundles = daemon_boot_bundle_sources();
        assert!(!bundles.is_empty(), "boot-bundle sources must be present");
        let mut divergences = Vec::new();
        let mut gaps = std::collections::BTreeMap::new();
        let mut agree = 0usize;
        for (label, src) in &bundles {
            match boot_bundle_verdict(src) {
                BootVerdict::Agrees => {
                    eprintln!("boot-bundle {label}: AGREES with the pin");
                    agree += 1;
                }
                BootVerdict::NamedGap(key) => {
                    eprintln!("boot-bundle {label}: named gap `{key}`");
                    *gaps.entry(key).or_insert(0usize) += 1;
                }
                BootVerdict::Divergent(detail) => {
                    divergences.push(format!("{label}: {detail}"));
                }
            }
        }
        eprintln!(
            "boot-bundle verdict: {}/{} agree; named gaps: {:?}",
            agree,
            bundles.len(),
            gaps
        );
        // (1) The bar: endor never diverges on a boot-bundle program.
        assert!(
            divergences.is_empty(),
            "boot-bundle divergence(s) forbidden by the accuracy-over-parity doctrine: {divergences:?}"
        );
        // (2) The ledger anchor: while the `globalThis` global-object binding
        // is unimplemented, every committed bundle stops at exactly that named
        // gap. When that binding lands, this assertion flips and the ledger
        // (README stage-4 evidence block) must be updated to the next gap.
        assert_eq!(
            gaps.get("boot:no-globalThis-global-object-binding").copied(),
            Some(bundles.len()),
            "expected every committed boot bundle to stop at the ledgered `globalThis` \
             global-object-binding gap; got {gaps:?} (if a gap closed, advance the ledger)"
        );
    }

    #[test]
    fn compartments_isolate_their_own_globals_against_a_seeded_value() {
        // Global separation, differential against the oracle's notion of a
        // value: seed the SAME global id with a different value in each of
        // two compartments over one machine, evaluate the exact bytecode the
        // oracle emits for a program that reads that global, and confirm each
        // compartment renders ITS OWN binding — matching the oracle's
        // `String()` of that value, and diverging between the compartments.
        use endor_vm::{Machine, Slot};

        // The oracle compiles `x` (a lone global reference) to the read-global
        // bytecode; we seed `x`'s program-local id per compartment. Two
        // literals whose `String()` the oracle certifies:
        let one = endor_oracle::run("String(11)").expect("oracle");
        let two = endor_oracle::run("String(22)").expect("oracle");
        assert_eq!(one.result, "11");
        assert_eq!(two.result, "22");

        // The read-global program addresses the global by its symbol id; we
        // reuse the compartment unit-test shape via the public seam.
        let read_x = {
            use endor_vm::Opcode;
            let [lo, hi] = 7u16.to_le_bytes();
            vec![
                Opcode::XS_CODE_EVAL_REFERENCE as u8, lo, hi,
                Opcode::XS_CODE_GET_VARIABLE as u8, lo, hi,
                Opcode::XS_CODE_SET_RESULT as u8,
                Opcode::XS_CODE_END as u8,
            ]
        };

        let machine = Machine::new();
        let mut a = machine.new_compartment();
        let mut b = machine.new_compartment();
        a.define_global_id(7, Slot::integer(11));
        b.define_global_id(7, Slot::integer(22));
        let ra = a.evaluate(&read_x);
        let rb = b.evaluate(&read_x);
        assert!(ra.completed && rb.completed);
        // Each compartment observes its own global, matching the oracle's
        // `String()` of that value...
        assert_eq!(ra.result, one.result, "compartment A sees its own 11");
        assert_eq!(rb.result, two.result, "compartment B sees its own 22");
        // ...and the two compartments diverge over one shared intrinsics graph.
        assert_ne!(ra.result, rb.result);
        assert!(std::rc::Rc::ptr_eq(a.intrinsics(), b.intrinsics()));
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
