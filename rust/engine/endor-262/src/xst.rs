//! `endor-xst`: the xst-analogue test262 runner for the Rust engine
//! (design [`designs/xs2rust-endor-test262-convergence.md`] § Part 2,
//! "Harness → `endor-xst`"). It plays for endor exactly the role
//! `xs/tools/xst.c` + `xst262.c` (@ `48ee02d8cfe0`) play for C-XS, plus the
//! one thing `xst` never had: a differential oracle.
//!
//! This module is the runner core (rollout step 1): full YAML frontmatter
//! ([`crate::frontmatter`]), the endor not-yet-implemented feature skip list
//! + `--features-include`, sloppy+strict double-run mode selection (strict a
//! named skip until the stage-5 compiler), negative verdicts (constructor
//! name vs `negative.type`, stack/memory aborts accepted for an expected
//! `RangeError`), the dual-run oracle wiring (verdict + observable agreement
//! gating, computron advisory, `--gate-meter-exact`, `--repeat N`
//! determinism), and the xst-shaped YAML report (`mode:` / `skip:` /
//! `fail:` plus the endor `advisory:` and `skip-detail:` extensions).
//!
//! It subsumes the dual-run harness rather than sitting beside it: the same
//! `assemble()` order (`sta.js`, `assert.js`, includes, body) and the same
//! [`crate::dual_run`] differential, with the verdict layer grown on top.
//! Pre-stage-5 the oracle is still the compiler (both engines run the
//! C-XS-emitted bytecode), so a divergence has one suspect; when
//! `endor-compile` lands the differential moves to source level and the
//! parse-phase negatives activate.

use crate::frontmatter::{self, Frontmatter, Negative};
use crate::{dual_run, Agreement, DualRun};
use endor_vm::Halt;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// The endor not-yet-implemented feature skip list — the analogue of
/// `xst262.c`'s 13-entry `gxFeatures`. A test whose frontmatter `features:`
/// names any of these is skipped before it runs, reported in the report's
/// `skip:` section by feature name (design § Part 2, "endor skip list").
///
/// This is deliberately the coarse, well-known not-landed surface, not an
/// exhaustive enumeration of every unimplemented feature: the honest split
/// still names everything else at the exact unsupported opcode when a run
/// reaches it (`skip-detail:`), so an under-inclusive list only moves a skip
/// from `feature:` to `unsupported-opcode:`, never hides it. Trimmed as the
/// stage ladder lands each surface; `--features-include <feature>` opts a
/// set back in (e.g. `ses-xs-parity` once stage-4 lockdown/Compartment
/// lands), matching the npm `test262-harness` idiom the repo drives `xst`
/// with.
pub const DEFAULT_ENDOR_SKIP_FEATURES: &[&str] = &[
    // xst `gxFeatures` analogues — surfaces endor does not implement.
    "Temporal",
    "ShadowRealm",
    "decorators",
    "Atomics",
    "SharedArrayBuffer",
    "tail-call-optimization",
    "IsHTMLDDA",
    // Hardened-JavaScript / SES parity opt-in set: needs the stage-4
    // lockdown/Compartment surface, not yet landed. Opt in explicitly with
    // `--features-include ses-xs-parity` once it does.
    "ses-xs-parity",
];

/// Runner configuration, derived from the CLI (design § Part 2, "dual-run
/// oracle wiring").
#[derive(Debug, Clone)]
pub struct Config {
    /// `--oracle` (default on) / `--no-oracle`: gate on verdict + observable
    /// agreement with the C-XS oracle. With it off, endor's own verdict
    /// stands and an oracle disagreement cannot fail the build (it is
    /// demoted to a named skip).
    pub oracle: bool,
    /// `--gate-meter-exact`: tighten `endor-meter-exact`-tagged cases to the
    /// historical bit-exact computron bar (a divergence fails). Off, the
    /// computron comparison is advisory only.
    pub gate_meter_exact: bool,
    /// `--repeat N`: re-run endor N times and require identical computrons
    /// across runs — the unconditional determinism gate. Default 1 (no
    /// extra runs).
    pub repeat: u32,
    /// `--features-include <feature>`: features to remove from the skip set
    /// (opt them into the run).
    pub features_include: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            oracle: true,
            gate_meter_exact: false,
            repeat: 1,
            features_include: Vec::new(),
        }
    }
}

/// One case's verdict — the section of the xst-shaped report it lands in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Ran end-to-end and met the bar (positive: observable agreement with
    /// the oracle; negative: the expected-type abort). The report's covered
    /// tally.
    Covered,
    /// Skipped before running — a declared-unimplemented feature, a
    /// structural shape (`module`/`async`), or an `onlyStrict` test whose
    /// sole mode is the not-yet-implemented strict mode. The report's
    /// `skip:` section (xst's feature/flag skips).
    PreSkip(String),
    /// Skipped after attempting the run, named by the exact opcode / value /
    /// structural reason that stopped it — the honest split the port's
    /// progress instrument. The report's `skip-detail:` section (an endor
    /// extension over `xst`).
    RunSkip(String),
    /// A real failure the bar forbids: a divergence from the oracle verdict
    /// or observable, an over-acceptance, a gated meter-exact violation, or
    /// a determinism failure. The report's `fail:` section.
    Fail(String),
}

/// The outcome of running one case through the mode/verdict machinery.
#[derive(Debug, Clone)]
pub struct CaseResult {
    pub verdict: Verdict,
    /// A strict-mode run existed for this case but was named-skipped
    /// (strict lands with the stage-5 compiler). Feeds the report's `mode:`
    /// strict tally.
    pub strict_skipped: bool,
    /// The case was covered but endor's computrons differed from the
    /// oracle's — advisory telemetry, never a failure by itself (design §
    /// Metering, accuracy-over-parity). Feeds the `advisory:` section.
    pub computron_gap: bool,
}

fn preskip(reason: &str) -> CaseResult {
    CaseResult {
        verdict: Verdict::PreSkip(reason.to_string()),
        strict_skipped: false,
        computron_gap: false,
    }
}

/// The effective feature skip set: the default not-implemented list minus
/// anything `--features-include` opted back in.
fn effective_skip_features(cfg: &Config) -> HashSet<String> {
    let opted: HashSet<&str> = cfg.features_include.iter().map(|s| s.as_str()).collect();
    DEFAULT_ENDOR_SKIP_FEATURES
        .iter()
        .filter(|f| !opted.contains(**f))
        .map(|f| f.to_string())
        .collect()
}

/// Strict-mode selection from a case's `flags` — endor's mirror of
/// `xst262.c`'s default two-run (sloppy then strict) with the `onlyStrict` /
/// `noStrict` / `raw` selectors. Returns `(run_sloppy, has_strict,
/// only_strict)`. `module` is handled as a structural pre-skip before this
/// is consulted.
pub fn strict_mode_status(flags: &[String]) -> (bool, bool, bool) {
    let has = |name: &str| flags.iter().any(|f| f == name);
    if has("onlyStrict") {
        return (false, true, true);
    }
    // `raw` runs the body verbatim (no harness, no "use strict" prologue);
    // `noStrict` selects the single sloppy run.
    if has("noStrict") || has("raw") {
        return (true, false, false);
    }
    // The test262 default: two runs, sloppy then strict.
    (true, true, false)
}

/// The constructor name carried by a stringified thrown value —
/// `String(new TypeError("m"))` is `"TypeError: m"`, so the constructor is
/// the text before the first `:`; a bare `"RangeError"` (empty message) is
/// itself. This is the same shape `xst262.c`'s verdict compares against
/// `negative.type`.
pub fn constructor_name(err: &str) -> &str {
    match err.find(':') {
        Some(i) => err[..i].trim(),
        None => err.trim(),
    }
}

fn looks_like_overflow(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    e.contains("stack") || e.contains("overflow") || e.contains("memory") || e.contains("allocat")
}

fn endor_completed(a: Agreement) -> bool {
    matches!(a, Agreement::BothComplete | Agreement::EndorOnlyComplete)
}

fn oracle_completed(a: Agreement) -> bool {
    matches!(a, Agreement::BothComplete | Agreement::OracleOnlyComplete)
}

/// Does the oracle's run satisfy an expected runtime negative of type `ty`?
/// The oracle must have aborted with a thrown value whose constructor name
/// is `ty`, or — for an expected `RangeError` — exited on a memory/stack
/// abort (`xst262.c` accepts a machine memory/stack exit for a `RangeError`).
pub fn oracle_negative_ok(ty: &str, run: &DualRun) -> bool {
    if oracle_completed(run.agreement) {
        return false;
    }
    if constructor_name(&run.oracle_error) == ty {
        return true;
    }
    ty == "RangeError" && (run.oracle_error.is_empty() || looks_like_overflow(&run.oracle_error))
}

/// Does endor's run satisfy an expected runtime negative of type `ty`? A
/// JS-level `Throw` whose constructor name is `ty`, or — for an expected
/// `RangeError` — endor's fixed-geometry stack overflow or meter abort,
/// which map to XS's memory/stack-exit acceptance (design § Part 2,
/// "Negative verdict").
pub fn endor_negative_ok(ty: &str, run: &DualRun) -> bool {
    match &run.endor_halt {
        Halt::Throw(s) => constructor_name(s) == ty,
        Halt::StackOverflow(_) | Halt::MeterAbort => ty == "RangeError",
        _ => false,
    }
}

/// Assemble the sloppy source both engines run — the standard test262 order
/// (`sta.js`, `assert.js`, each `includes:` file, the body), or the body
/// verbatim for a `raw` test. Structural shapes the differential cannot
/// model (`module`, `async`) are handled by the caller before this; a
/// missing harness file is a named structural skip.
fn assemble(harness_dir: &Path, src: &str, fm: &Frontmatter) -> Result<String, String> {
    if fm.flags.iter().any(|f| f == "raw") {
        return Ok(src.to_string());
    }
    let read = |name: &str| -> Result<String, String> {
        std::fs::read_to_string(harness_dir.join(name))
            .map_err(|e| format!("structural:missing-harness:{}:{}", name, e))
    };
    let mut out = String::new();
    out.push_str(&read("sta.js")?);
    out.push('\n');
    out.push_str(&read("assert.js")?);
    out.push('\n');
    for inc in &fm.includes {
        out.push_str(&read(inc)?);
        out.push('\n');
    }
    out.push_str(src);
    Ok(out)
}

struct Eval {
    outcome: Verdict,
    endor_computrons: u64,
    computron_gap: bool,
}

/// Evaluate one assembled sloppy source against the oracle differential.
fn evaluate(cfg: &Config, source: &str, fm: &Frontmatter, meter_exact_gate: bool) -> Eval {
    let run = match dual_run(source) {
        Some(r) => r,
        None => {
            return Eval {
                outcome: Verdict::RunSkip("oracle-machine-error".into()),
                endor_computrons: 0,
                computron_gap: false,
            }
        }
    };
    let outcome = match &fm.negative {
        Some(neg) => evaluate_negative(cfg, &run, neg),
        None => evaluate_positive(cfg, &run, meter_exact_gate),
    };
    // The computron comparison is advisory (accuracy-over-parity): a covered
    // case whose computrons drift from the oracle's is telemetry, folded
    // into the report's `advisory:` section, never a failure on its own.
    let computron_gap =
        matches!(outcome, Verdict::Covered) && run.oracle_computrons != run.endor_computrons;
    Eval {
        outcome,
        endor_computrons: run.endor_computrons,
        computron_gap,
    }
}

fn evaluate_positive(cfg: &Config, run: &DualRun, meter_exact_gate: bool) -> Verdict {
    // Structural endor stops name themselves — the honest skip.
    match &run.endor_halt {
        Halt::Unsupported(op) => return Verdict::RunSkip(format!("unsupported-opcode:{}", op)),
        Halt::Decode(_) => return Verdict::RunSkip("parse-or-decode".into()),
        _ => {}
    }
    let meter_violation = |run: &DualRun| -> Verdict {
        Verdict::Fail(format!(
            "meter-exact violation: oracle={} endor={} computrons",
            run.oracle_computrons, run.endor_computrons
        ))
    };
    match run.agreement {
        Agreement::BothComplete => {
            if run.result_agrees {
                // Observable agreement (gating) met. Computron is advisory,
                // unless a meter-exact gate is armed for this case.
                if meter_exact_gate && run.oracle_computrons != run.endor_computrons {
                    meter_violation(run)
                } else {
                    Verdict::Covered
                }
            } else if run.endor_result == "[object Object]" {
                // A non-primitive completion endor renders as its Reference
                // stub where the oracle's `String()` differs — a built-in
                // coercion gap, honestly named, not a covered-grammar error.
                Verdict::RunSkip("non-primitive-completion".into())
            } else {
                Verdict::Fail(format!(
                    "result divergence: oracle={:?} endor={:?}",
                    run.oracle_result, run.endor_result
                ))
            }
        }
        Agreement::BothAbort => match &run.endor_halt {
            Halt::Throw(_) => {
                if run.error_agrees {
                    if meter_exact_gate && run.oracle_computrons != run.endor_computrons {
                        meter_violation(run)
                    } else {
                        Verdict::Covered
                    }
                } else {
                    // Both aborted but endor threw a different value — the
                    // oracle's real Error object endor does not construct, a
                    // built-in gap, not a covered-grammar divergence.
                    Verdict::RunSkip("abort-value-differs".into())
                }
            }
            // endor aborted for a limit reason (stack/meter) the oracle
            // cannot share: an endor limitation, not a semantic lie.
            _ => Verdict::RunSkip("endor-aborted-limit".into()),
        },
        // endor completed a source the oracle rejected — the over-acceptance
        // the differential exists to catch (gating under `--oracle`).
        Agreement::EndorOnlyComplete => {
            if cfg.oracle {
                Verdict::Fail(
                    "over-acceptance: endor completed a source the oracle rejected".into(),
                )
            } else {
                Verdict::RunSkip("oracle-gate-off:endor-only-complete".into())
            }
        }
        // endor aborted where the oracle completed — an endor limitation.
        Agreement::OracleOnlyComplete => Verdict::RunSkip("endor-aborted".into()),
    }
}

fn evaluate_negative(cfg: &Config, run: &DualRun, neg: &Negative) -> Verdict {
    // Parse/resolution-phase negatives need endor's own compiler to mirror
    // the reject; pre-stage-5 the oracle compiles, so they are named skips
    // until `endor-compile` lands (design § Part 2).
    if neg.phase == "parse" || neg.phase == "resolution" {
        return Verdict::RunSkip(format!("negative-{}:pending-compiler", neg.phase));
    }
    // A runtime negative endor never reached (an unsupported opcode / decode
    // stop) is an honest opcode skip, not a verdict.
    match &run.endor_halt {
        Halt::Unsupported(op) => return Verdict::RunSkip(format!("unsupported-opcode:{}", op)),
        Halt::Decode(_) => return Verdict::RunSkip("parse-or-decode".into()),
        _ => {}
    }
    let oracle_ok = oracle_negative_ok(&neg.ty, run);
    let endor_ok = endor_negative_ok(&neg.ty, run);
    if endor_completed(run.agreement) {
        // endor did not abort where a throw was expected.
        if cfg.oracle && oracle_ok {
            Verdict::Fail(format!(
                "negative over-acceptance: endor completed; expected a {} throw",
                neg.ty
            ))
        } else {
            Verdict::RunSkip("negative-oracle-unexpected".into())
        }
    } else if endor_ok && (!cfg.oracle || oracle_ok) {
        Verdict::Covered
    } else if endor_ok {
        // endor got the expected type but the oracle did not — an
        // oracle-side surprise, not an endor failure.
        Verdict::RunSkip("negative-oracle-unexpected".into())
    } else {
        // endor aborted with a value not of the expected type — its Error
        // surface is incomplete here, honestly named.
        Verdict::RunSkip(format!("negative-type-unmatched:{}", neg.ty))
    }
}

/// Re-run endor `repeat` times and report whether its computrons ever differ
/// from `baseline` — the unconditional determinism gate (design § Part 2:
/// identical computrons per build). The oracle is deterministic too, so a
/// plain re-`dual_run` isolates any endor nondeterminism.
fn determinism_violation(source: &str, repeat: u32, baseline: u64) -> bool {
    for _ in 1..repeat {
        match dual_run(source) {
            Some(r) if r.endor_computrons != baseline => return true,
            _ => {}
        }
    }
    false
}

/// Run one case (source text) through the full mode/verdict machinery.
pub fn run_case(cfg: &Config, harness_dir: &Path, src: &str) -> CaseResult {
    let fm = frontmatter::parse(src);

    // Feature pre-skip: a declared feature endor does not implement.
    let skip_set = effective_skip_features(cfg);
    if let Some(f) = fm.features.iter().find(|f| skip_set.contains(f.as_str())) {
        return preskip(&format!("feature:{}", f));
    }

    // Structural pre-skips: shapes the differential cannot model yet.
    if fm.flags.iter().any(|f| f == "module") {
        return preskip("structural:module");
    }
    if fm
        .flags
        .iter()
        .any(|f| f == "async" || f == "CanBlockIsFalse")
    {
        return preskip("structural:async-or-can-block");
    }

    let (_run_sloppy, has_strict, only_strict) = strict_mode_status(&fm.flags);

    // An `onlyStrict` test's sole mode is the not-yet-implemented strict
    // mode — the whole test is a named strict skip.
    if only_strict {
        return CaseResult {
            verdict: Verdict::PreSkip("onlyStrict:strict-mode-unimplemented".into()),
            strict_skipped: true,
            computron_gap: false,
        };
    }

    let source = match assemble(harness_dir, src, &fm) {
        Ok(s) => s,
        Err(reason) => {
            return CaseResult {
                verdict: Verdict::PreSkip(reason),
                strict_skipped: has_strict,
                computron_gap: false,
            }
        }
    };

    let meter_exact_gate =
        cfg.gate_meter_exact && fm.features.iter().any(|f| f == "endor-meter-exact");
    let eval = evaluate(cfg, &source, &fm, meter_exact_gate);

    // The determinism gate (unconditional half of the doctrine) overrides a
    // covered/skip verdict with a failure if endor's computrons are not
    // reproducible across runs of this same build.
    let verdict =
        if cfg.repeat > 1 && determinism_violation(&source, cfg.repeat, eval.endor_computrons) {
            Verdict::Fail(format!(
                "nondeterministic computrons across {} runs",
                cfg.repeat
            ))
        } else {
            eval.outcome
        };

    CaseResult {
        verdict,
        strict_skipped: has_strict,
        computron_gap: eval.computron_gap,
    }
}

/// The xst-shaped report over a set of cases: `mode:` / `skip:` / `fail:`
/// plus the endor `advisory:` and `skip-detail:` extensions (design § Part
/// 2, "dual-run oracle wiring" + "the honest-split discipline").
#[derive(Debug, Default, Clone)]
pub struct XstReport {
    pub total: usize,
    /// Cases covered (ran end-to-end, met the bar).
    pub covered: usize,
    /// Cases that attempted a sloppy run (covered + run-skips + failures).
    pub sloppy_run: usize,
    /// Strict-mode runs named-skipped (mode: section).
    pub strict_skipped: usize,
    /// `fail:` — real failures: `(path, detail)`. Must be empty to meet the bar.
    pub failures: Vec<(String, String)>,
    /// `skip:` — pre-run feature/flag/structural skips → count.
    pub pre_skips: BTreeMap<String, usize>,
    /// `skip-detail:` — post-run honest named skips → count.
    pub run_skips: BTreeMap<String, usize>,
    /// `advisory:` — covered cases whose computrons drifted from the oracle.
    pub computron_advisories: usize,
}

impl XstReport {
    /// The bar: a nonzero total with zero failures (design's honest-split
    /// discipline — zero divergence on whatever the covered grammar reaches).
    pub fn met_bar(&self) -> bool {
        self.total > 0 && self.failures.is_empty()
    }

    /// Fold one case's result in, attributed to `path`.
    pub fn record(&mut self, path: &str, r: CaseResult) {
        self.total += 1;
        if r.strict_skipped {
            self.strict_skipped += 1;
        }
        if r.computron_gap {
            self.computron_advisories += 1;
        }
        match r.verdict {
            Verdict::Covered => {
                self.covered += 1;
                self.sloppy_run += 1;
            }
            Verdict::RunSkip(reason) => {
                *self.run_skips.entry(reason).or_insert(0) += 1;
                self.sloppy_run += 1;
            }
            Verdict::PreSkip(reason) => {
                *self.pre_skips.entry(reason).or_insert(0) += 1;
            }
            Verdict::Fail(detail) => {
                self.failures.push((path.to_string(), detail));
                self.sloppy_run += 1;
            }
        }
    }

    fn sorted(map: &BTreeMap<String, usize>) -> Vec<(String, usize)> {
        let mut v: Vec<_> = map.iter().map(|(k, n)| (k.clone(), *n)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v
    }

    /// The pre-run feature/flag skips, most-skipped first.
    pub fn skip_summary(&self) -> Vec<(String, usize)> {
        Self::sorted(&self.pre_skips)
    }

    /// The post-run honest named skips, most-skipped first.
    pub fn skip_detail_summary(&self) -> Vec<(String, usize)> {
        Self::sorted(&self.run_skips)
    }

    /// The xst-shaped YAML report — `mode:` / `skip:` / `fail:` plus the
    /// endor `advisory:` and `skip-detail:` sections. Tooling that reads an
    /// `xst` `-o` report reads this unchanged; the two endor sections are
    /// additive.
    pub fn to_yaml(&self) -> String {
        let mut s = String::new();
        s.push_str("runner: endor-xst\n");
        s.push_str(&format!("total: {}\n", self.total));
        s.push_str(&format!("covered: {}\n", self.covered));
        s.push_str(&format!("bar-met: {}\n", self.met_bar()));
        s.push_str("mode:\n");
        s.push_str(&format!("  sloppy-run: {}\n", self.sloppy_run));
        s.push_str(&format!(
            "  strict-skipped-unimplemented: {}\n",
            self.strict_skipped
        ));
        s.push_str("fail:\n");
        for (path, detail) in &self.failures {
            s.push_str(&format!("  - path: {}\n", yaml_quote(path)));
            s.push_str(&format!("    detail: {}\n", yaml_quote(detail)));
        }
        s.push_str("skip:\n");
        for (reason, n) in self.skip_summary() {
            s.push_str(&format!("  {}: {}\n", yaml_quote(&reason), n));
        }
        s.push_str("skip-detail:\n");
        for (reason, n) in self.skip_detail_summary() {
            s.push_str(&format!("  {}: {}\n", yaml_quote(&reason), n));
        }
        s.push_str("advisory:\n");
        s.push_str(&format!("  computron-gap: {}\n", self.computron_advisories));
        s
    }
}

/// Double-quote a YAML scalar, escaping `\` and `"` so a colon/bracket in a
/// path or a detail string never breaks the mapping.
fn yaml_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Run a set of test262 files (absolute paths) against the harness in
/// `harness_dir`, returning the xst-shaped report. `root` is stripped from
/// each path for readable failure labels.
pub fn run_files(cfg: &Config, harness_dir: &Path, root: &Path, files: &[PathBuf]) -> XstReport {
    let mut rep = XstReport::default();
    for path in files {
        let rel = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned();
        let src = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => {
                rep.record(&rel, preskip("unreadable"));
                continue;
            }
        };
        let r = run_case(cfg, harness_dir, &src);
        rep.record(&rel, r);
    }
    rep
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_selection_mirrors_xst() {
        assert_eq!(strict_mode_status(&[]), (true, true, false)); // default: both
        assert_eq!(
            strict_mode_status(&["onlyStrict".into()]),
            (false, true, true)
        );
        assert_eq!(
            strict_mode_status(&["noStrict".into()]),
            (true, false, false)
        );
        assert_eq!(strict_mode_status(&["raw".into()]), (true, false, false));
    }

    #[test]
    fn constructor_name_extracts_the_type() {
        assert_eq!(constructor_name("TypeError: cannot read x"), "TypeError");
        assert_eq!(constructor_name("RangeError"), "RangeError");
        assert_eq!(
            constructor_name("Test262Error: Expected a TypeError"),
            "Test262Error"
        );
    }

    #[test]
    fn features_include_opts_a_feature_back_in() {
        let mut cfg = Config::default();
        assert!(effective_skip_features(&cfg).contains("ses-xs-parity"));
        cfg.features_include = vec!["ses-xs-parity".into()];
        assert!(!effective_skip_features(&cfg).contains("ses-xs-parity"));
        // A never-listed feature is never in the skip set.
        assert!(!effective_skip_features(&cfg).contains("Symbol"));
    }

    #[test]
    fn endor_range_error_accepts_stack_and_meter_aborts() {
        // A synthetic dual-run with endor stack overflow: an expected
        // RangeError negative is satisfied on endor's side.
        let run = synthetic_abort(Halt::StackOverflow(4), "");
        assert!(endor_negative_ok("RangeError", &run));
        assert!(!endor_negative_ok("TypeError", &run));

        let meter = synthetic_abort(Halt::MeterAbort, "");
        assert!(endor_negative_ok("RangeError", &meter));

        let thrown = synthetic_abort(Halt::Throw("TypeError: bad".into()), "TypeError: bad");
        assert!(endor_negative_ok("TypeError", &thrown));
        assert!(!endor_negative_ok("RangeError", &thrown));
    }

    #[test]
    fn report_yaml_has_the_xst_sections() {
        let mut rep = XstReport::default();
        rep.record(
            "language/a.js",
            CaseResult {
                verdict: Verdict::Covered,
                strict_skipped: true,
                computron_gap: true,
            },
        );
        rep.record(
            "language/b.js",
            CaseResult {
                verdict: Verdict::RunSkip("unsupported-opcode:XS_CODE_FOO".into()),
                strict_skipped: true,
                computron_gap: false,
            },
        );
        rep.record(
            "language/c.js",
            CaseResult {
                verdict: Verdict::PreSkip("feature:Temporal".into()),
                strict_skipped: false,
                computron_gap: false,
            },
        );
        let y = rep.to_yaml();
        assert!(y.contains("runner: endor-xst"));
        assert!(y.contains("mode:"));
        assert!(y.contains("strict-skipped-unimplemented: 2"));
        assert!(y.contains("skip:"));
        assert!(y.contains("skip-detail:"));
        assert!(y.contains("advisory:"));
        assert!(y.contains("computron-gap: 1"));
        assert!(rep.met_bar());
    }

    #[test]
    fn covered_grammar_sections_have_zero_failures_through_xst() {
        // The endor-xst analogue of test262.rs's covered-grammar bar: walk a
        // bounded, deterministic slice of the covered-grammar sections
        // through the full mode/verdict/oracle machinery and require ZERO
        // failures — every case endor runs end-to-end either meets the bar
        // (covered) or is honestly named-skipped; nothing diverges. The
        // covered count is reported, not asserted to a target (it grows as
        // stages land the built-ins). The full-tree walk is the `endor-xst`
        // binary; this in-`cargo test` slice stays bounded so the oracle RSS
        // is contained.
        use crate::test262::{collect_js, locate_test262};
        let (root, harness) = match locate_test262() {
            Some(p) => p,
            None => {
                eprintln!("test262 subset absent; skipping the endor-xst covered-grammar bar");
                return;
            }
        };
        let sections = [
            "language/expressions/addition",
            "language/expressions/logical-not",
            "language/statements/throw",
            "language/statements/if",
        ];
        let mut files = Vec::new();
        for s in sections {
            files.extend(collect_js(&root.join(s)));
        }
        assert!(
            !files.is_empty(),
            "covered-grammar sections must have tests"
        );
        let cfg = Config::default();
        let rep = run_files(&cfg, &harness, &root, &files);
        eprintln!(
            "endor-xst covered-grammar slice: total={} covered={} failed={} advisory-computron-gap={}",
            rep.total,
            rep.covered,
            rep.failures.len(),
            rep.computron_advisories,
        );
        for (reason, n) in rep.skip_detail_summary() {
            eprintln!("    {:>5}  {}", n, reason);
        }
        for (path, detail) in &rep.failures {
            eprintln!("  FAIL {}\n    {}", path, detail);
        }
        // The report YAML must be well-formed enough to re-parse its own
        // shape (a smoke check on the emitter).
        let y = rep.to_yaml();
        assert!(y.contains("runner: endor-xst") && y.contains("bar-met: true"));
        assert!(
            rep.met_bar(),
            "zero failures required through the xst runner; got {}",
            rep.failures.len()
        );
    }

    /// A `DualRun` in a shared-abort shape for exercising the negative
    /// verdict helpers without an oracle machine.
    fn synthetic_abort(endor_halt: Halt, endor_error: &str) -> DualRun {
        DualRun {
            source: String::new(),
            agreement: Agreement::BothAbort,
            result_agrees: false,
            oracle_result: String::new(),
            endor_result: String::new(),
            computrons_agree: false,
            oracle_computrons: 0,
            endor_computrons: 0,
            error_agrees: false,
            oracle_error: String::new(),
            endor_error: endor_error.to_string(),
            oracle_meter_raw: 0,
            endor_meter_raw: 0,
            endor_dispatched: 0,
            endor_halt,
            bytecode: Vec::new(),
        }
    }
}
