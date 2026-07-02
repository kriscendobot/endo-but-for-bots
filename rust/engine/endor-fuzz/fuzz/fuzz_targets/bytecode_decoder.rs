//! Fuzz target 2 (design § Fuzzability): bytecode decoder fuzzing.
//! Arbitrary and truncated bytes through the decoder and interpreter
//! must degrade to a Halt::Decode, never panic (endor's loader must not
//! trust a corrupt snapshot or a buggy compiler).
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = endor_fuzz::decoder_is_panic_free(data);
});
