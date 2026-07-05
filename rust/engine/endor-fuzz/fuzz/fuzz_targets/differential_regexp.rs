//! Fuzz target (stage-3b XSRE, child 8/9): differential regexp fuzzing.
//! A structure-aware generator turns fuzzer bytes into a supported-grammar
//! pattern + subject; the endor-regexp matcher and the C-XS pin
//! (`fxCompileRegExp` + `fxMatchRegExp`) must agree bit-for-bit on the
//! matched answer, every capture's byte offsets, and the per-step match
//! meter.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let case = endor_fuzz::gen_regexp(data);
    if let Err(divergence) = endor_fuzz::differential_check_regexp(&case) {
        panic!("regexp differential divergence vs C-XS pin: {:?}", divergence);
    }
});
