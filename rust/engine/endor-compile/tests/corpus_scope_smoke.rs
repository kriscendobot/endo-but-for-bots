//! Whole-corpus scope smoke (stage-5 child 4 robustness bar).
//!
//! The scoper runs its two passes (hoist + bind) over every
//! conformance-corpus program the parser accepts, asserting it never
//! panics and returns cleanly (either a scope tree or a classified early
//! error). This mirrors the parser child's parse-smoke: the coder child
//! and the eventual fuzz target lean on the scoper being total over the
//! parser's output, so a panic here is a defect. It does **not** compare
//! scope shapes against an oracle (C-XS exposes no scope dump); the
//! shape/numbering contract is pinned by the unit fixtures in
//! `src/scoper/tests.rs`.

use std::path::PathBuf;

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../endor-262/corpora")
}

/// A program line worth scoping: non-blank and not a whole-line comment.
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

/// True if the parser accepts `src` as a Script (the scoper only runs on
/// parser-accepted programs).
fn parses(src: &str) -> bool {
    match endor_compile::Parser::new(src, false, false) {
        Ok(mut p) => p.parse_program(false).is_ok(),
        Err(_) => false,
    }
}

#[test]
fn corpus_scope_smoke() {
    let dir = corpus_dir();
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read corpus dir {}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|x| x == "js").unwrap_or(false))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no corpus files under {}", dir.display());

    let mut scoped = 0usize;
    let mut early_errors = 0usize;
    for file in &files {
        let contents = std::fs::read_to_string(file).unwrap();
        for line in code_lines(&contents) {
            if !parses(line) {
                continue;
            }
            // A panic here fails the test (the point of the smoke).
            match endor_compile::scope_program(line, false) {
                Ok(_) => scoped += 1,
                Err(_) => early_errors += 1,
            }
        }
    }
    eprintln!("corpus scope smoke: {} files, {scoped} scoped, {early_errors} early errors", files.len());
    assert!(scoped > 0, "expected to scope at least one corpus program");
}
