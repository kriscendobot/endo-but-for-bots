//! Whole-corpus parse smoke (stage-5 child 3 local bar).
//!
//! Every conformance-corpus program (the curated `endor-262/corpora`
//! files, one program per line) is parsed by the `endor-compile` parser
//! **as a Script** and its accept/reject verdict compared against the
//! C-XS oracle (`endor_oracle::run`). Two things are asserted:
//!
//!   * **Zero panics.** Every line yields a `Result`, never a panic — the
//!     invariant the parser fuzz target (a later child) depends on.
//!   * **Accept/reject agreement.** Every program the oracle *parses* (did
//!     not reject with a `SyntaxError`) the endor parser must parse too.
//!     Mismatches are named, not hidden.
//!
//! Byte-identity of the emitted tree is out of scope here — that is the
//! coder child's bar. This test certifies only that the parse surface is
//! complete enough to accept the whole corpus without panicking.

use std::path::PathBuf;

use endor_compile::parser::Parser;

/// Whether the endor parser accepts `src` as a Script (sloppy top level;
/// a `"use strict"` prologue upgrades from within).
fn endor_accepts(src: &str) -> bool {
    match Parser::new(src, false, false) {
        Ok(mut p) => p.parse_program(false).is_ok(),
        // A lexer error before the first token is a rejection, not a panic.
        Err(_) => false,
    }
}

/// Whether the C-XS oracle *parsed* `src` (as opposed to rejecting it with
/// a `SyntaxError`). A runtime throw still counts as "parsed".
fn oracle_parses(src: &str) -> Option<bool> {
    let outcome = endor_oracle::run(src)?;
    if outcome.completed {
        return Some(true);
    }
    // A parse rejection surfaces as an uncompleted run whose error is a
    // SyntaxError; any other error is a runtime throw of a parsed program.
    Some(!outcome.error.contains("SyntaxError"))
}

/// The corpus directory, resolved from this crate's manifest.
fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../endor-262/corpora")
}

/// The code lines of one corpus file: non-blank lines that are not
/// whole-line `//` comments.
fn code_lines(contents: &str) -> Vec<&str> {
    contents
        .lines()
        .map(str::trim_end)
        .filter(|l| {
            let t = l.trim_start();
            !t.is_empty() && !t.starts_with("//")
        })
        .collect()
}

#[test]
fn corpus_parse_smoke() {
    let dir = corpus_dir();
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read corpus dir {}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|x| x == "js").unwrap_or(false))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no corpus files under {}", dir.display());

    let mut total = 0usize;
    let mut agree_accept = 0usize;
    let mut agree_reject = 0usize;
    let mut oracle_unavailable = 0usize;
    // The consequential disagreement: the oracle parsed it but we did not.
    let mut endor_rejected_oracle_accepted: Vec<(String, String)> = Vec::new();
    // The benign direction (we accept, oracle rejects) — recorded, not fatal.
    let mut endor_accepted_oracle_rejected: Vec<(String, String)> = Vec::new();

    for file in &files {
        let name = file.file_name().unwrap().to_string_lossy().into_owned();
        let contents = std::fs::read_to_string(file).unwrap();
        for line in code_lines(&contents) {
            let oracle = match oracle_parses(line) {
                Some(v) => v,
                None => {
                    oracle_unavailable += 1;
                    continue;
                }
            };
            total += 1;
            let mine = endor_accepts(line);
            match (mine, oracle) {
                (true, true) => agree_accept += 1,
                (false, false) => agree_reject += 1,
                (false, true) => {
                    endor_rejected_oracle_accepted.push((name.clone(), line.to_string()))
                }
                (true, false) => {
                    endor_accepted_oracle_rejected.push((name.clone(), line.to_string()))
                }
            }
        }
    }

    // The named tally.
    eprintln!("corpus parse smoke: {} files, {} programs", files.len(), total);
    eprintln!("  agree/accept : {agree_accept}");
    eprintln!("  agree/reject : {agree_reject}");
    eprintln!("  endor-rejected / oracle-accepted : {}", endor_rejected_oracle_accepted.len());
    eprintln!("  endor-accepted / oracle-rejected : {}", endor_accepted_oracle_rejected.len());
    if oracle_unavailable > 0 {
        eprintln!("  (oracle unavailable on {oracle_unavailable} lines, skipped)");
    }
    for (f, l) in &endor_accepted_oracle_rejected {
        eprintln!("  ~ endor-only accept [{f}]: {l}");
    }
    for (f, l) in &endor_rejected_oracle_accepted {
        eprintln!("  ! endor rejected an oracle-accepted program [{f}]: {l}");
    }

    assert!(
        endor_rejected_oracle_accepted.is_empty(),
        "{} corpus program(s) the oracle parses were rejected by the endor parser (see above)",
        endor_rejected_oracle_accepted.len()
    );
    assert!(agree_accept > 0, "expected the corpus to contain accepted programs");
}
