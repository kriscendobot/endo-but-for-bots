//! Stage-5 compile-differential fuzz target (design § roadmap row 5,
//! Fuzzability): `endor_compile::compile` vs the C-XS oracle compiler on
//! identical source — accept/reject agreement and, on a shared accept,
//! byte identity. An oracle process crash is a NAMED outcome
//! (`OracleUnavailable`), not a harness abort; a coder fold is
//! `EndorRejected`. A real `ByteDivergence` or an endor-only accept
//! (`OracleRejected`) is the finding.
#![no_main]
use endor_fuzz::CompileFuzzOutcome;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let program = endor_fuzz::gen_compile_program(data);
    match endor_fuzz::compile_differential_check(&program) {
        CompileFuzzOutcome::ByteDivergence { detail } => {
            panic!("compile byte divergence vs C-XS oracle on {program:?}: {detail}");
        }
        CompileFuzzOutcome::OracleRejected => {
            panic!("endor compiled a program the C-XS oracle rejected: {program:?}");
        }
        // Identical / BothReject / EndorRejected (coder fold) /
        // OracleUnavailable are all valid, non-finding outcomes.
        _ => {}
    }
});
