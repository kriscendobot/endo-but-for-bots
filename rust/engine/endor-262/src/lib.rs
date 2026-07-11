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
//! The bespoke per-stage corpus (`corpora/*.js` + the `stage*_corpus()`
//! accessors) that drove bring-up has **retired** into a test262-shaped
//! `cases/` tree: the `corpus-to-262` converter (bin) dual-runs each corpus
//! line once and emits one standard test262 case, and `endor-xst` ([`xst`])
//! runs that tree with the same differential (design § Part 1, "the corpus
//! becomes test262 cases"). The coverage-equivalence proof lives in
//! `tests/corpus_conversion_equivalence.rs`. Whole-section runs draw from the
//! monorepo's existing `packages/test262-runner` test262 subset and its
//! `ses-xs-parity` feature markers -- the same tree that package uses to
//! prove XS<->Node HardenedJS parity -- rather than a separate pinned test262
//! submodule (maintainer directive, PR #600, 2026-07-03; design section
//! "test262 conformance"). [`dual_run`] and [`parse_corpus`] remain the
//! differential and line-splitting primitives the converter and the runner
//! share.

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
fn compile_for(
    compiler: Compiler,
    source: &str,
    oracle: &endor_oracle::OracleOutcome,
) -> (Vec<u8>, Vec<u8>) {
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

/// Parse a corpus file: one program per non-empty, non-`//` line. Keeping
/// entries to a single line keeps the completion value (the last expression)
/// unambiguous. The per-stage `stage*_corpus()` accessors this once fed
/// retired with the corpus → test262 case conversion (design § Part 1); the
/// function survives because the `corpus-to-262` converter still reads the
/// `corpora/*.js` lines through it during a re-generation.
pub fn parse_corpus(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with("//"))
        .map(|l| l.to_string())
        .collect()
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
        let programs = [
            "1 + 2 * 3",
            "if (1) { 2 } else { 3 }",
            "(function(a){ return a + 1 })(4)",
        ];
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
        assert!(
            throwing.is_bit_exact(),
            "BothAbort with a Throw is bit-exact"
        );

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
        assert!(
            !wrong_value.is_bit_exact(),
            "a divergent thrown value is not bit-exact"
        );

        // Divergent computrons on an otherwise-matching throw.
        let mut wrong_cost = r.clone();
        wrong_cost.endor_computrons = 7;
        assert!(
            !wrong_cost.is_bit_exact(),
            "a divergent computron count is not bit-exact"
        );
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

    // Arm a fresh endor interpreter on the oracle's bytecode for `src`,
    // recording every computron value the meter host is consulted at and
    // whether the run was allowed to complete (`allow`) or refused at the
    // `refuse_at`-th consultation (1-based; 0 = never refuse). Returns
    // `(halt, completed, consulted_computrons)`.
    fn metered_run(src: &str, interval: u64, refuse_at: usize) -> (endor_vm::Halt, bool, Vec<u64>) {
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
        assert_eq!(
            halt,
            endor_vm::Halt::Return,
            "must complete: no check point"
        );
        assert!(completed);
        assert!(
            consulted.is_empty(),
            "the exit-to-C return must not check the meter"
        );
    }

    #[test]
    fn meter_checks_fire_at_call_entry_and_return_into_js() {
        // A single user-function call has exactly two `mxFirstCode` check
        // points: call entry (`run` installing the callee frame) and the
        // callee's `end` returning into the JS program frame. The
        // program's own final `return` (exit to C) does not check. So a
        // permissive armed run is consulted exactly twice and completes.
        let (halt, completed, consulted) = metered_run("(function(){return 1})()", 1, 0);
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
        let (halt, completed, consulted) = metered_run("(function(){return 1})()", 1, 1);
        assert_eq!(
            halt,
            endor_vm::Halt::MeterAbort,
            "must abort at the call-entry check"
        );
        assert!(
            !completed,
            "the call must not complete once refused at entry"
        );
        assert_eq!(
            consulted.len(),
            1,
            "aborts on the first (call-entry) consultation"
        );
    }

    #[test]
    fn armed_meter_aborts_at_backward_branch_in_a_loop() {
        // A loop's backward branch is a check point (as in stage 2a); a
        // function body containing a loop still aborts there under an armed
        // meter, never at the function's `end` exit or the program
        // `return`.
        let src = "var i=0; while(i<1000000){i=i+1} i";
        let (halt, completed, _consulted) = metered_run(src, 1, 3);
        assert_eq!(
            halt,
            endor_vm::Halt::MeterAbort,
            "the backward branch must abort"
        );
        assert!(!completed);
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
        assert_eq!(
            r.oracle_computrons, r.endor_computrons,
            "uncaught-throw computrons agree"
        );
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
        assert_eq!(
            one.endor_result, "2",
            "the shared cell mutates across calls"
        );
        assert_eq!(one.oracle_result, "2");

        let two = dual_run(
            "var mk=function(){var n=0; return function(){return n=n+1}}; var a=mk(),b=mk(); a(); a(); b()",
        )
        .expect("oracle");
        assert_eq!(two.endor_result, "1", "b's cell is independent of a's");
        assert_eq!(two.oracle_result, "1");
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
            gaps.get("boot:no-globalThis-global-object-binding")
                .copied(),
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
                Opcode::XS_CODE_EVAL_REFERENCE as u8,
                lo,
                hi,
                Opcode::XS_CODE_GET_VARIABLE as u8,
                lo,
                hi,
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
            assert_eq!(
                a.computrons, b.computrons,
                "determinism-per-release for {src:?}"
            );
            assert_eq!(
                a.computrons, *expected,
                "frozen UTF-16 computron cost for {src:?} (endor's recalibrated value)",
            );
        }
    }
}
