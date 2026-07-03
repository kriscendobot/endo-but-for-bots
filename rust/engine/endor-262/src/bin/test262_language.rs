//! Full test262 `language/` dual-run runner (design § test262 conformance,
//! the stage-2 acceptance bar). Walks the whole checked-in `language/` tree
//! (or a `language/<subpath>` given as an argument), assembles each test the
//! standard test262 way, dual-runs it on the C-XS oracle and endor-vm, and
//! prints the honest covered/skipped/divergent split — every skip named by
//! the opcode/feature that stopped endor, never folded into a pass rate.
//!
//! Usage:
//!   cargo run -p endor-262 --bin test262-language                 # all of language/
//!   cargo run -p endor-262 --bin test262-language -- expressions/addition
//!
//! Exit code is nonzero on any divergence (endor completing with a wrong
//! value/computron, or accepting what XS rejects), so CI/nightly can gate.
//!
//! Memory note: the C-XS oracle accumulates process memory across the tens
//! of thousands of machine create/destroy cycles a whole-tree run makes, so
//! walking all of `language/` (~20.6k files) in one process can exhaust RAM.
//! Run it per subtree — `expressions`, then `statements`, … — to bound the
//! working set; each subprocess frees everything on exit. The in-crate
//! `covered_grammar_language_subset_has_zero_divergence` test drives the
//! covered-grammar sections (a fast, bounded slice) as the CI gate.

use endor_262::test262::{collect_js, locate_test262, run_files};

fn main() {
    let (root, harness) = match locate_test262() {
        Some(p) => p,
        None => {
            eprintln!("test262 subset not found under packages/test262-runner/test262");
            std::process::exit(2);
        }
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let sub = args.first().map(|s| s.as_str()).unwrap_or("language");
    let base = if sub.starts_with("language") {
        root.join(sub)
    } else {
        root.join("language").join(sub)
    };

    let files = collect_js(&base);
    if files.is_empty() {
        eprintln!("no test files under {}", base.display());
        std::process::exit(2);
    }
    eprintln!("running {} test262 files under {}", files.len(), base.display());
    let rep = run_files(&harness, &root, &files);

    println!("{}", "=".repeat(72));
    println!(
        "test262 language/ dual-run: total={} covered={} divergent={} skipped={}",
        rep.total,
        rep.covered,
        rep.divergences.len(),
        rep.total - rep.covered - rep.divergences.len(),
    );
    println!("{}", "-".repeat(72));
    println!("skipped-by-reason (honest split — named, not folded into a pass rate):");
    for (reason, n) in rep.skip_summary() {
        println!("  {:>6}  {}", n, reason);
    }
    if !rep.divergences.is_empty() {
        println!("{}", "-".repeat(72));
        println!("DIVERGENCES ({}):", rep.divergences.len());
        for (path, detail) in &rep.divergences {
            println!("  {}\n    {}", path, detail);
        }
    }
    println!("{}", "=".repeat(72));
    if rep.met_bar() {
        println!(
            "BAR MET: {} covered bit-exact, 0 divergent (of {} total; {} skipped by named reason)",
            rep.covered,
            rep.total,
            rep.total - rep.covered
        );
    } else {
        println!("BAR NOT MET: {} divergence(s)", rep.divergences.len());
        std::process::exit(1);
    }
}
