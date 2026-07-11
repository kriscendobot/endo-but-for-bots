//! Whole-corpus parse smoke (stage-5 child 3 local bar).
//!
//! Every conformance-corpus program — the curated corpus lines, now carried
//! verbatim in the `info: Source:` frontmatter of the `endor-262/cases/`
//! tree (the `corpora/*.js` line files retired in PR #600 convergence 2/5) —
//! is parsed by the `endor-compile` parser **as a Script** and its
//! accept/reject verdict compared against the C-XS oracle
//! (`endor_oracle::run`). Two things are asserted:
//!
//!   * **Zero panics.** Every program yields a `Result`, never a panic — the
//!     invariant the parser fuzz target (a later child) depends on.
//!   * **Accept/reject agreement.** Every program the oracle *parses* (did
//!     not reject with a `SyntaxError`) the endor parser must parse too.
//!     Mismatches are named, not hidden.
//!
//! Byte-identity of the emitted tree is out of scope here — that is the
//! coder child's bar. This test certifies only that the parse surface is
//! complete enough to accept the whole corpus without panicking.

use endor_compile::parser::Parser;

mod corpus_cases;
use corpus_cases::{corpus_programs, CORPUS_PROGRAM_COUNT};

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

#[test]
fn corpus_parse_smoke() {
    let programs = corpus_programs();
    assert_eq!(
        programs.len(),
        CORPUS_PROGRAM_COUNT,
        "expected {CORPUS_PROGRAM_COUNT} corpus programs in endor-262/cases, found {}",
        programs.len()
    );

    let mut total = 0usize;
    let mut agree_accept = 0usize;
    let mut agree_reject = 0usize;
    let mut oracle_unavailable = 0usize;
    // The consequential disagreement: the oracle parsed it but we did not.
    let mut endor_rejected_oracle_accepted: Vec<(String, String)> = Vec::new();
    // The benign direction (we accept, oracle rejects) — recorded, not fatal.
    let mut endor_accepted_oracle_rejected: Vec<(String, String)> = Vec::new();

    for (id, program) in &programs {
        let line = program.as_str();
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
            (false, true) => endor_rejected_oracle_accepted.push((id.clone(), line.to_string())),
            (true, false) => endor_accepted_oracle_rejected.push((id.clone(), line.to_string())),
        }
    }

    // The named tally.
    eprintln!("corpus parse smoke: {} programs, {total} oracle-compared", programs.len());
    eprintln!("  agree/accept : {agree_accept}");
    eprintln!("  agree/reject : {agree_reject}");
    eprintln!("  endor-rejected / oracle-accepted : {}", endor_rejected_oracle_accepted.len());
    eprintln!("  endor-accepted / oracle-rejected : {}", endor_accepted_oracle_rejected.len());
    if oracle_unavailable > 0 {
        eprintln!("  (oracle unavailable on {oracle_unavailable} programs, skipped)");
    }
    for (id, l) in &endor_accepted_oracle_rejected {
        eprintln!("  ~ endor-only accept [{id}]: {l}");
    }
    for (id, l) in &endor_rejected_oracle_accepted {
        eprintln!("  ! endor rejected an oracle-accepted program [{id}]: {l}");
    }

    assert!(
        endor_rejected_oracle_accepted.is_empty(),
        "{} corpus program(s) the oracle parses were rejected by the endor parser (see above)",
        endor_rejected_oracle_accepted.len()
    );
    assert!(agree_accept > 0, "expected the corpus to contain accepted programs");
}
