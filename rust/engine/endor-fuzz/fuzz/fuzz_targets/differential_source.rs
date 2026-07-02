//! Fuzz target 1 (design § Fuzzability): differential source fuzzing.
//! A structure-aware generator turns fuzzer bytes into a subset-grammar
//! program; endor and the C-XS oracle must agree bit-for-bit on
//! completion, result, and computrons.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let program = endor_fuzz::gen_program(data);
    if let Err(divergence) = endor_fuzz::differential_check(&program) {
        panic!("differential divergence vs C-XS oracle: {:?}", divergence);
    }
});
