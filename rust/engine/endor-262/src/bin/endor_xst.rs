//! `endor-xst`: the xst-analogue test262 runner (design
//! [`designs/xs2rust-endor-test262-convergence.md`] § Part 2). It plays for
//! the Rust engine the role `xs/tools/xst.c` + `xst262.c` play for C-XS —
//! full YAML frontmatter, the not-yet-implemented feature skip list,
//! sloppy+strict mode selection, negative verdicts, the xst-shaped `-o` YAML
//! report — plus the one thing `xst` never had: a differential oracle
//! (`--oracle`, default on) whose verdict + observable agreement gate the
//! build and whose computron comparison is advisory.
//!
//! It subsumes `test262-language`: the same positional-subtree walk, the
//! same per-subtree-process memory bounding, the same honest covered /
//! skipped / divergent split — now with the full verdict and report layer.
//!
//! Usage:
//!   endor-xst                                  # all of language/ (default)
//!   endor-xst expressions/addition             # a language/ subtree
//!   endor-xst built-ins/Boolean                # a built-ins/ subtree
//!   endor-xst --features-include ses-xs-parity built-ins/Object
//!   endor-xst --repeat 3 --gate-meter-exact language/expressions/addition
//!   endor-xst -o report.yaml language/statements
//!   endor-xst --test262-dir /path/to/test262 --no-oracle built-ins/Math
//!
//! Positional paths are subtrees under the located test262 root; a bare path
//! defaults under `language/` for back-compat with `test262-language`. Exit
//! code is nonzero on any failure (a divergence, an over-acceptance, a
//! meter-exact-gate or determinism violation), so CI/nightly can gate.
//!
//! Memory note (inherited from `test262-language`): the C-XS oracle
//! accumulates process memory across the tens of thousands of machine
//! create/destroy cycles a whole-tree run makes, so walk one subtree per
//! process to bound the working set; each subprocess frees everything on
//! exit.

use endor_262::test262::{collect_js, locate_test262};
use endor_262::xst::{run_files, Config};
use std::path::PathBuf;

fn main() {
    let mut cfg = Config::default();
    let mut test262_dir: Option<PathBuf> = None;
    let mut report_path: Option<PathBuf> = None;
    let mut subtrees: Vec<String> = Vec::new();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--oracle" => cfg.oracle = true,
            "--no-oracle" => cfg.oracle = false,
            "--gate-meter-exact" => cfg.gate_meter_exact = true,
            "--repeat" => {
                cfg.repeat = args
                    .next()
                    .and_then(|n| n.parse().ok())
                    .filter(|&n| n >= 1)
                    .unwrap_or_else(|| fail("--repeat needs a positive integer"));
            }
            "--features-include" => {
                let v = args
                    .next()
                    .unwrap_or_else(|| fail("--features-include needs a feature"));
                // Comma-separated or repeated: both accepted.
                cfg.features_include.extend(
                    v.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty()),
                );
            }
            "--test262-dir" => {
                test262_dir = Some(PathBuf::from(
                    args.next()
                        .unwrap_or_else(|| fail("--test262-dir needs a path")),
                ));
            }
            "-o" | "--report" => {
                report_path = Some(PathBuf::from(
                    args.next().unwrap_or_else(|| fail("-o needs a path")),
                ));
            }
            "-h" | "--help" => {
                print!("{}", HELP);
                return;
            }
            other if other.starts_with('-') => fail(&format!("unknown flag: {}", other)),
            other => subtrees.push(other.to_string()),
        }
    }

    // Locate the test262 root + harness dir: the `--test262-dir` override, or
    // the checked-in `packages/test262-runner/test262` subset.
    let (root, harness) = match test262_dir {
        Some(dir) => {
            let harness = dir.join("harness");
            if !harness.join("sta.js").is_file() {
                fail(&format!("no test262 harness under {}", harness.display()));
            }
            (dir.join("test"), harness)
        }
        None => match locate_test262() {
            Some(p) => p,
            None => fail("test262 subset not found under packages/test262-runner/test262"),
        },
    };

    // Default to all of `language/` (test262-language's default); a bare
    // subpath resolves under `language/`, `language/…` and `built-ins/…`
    // from the root.
    if subtrees.is_empty() {
        subtrees.push("language".to_string());
    }

    let mut files = Vec::new();
    for sub in &subtrees {
        let base = if sub.starts_with("language") || sub.starts_with("built-ins") {
            root.join(sub)
        } else {
            root.join("language").join(sub)
        };
        let found = collect_js(&base);
        if found.is_empty() {
            eprintln!("warning: no test files under {}", base.display());
        }
        files.extend(found);
    }
    files.sort();
    files.dedup();
    if files.is_empty() {
        fail("no test files to run");
    }

    eprintln!(
        "endor-xst: running {} test262 files (oracle={}, repeat={}, gate-meter-exact={})",
        files.len(),
        cfg.oracle,
        cfg.repeat,
        cfg.gate_meter_exact,
    );
    let rep = run_files(&cfg, &harness, &root, &files);

    println!("{}", "=".repeat(72));
    println!(
        "endor-xst: total={} covered={} failed={} skipped={}",
        rep.total,
        rep.covered,
        rep.failures.len(),
        rep.total - rep.covered - rep.failures.len(),
    );
    println!(
        "  mode: sloppy-run={} strict-skipped-unimplemented={}",
        rep.sloppy_run, rep.strict_skipped
    );
    println!("  advisory: computron-gap={}", rep.computron_advisories);
    println!("{}", "-".repeat(72));
    println!("skip: (pre-run feature/flag/structural skips — named, not folded into a rate)");
    for (reason, n) in rep.skip_summary() {
        println!("  {:>6}  {}", n, reason);
    }
    println!("skip-detail: (post-run honest split by opcode/value/reason)");
    for (reason, n) in rep.skip_detail_summary() {
        println!("  {:>6}  {}", n, reason);
    }
    if !rep.failures.is_empty() {
        println!("{}", "-".repeat(72));
        println!("fail: ({}):", rep.failures.len());
        for (path, detail) in &rep.failures {
            println!("  {}\n    {}", path, detail);
        }
    }
    println!("{}", "=".repeat(72));

    if let Some(path) = &report_path {
        match std::fs::write(path, rep.to_yaml()) {
            Ok(()) => eprintln!("wrote report to {}", path.display()),
            Err(e) => fail(&format!(
                "could not write report to {}: {}",
                path.display(),
                e
            )),
        }
    }

    if rep.met_bar() {
        println!(
            "BAR MET: {} covered, 0 failed (of {} total; {} skipped by named reason)",
            rep.covered,
            rep.total,
            rep.total - rep.covered
        );
    } else {
        println!("BAR NOT MET: {} failure(s)", rep.failures.len());
        std::process::exit(1);
    }
}

fn fail(msg: &str) -> ! {
    eprintln!("endor-xst: {}", msg);
    std::process::exit(2);
}

const HELP: &str = "\
endor-xst: the xst-analogue test262 runner (design § Part 2)

USAGE:
    endor-xst [OPTIONS] [SUBTREE...]

SUBTREE:
    A subtree under the test262 root. `language/…` and `built-ins/…` resolve
    from the root; a bare path resolves under `language/`. Default: language/

OPTIONS:
    --oracle                 gate on C-XS oracle agreement (default on)
    --no-oracle              do not gate on oracle agreement
    --gate-meter-exact       fail endor-meter-exact cases on a computron drift
    --repeat N               re-run endor N times; require identical computrons
    --features-include F[,F] opt features back into the run (e.g. ses-xs-parity)
    --test262-dir DIR        use DIR as the test262 root (has harness/, test/)
    -o, --report FILE        write the xst-shaped YAML report to FILE
    -h, --help               print this help
";
