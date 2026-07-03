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

use endor_vm::{run_program, Halt, RunOutcome};

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
            Agreement::BothAbort => matches!(self.endor_halt, Halt::Throw(_)),
            _ => false,
        }
    }
}

/// Run one program on both engines and compare.
///
/// Returns `None` only if the oracle machine itself fails to start.
pub fn dual_run(source: &str) -> Option<DualRun> {
    let oracle = endor_oracle::run(source)?;

    let endor: RunOutcome = run_program(&oracle.bytecode);

    let agreement = match (oracle.completed, endor.completed) {
        (true, true) => Agreement::BothComplete,
        (false, false) => Agreement::BothAbort,
        (false, true) => Agreement::EndorOnlyComplete,
        (true, false) => Agreement::OracleOnlyComplete,
    };

    let result_agrees = oracle.completed && endor.completed && oracle.result == endor.result;
    let computrons_agree =
        oracle.completed && endor.completed && oracle.computrons == endor.computrons;

    Some(DualRun {
        source: source.to_string(),
        agreement,
        result_agrees,
        oracle_result: oracle.result,
        endor_result: endor.result,
        computrons_agree,
        oracle_computrons: oracle.computrons,
        endor_computrons: endor.computrons,
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
    parse_corpus(include_str!("../corpora/stage2-behavioral.js"))
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

    // A `DualRun` with the given agreement and endor halt; the other
    // fields are irrelevant to `is_bit_exact` for aborts.
    fn abort_run(agreement: Agreement, endor_halt: Halt) -> DualRun {
        DualRun {
            source: String::new(),
            agreement,
            result_agrees: false,
            oracle_result: String::new(),
            endor_result: String::new(),
            computrons_agree: false,
            oracle_computrons: 0,
            endor_computrons: 0,
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
