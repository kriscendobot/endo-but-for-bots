//! Dual-run harness CLI: runs the stage-1 corpus (or programs passed as
//! arguments) on the C-XS oracle and endor-vm, printing per-program
//! (result, computron) agreement and a summary. Exit code is nonzero if
//! the stage-1 acceptance bar is not met, so CI can gate on it.

use endor_262::{dual_run, run_corpus, stage1_corpus, Agreement};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let programs = if args.is_empty() {
        stage1_corpus()
    } else {
        args
    };

    let (runs, summary) = run_corpus(&programs);

    println!("{:<34} {:<8} {:>12} {:>12}  {}", "program", "agree", "oracle¢", "endor¢", "result");
    println!("{}", "-".repeat(86));
    for r in &runs {
        let mark = if r.is_bit_exact() { "ok" } else { "DIVERGE" };
        let agree = match r.agreement {
            Agreement::BothComplete => "both",
            Agreement::BothAbort => "abort",
            Agreement::EndorOnlyComplete => "endor!",
            Agreement::OracleOnlyComplete => "oracle!",
        };
        println!(
            "{:<34} {:<8} {:>12} {:>12}  {} [{}]",
            truncate(&r.source, 34),
            agree,
            r.oracle_computrons,
            r.endor_computrons,
            r.oracle_result,
            mark,
        );
        if std::env::var("ENDOR_SHOW_RAW").is_ok() {
            println!("    raw oracle={} endor={} gap={}",
                r.oracle_meter_raw, r.endor_meter_raw,
                r.oracle_meter_raw as i64 - r.endor_meter_raw as i64);
        }
        if !r.is_bit_exact() {
            println!("    oracle_result={:?} endor_result={:?} endor_dispatched={} halt={:?}",
                r.oracle_result, r.endor_result, r.endor_dispatched, r.endor_halt);
            println!("    oracle_raw={} endor_raw={} raw_gap={}",
                r.oracle_meter_raw, r.endor_meter_raw,
                r.oracle_meter_raw as i64 - r.endor_meter_raw as i64);
            println!("    bytecode={:02x?}", r.bytecode);
        }
    }
    let _ = dual_run; // keep the single-program entry reachable
    println!("{}", "-".repeat(86));
    println!(
        "total={} bit_exact={} result_div={} computron_div={} completion_div={} unsupported={}",
        summary.total,
        summary.bit_exact,
        summary.result_divergences,
        summary.computron_divergences,
        summary.completion_divergences,
        summary.unsupported,
    );
    if summary.met_bar() {
        println!("ACCEPTANCE BAR MET: {}/{} bit-exact (result, computron) agreement with the oracle", summary.bit_exact, summary.total);
    } else {
        println!("ACCEPTANCE BAR NOT MET");
        std::process::exit(1);
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n { s.to_string() } else { format!("{}…", &s[..n - 1]) }
}
