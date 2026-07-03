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
