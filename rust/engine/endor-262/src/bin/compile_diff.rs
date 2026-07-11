//! Full-corpus **byte-identity differential** runner (stage-5 child 7/7,
//! the STAGE BAR). For every source file in a test262 subtree (or the
//! curated corpora), where the C-XS oracle compiler accepts the file,
//! asserts `endor_compile::compile(src)` == `endor_oracle::run(src).bytecode`
//! byte for byte, and prints the honest
//! `total / identical / divergent / oracle-rejected / endor-rejected`
//! split with NAMED divergence classes and per-file identification.
//!
//! Usage:
//!   cargo run -p endor-262 --bin compile-diff                       # curated corpora (bounded)
//!   cargo run -p endor-262 --bin compile-diff -- language/expressions/addition
//!   cargo run -p endor-262 --bin compile-diff -- built-ins/Boolean
//!
//! Memory note (same as `endor-xst`): the C-XS oracle accumulates
//! process RSS across the machine create/destroy cycles a whole-tree run
//! makes, so walking all of `language/` in one process can exhaust RAM.
//! Run it **per subtree**; each subprocess frees everything on exit.
//!
//! Exit code is nonzero on any bar violation (a byte divergence or an
//! accept/reject disagreement), so CI/nightly can gate on it.

use endor_262::compile_diff::{
    collect_js, compile_diff_files, corpora_programs, compile_diff_programs, print_report,
};
use endor_262::test262::locate_test262;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let (report, label) = if args.is_empty() {
        // Default: the bounded curated corpora (no test262 subset needed).
        let programs = corpora_programs();
        (compile_diff_programs(&programs), "corpora".to_string())
    } else {
        let (root, _harness) = match locate_test262() {
            Some(p) => p,
            None => {
                eprintln!("test262 subset not found under packages/test262-runner/test262");
                std::process::exit(2);
            }
        };
        let sub = &args[0];
        let base = if sub.starts_with("language") || sub.starts_with("built-ins") {
            root.join(sub)
        } else {
            root.join("language").join(sub)
        };
        let files = collect_js(&base);
        if files.is_empty() {
            eprintln!("no test files under {}", base.display());
            std::process::exit(2);
        }
        eprintln!("compiling {} files under {}", files.len(), base.display());
        (compile_diff_files(&files), base.display().to_string())
    };

    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    print_report(&mut lock, &report, &label).unwrap();

    if !report.met_bar() {
        std::process::exit(1);
    }
}
