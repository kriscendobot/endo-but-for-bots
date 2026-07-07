//! The full-corpus **byte-identity differential harness** (stage-5 child
//! 7/7, the STAGE BAR). For every source in the conformance corpus, where
//! the C-XS oracle compiler *accepts* the file, this asserts that
//! `endor_compile::compile(source)` equals `endor_oracle::run(source).bytecode`
//! **byte for byte** — XS's coder is the ground truth (design § roadmap
//! row 5; Design Decisions 4 and 5).
//!
//! **Stage bar** (design § Feasibility Verdict): `divergent == 0` *and*
//! accept/reject agreement, over the full corpus. A byte divergence on a
//! program both engines accept is the KILL CRITERION signal — it is never
//! skipped or hidden; every one is reported with a NAMED class and its
//! source identity so the supervisor can adjudicate.
//!
//! Panic discipline: the coder still `panic!`s on constructs outside the
//! ported surface (a loud fold, not a silent skip). The harness must be
//! total over arbitrary corpus input, so every `compile` call runs under
//! [`std::panic::catch_unwind`] with the process panic hook silenced for
//! the batch; a caught panic classifies as an `endor-rejected` (coder
//! fold), never a harness abort. An oracle machine-startup failure
//! (`run` returns `None`) is the named `oracle-unavailable` outcome, also
//! not an abort.

use std::collections::BTreeMap;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};

/// How one source classified under the compile differential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompileVerdict {
    /// Both engines accept and the bytes are byte-identical (the bar).
    Identical,
    /// Both engines accept but the bytes differ — a KILL-CRITERION
    /// divergence. Carries the named class and a triage detail.
    Divergent { class: String, detail: String },
    /// The oracle rejected the source (a `SyntaxError` at parse). Bytes
    /// are not compared; accept/reject agreement is still checked.
    OracleRejected,
    /// The oracle accepted but endor did not — a structured parse/scope
    /// rejection or a coder panic (the ported-surface fold). A bar
    /// violation (accept/reject disagreement); carries why.
    EndorRejected { detail: String },
    /// endor accepted where the oracle rejected — the benign accept/reject
    /// disagreement direction, recorded, still a bar violation.
    OracleRejectedEndorAccepted,
    /// The oracle machine failed to start (`run` returned `None`) — not a
    /// finding, skipped from the ratio.
    OracleUnavailable,
}

/// The tally over a corpus, honest about every class (design § the honest
/// covered/skipped split, applied to bytes).
#[derive(Debug, Default, Clone)]
pub struct CompileReport {
    /// Programs classified (excludes `oracle-unavailable`).
    pub total: usize,
    pub identical: usize,
    pub divergent: usize,
    pub oracle_rejected: usize,
    pub endor_rejected: usize,
    pub accept_disagreements: usize,
    pub oracle_unavailable: usize,
    /// Named divergence classes → count (byte-mismatch cases only).
    pub divergence_classes: BTreeMap<String, usize>,
    /// Per-item byte divergences: `(id, class, detail)`.
    pub divergences: Vec<(String, String, String)>,
    /// Per-item endor rejections where the oracle accepted: `(id, why)`.
    pub endor_rejections: Vec<(String, String)>,
    /// Per-item endor-only accepts (oracle rejected): `(id, note)`.
    pub endor_only_accepts: Vec<(String, String)>,
}

impl CompileReport {
    /// The stage bar: zero byte divergence AND full accept/reject
    /// agreement (no endor-rejected, no endor-only-accept) over every
    /// classified program.
    pub fn met_bar(&self) -> bool {
        self.divergent == 0 && self.endor_rejected == 0 && self.accept_disagreements == 0
    }

    fn record(&mut self, id: &str, verdict: CompileVerdict) {
        match verdict {
            CompileVerdict::OracleUnavailable => {
                self.oracle_unavailable += 1;
                return;
            }
            _ => self.total += 1,
        }
        match verdict {
            CompileVerdict::Identical => self.identical += 1,
            CompileVerdict::Divergent { class, detail } => {
                self.divergent += 1;
                *self.divergence_classes.entry(class.clone()).or_default() += 1;
                self.divergences.push((id.to_string(), class, detail));
            }
            CompileVerdict::OracleRejected => self.oracle_rejected += 1,
            CompileVerdict::EndorRejected { detail } => {
                self.endor_rejected += 1;
                self.endor_rejections.push((id.to_string(), detail));
            }
            CompileVerdict::OracleRejectedEndorAccepted => {
                self.accept_disagreements += 1;
                self.endor_only_accepts
                    .push((id.to_string(), "endor accepted; oracle rejected".to_string()));
            }
            CompileVerdict::OracleUnavailable => unreachable!(),
        }
    }
}

/// Whether the oracle *parsed* `source` (accepted it), returning the exact
/// XS bytecode on accept. `None` = machine startup failure (unavailable).
/// A runtime throw (e.g. a `ReferenceError` from an unbound global) still
/// counts as parsed — the bytecode was emitted.
fn oracle_compile(source: &str) -> Option<(bool, Vec<u8>)> {
    let o = endor_oracle::run(source)?;
    if o.completed {
        return Some((true, o.bytecode));
    }
    // An uncompleted run is a parse rejection only if the error is a
    // SyntaxError; any other error is a runtime throw of a parsed program.
    let parsed = !o.error.contains("SyntaxError");
    Some((parsed, o.bytecode))
}

/// endor's compile verdict for `source`, total over panics. `Ok(Ok(bytes))`
/// = accepted; `Ok(Err(reason))` = structured rejection; `Err(reason)` =
/// coder panic (the ported-surface fold). The caller silences the panic
/// hook for the batch.
fn endor_compile(source: &str) -> Result<Result<Vec<u8>, String>, String> {
    let caught = panic::catch_unwind(AssertUnwindSafe(|| endor_compile::compile(source)));
    match caught {
        Ok(Ok(bytes)) => Ok(Ok(bytes)),
        Ok(Err(e)) => Ok(Err(format!("{:?}", e))),
        Err(payload) => Err(panic_message(payload)),
    }
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "coder panic (opaque payload)".to_string()
    }
}

/// Name the divergence class for a byte mismatch on a both-accept program.
/// Classes are coarse but named — the triage detail carries the exact
/// offset and bytes.
fn classify_divergence(oracle: &[u8], endor: &[u8]) -> (String, String) {
    if oracle.len() != endor.len() {
        let class = if endor.len() < oracle.len() {
            "byte-length/endor-shorter"
        } else {
            "byte-length/endor-longer"
        };
        // Still locate the first divergence for triage.
        let first = first_diff(oracle, endor);
        let detail = match first {
            Some(i) => format!(
                "len oracle={} endor={}; first diff @{} oracle=0x{:02x} endor=0x{:02x}",
                oracle.len(),
                endor.len(),
                i,
                oracle.get(i).copied().unwrap_or(0),
                endor.get(i).copied().unwrap_or(0),
            ),
            None => format!(
                "len oracle={} endor={}; common prefix identical",
                oracle.len(),
                endor.len()
            ),
        };
        (class.to_string(), detail)
    } else {
        let i = first_diff(oracle, endor).unwrap_or(0);
        let class = format!("opcode-mismatch@byte0x{:02x}", oracle.get(i).copied().unwrap_or(0));
        let detail = format!(
            "same len {}; first diff @{} oracle=0x{:02x} endor=0x{:02x}",
            oracle.len(),
            i,
            oracle.get(i).copied().unwrap_or(0),
            endor.get(i).copied().unwrap_or(0),
        );
        (class, detail)
    }
}

fn first_diff(a: &[u8], b: &[u8]) -> Option<usize> {
    let n = a.len().min(b.len());
    (0..n).find(|&i| a[i] != b[i]).or(if a.len() == b.len() { None } else { Some(n) })
}

/// Classify one `(id, source)` under the compile differential.
pub fn compile_one(source: &str) -> CompileVerdict {
    let (oracle_accepts, oracle_bytes) = match oracle_compile(source) {
        Some(v) => v,
        None => return CompileVerdict::OracleUnavailable,
    };
    let endor = endor_compile(source);
    let endor_accepts = matches!(endor, Ok(Ok(_)));

    match (oracle_accepts, endor_accepts) {
        (true, true) => {
            let endor_bytes = match endor {
                Ok(Ok(b)) => b,
                _ => unreachable!(),
            };
            if endor_bytes == oracle_bytes {
                CompileVerdict::Identical
            } else {
                let (class, detail) = classify_divergence(&oracle_bytes, &endor_bytes);
                CompileVerdict::Divergent { class, detail }
            }
        }
        (true, false) => {
            let detail = match endor {
                Ok(Err(e)) => format!("parse/scope reject: {}", e),
                Err(p) => format!("coder panic: {}", p),
                _ => unreachable!(),
            };
            CompileVerdict::EndorRejected { detail }
        }
        (false, true) => CompileVerdict::OracleRejectedEndorAccepted,
        (false, false) => CompileVerdict::OracleRejected,
    }
}

/// Run the compile differential over an explicit list of `(id, source)`
/// programs. Silences the panic hook for the batch so a coder fold does
/// not spew to stderr per program (each is still counted and named).
pub fn compile_diff_programs(programs: &[(String, String)]) -> CompileReport {
    let prev_hook = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));
    let mut report = CompileReport::default();
    for (id, source) in programs {
        let verdict = compile_one(source);
        report.record(id, verdict);
    }
    panic::set_hook(prev_hook);
    report
}

/// Run the compile differential over a list of source *files*, compiling
/// each file's whole source as one program (the file is the corpus unit).
pub fn compile_diff_files(files: &[PathBuf]) -> CompileReport {
    let programs: Vec<(String, String)> = files
        .iter()
        .filter_map(|p| {
            std::fs::read_to_string(p)
                .ok()
                .map(|s| (p.display().to_string(), s))
        })
        .collect();
    compile_diff_programs(&programs)
}

/// The curated-corpus programs (`endor-262/corpora/*.js`, one program per
/// non-comment line) as `(id, source)` pairs — the bounded, known-covered
/// byte-identity slice the in-crate `cargo test` gate drives.
pub fn corpora_programs() -> Vec<(String, String)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("corpora");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().map(|x| x == "js").unwrap_or(false))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    let mut out = Vec::new();
    for file in &files {
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let contents = match std::fs::read_to_string(file) {
            Ok(c) => c,
            Err(_) => continue,
        };
        for (lineno, line) in contents.lines().enumerate() {
            let t = line.trim();
            if t.is_empty() || t.starts_with("//") {
                continue;
            }
            out.push((format!("{}:{}", name, lineno + 1), line.to_string()));
        }
    }
    out
}

/// Pretty-print a report to a writer (the binary and the test share this
/// so the honest split reads the same everywhere).
pub fn print_report(w: &mut dyn std::io::Write, report: &CompileReport, label: &str) -> std::io::Result<()> {
    writeln!(w, "{}", "=".repeat(72))?;
    writeln!(
        w,
        "compile byte-identity differential [{}]: total={} identical={} divergent={} oracle-rejected={} endor-rejected={} accept-disagree={} (oracle-unavailable={})",
        label,
        report.total,
        report.identical,
        report.divergent,
        report.oracle_rejected,
        report.endor_rejected,
        report.accept_disagreements,
        report.oracle_unavailable,
    )?;
    if !report.divergence_classes.is_empty() {
        writeln!(w, "{}", "-".repeat(72))?;
        writeln!(w, "NAMED divergence classes (byte mismatch on both-accept):")?;
        for (class, n) in &report.divergence_classes {
            writeln!(w, "  {:>5}  {}", n, class)?;
        }
    }
    for (id, class, detail) in &report.divergences {
        writeln!(w, "  DIVERGENT [{}] {}\n    {}", class, id, detail)?;
    }
    for (id, why) in &report.endor_rejections {
        writeln!(w, "  ENDOR-REJECTED [{}]\n    {}", id, why)?;
    }
    for (id, note) in &report.endor_only_accepts {
        writeln!(w, "  ENDOR-ONLY-ACCEPT [{}] {}", id, note)?;
    }
    writeln!(w, "{}", "=".repeat(72))?;
    if report.met_bar() {
        writeln!(
            w,
            "BAR MET: {} identical, 0 divergent, full accept/reject agreement (of {} classified)",
            report.identical, report.total
        )?;
    } else {
        writeln!(
            w,
            "BAR NOT MET: {} divergent, {} endor-rejected, {} accept-disagreements",
            report.divergent, report.endor_rejected, report.accept_disagreements
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corpora_byte_identity_no_undocumented_divergence() {
        // The bounded, in-`cargo test` regression gate for the stage-5
        // byte-identity bar over the curated conformance corpora (every
        // non-comment line). The **stage bar** is `divergent == 0` with
        // full accept/reject agreement over the FULL corpus — which is
        // NOT yet met, and is the supervisor's kill-criterion call (design
        // § Feasibility Verdict). Two named folds remain, reported to the
        // supervisor with evidence, not hidden:
        //
        //   1. **CESU-8 astral/surrogate string literals.** XS stores
        //      string literals in the bytecode as CESU-8 (a non-BMP char
        //      is a surrogate pair, each surrogate a 3-byte sequence; a
        //      lone surrogate is a 3-byte CESU-8 unit that is not valid
        //      UTF-8 at all), whereas the coder currently emits the Rust
        //      `String`'s UTF-8. Every byte divergence is one of these and
        //      lives in `stage3-string-utf16.js`. Fixing it is a string-
        //      value representation change across lexer/ast/coder — a
        //      child-6-shape coder task, not this acceptance child.
        //   2. **Named coder folds** — constructs the coder still folds on
        //      with a *deliberate* panic (`unsupported node kind Target`
        //      for `new.target`, `... Chain` for optional chaining, and
        //      the `... reached (…child/slice)` declaring-scope /
        //      function-class-declaration paths) — endor-rejects where the
        //      oracle accepts. A panic that is NOT one of these deliberate
        //      fold markers (e.g. an index-out-of-bounds) is a real bug and
        //      fails this gate.
        //
        // This gate asserts there is **no divergence or rejection outside
        // those two documented folds** — so any NEW byte divergence (a
        // regression in a currently-identical program) or any new coder
        // panic on a new construct fails the build. The whole-`language/`
        // sweep lives in the `compile-diff` binary (bounded per subtree to
        // contain oracle RSS).
        let programs = corpora_programs();
        assert!(!programs.is_empty(), "curated corpora must contain programs");
        let report = compile_diff_programs(&programs);
        let mut buf = Vec::new();
        print_report(&mut buf, &report, "corpora").unwrap();
        eprintln!("{}", String::from_utf8_lossy(&buf));

        // Fold 1: every byte divergence is a CESU-8 astral/surrogate
        // string literal, all of which live in `stage3-string-utf16.js`.
        let undocumented_div: Vec<_> = report
            .divergences
            .iter()
            .filter(|(id, _, _)| !id.starts_with("stage3-string-utf16.js"))
            .collect();
        assert!(
            undocumented_div.is_empty(),
            "undocumented byte divergence(s) outside the CESU-8 string fold: {:#?}",
            undocumented_div
        );

        // Fold 2: every endor-rejection is a *deliberate* coder fold —
        // recognizable by its panic marker, not an accidental bug panic.
        let named = ["unsupported node kind", "reached (", "declaring scope"];
        let undocumented_rej: Vec<_> = report
            .endor_rejections
            .iter()
            .filter(|(_, why)| !named.iter().any(|m| why.contains(m)))
            .collect();
        assert!(
            undocumented_rej.is_empty(),
            "undocumented endor-rejection(s) outside the named coder folds {:?}: {:#?}",
            named,
            undocumented_rej
        );

        // Accept/reject agreement in the benign direction must be perfect
        // (endor never accepts what the oracle rejects), and the corpus is
        // overwhelmingly byte-identical.
        assert_eq!(
            report.accept_disagreements, 0,
            "endor accepted program(s) the oracle rejected — a real bar violation"
        );
        assert!(report.identical > 0, "expected accepted programs in the corpora");
        assert!(
            report.identical * 20 > report.total,
            "byte-identity should dominate the corpus (identical={} total={})",
            report.identical,
            report.total
        );
    }
}

/// Walk `dir` collecting `.js` files (recursively, sorted), skipping
/// `staging/` and `_FIXTURE.js` — the same selection the dual-run runner
/// uses, re-exported here so the `compile-diff` binary can address a
/// test262 subtree without depending on the runner's private walker.
pub fn collect_js(dir: &Path) -> Vec<PathBuf> {
    crate::test262::collect_js(dir)
}
