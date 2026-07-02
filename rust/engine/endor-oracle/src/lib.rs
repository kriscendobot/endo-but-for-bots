//! Safe wrapper over the C-XS differential oracle.
//!
//! `endor-oracle` is the one crate in the engine workspace that is
//! allowed `unsafe`: it FFIs into C-XS (design § Minimizing `unsafe`,
//! the `endor-oracle` row) to (a) compile JavaScript source to XS
//! bytecode and (b) execute it on C-XS, returning `(bytecode, result,
//! computrons)` triples for comparison. It is dev-and-CI only and is
//! never linked into a shipped engine.
//!
//! The run-only computron count excludes parse metering (the shim
//! resets `meterIndex` after parse and reads it after run), so a
//! divergence between endor and the oracle points at the interpreter,
//! not the compiler, during stages 1 through 4.

use std::os::raw::{c_char, c_int};

// NOT #![forbid(unsafe_code)] — this crate is the audited FFI seam.

#[repr(C)]
struct EndorOracleResultRaw {
    code: *mut i8,
    code_size: u32,
    symbols: *mut i8,
    symbols_size: u32,
    computrons: u32,
    ok: u32,
    result: [u8; 1024],
    error: [u8; 256],
}

impl Default for EndorOracleResultRaw {
    fn default() -> Self {
        EndorOracleResultRaw {
            code: std::ptr::null_mut(),
            code_size: 0,
            symbols: std::ptr::null_mut(),
            symbols_size: 0,
            computrons: 0,
            ok: 0,
            result: [0u8; 1024],
            error: [0u8; 256],
        }
    }
}

extern "C" {
    fn endor_oracle_run(
        source: *const c_char,
        source_len: u32,
        out: *mut EndorOracleResultRaw,
    ) -> c_int;
    fn endor_oracle_free(out: *mut EndorOracleResultRaw);
}

/// The outcome of running one program on C-XS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OracleOutcome {
    /// The exact XS bytecode the C-XS compiler emitted for the program.
    pub bytecode: Vec<u8>,
    /// The serialized symbols atom (present when the program references
    /// symbols; empty for the pure-operator stage-1 corpus).
    pub symbols: Vec<u8>,
    /// `true` if the program completed normally; `false` if it threw or
    /// failed to parse.
    pub completed: bool,
    /// Completion value coerced with JS `String()` (valid when
    /// `completed`), else empty.
    pub result: String,
    /// The thrown value stringified (valid when `!completed`).
    pub error: String,
    /// Run-only computrons: `meterIndex >> 16` measured over execution,
    /// with parse metering excluded.
    pub computrons: u64,
}

/// Compile `source` to XS bytecode and run it on C-XS.
///
/// Returns `None` only on a machine-level failure (out of memory
/// creating the machine); a thrown exception or syntax error is a
/// normal `OracleOutcome` with `completed == false`.
pub fn run(source: &str) -> Option<OracleOutcome> {
    let bytes = source.as_bytes();
    let mut raw = EndorOracleResultRaw::default();
    // Safety: `raw` is a valid, zeroed out-parameter; the C side writes
    // only within it and heap buffers we copy out and then free.
    let rc = unsafe {
        endor_oracle_run(
            bytes.as_ptr() as *const c_char,
            bytes.len() as u32,
            &mut raw as *mut _,
        )
    };
    if rc != 0 {
        return None;
    }

    let bytecode = if raw.code.is_null() || raw.code_size == 0 {
        Vec::new()
    } else {
        // Safety: the shim malloc'd `code_size` bytes at `code`.
        unsafe {
            std::slice::from_raw_parts(raw.code as *const u8, raw.code_size as usize).to_vec()
        }
    };
    let symbols = if raw.symbols.is_null() || raw.symbols_size == 0 {
        Vec::new()
    } else {
        unsafe {
            std::slice::from_raw_parts(raw.symbols as *const u8, raw.symbols_size as usize)
                .to_vec()
        }
    };

    let outcome = OracleOutcome {
        bytecode,
        symbols,
        completed: raw.ok != 0,
        result: cstr_field(&raw.result),
        error: cstr_field(&raw.error),
        computrons: raw.computrons as u64,
    };

    // Safety: frees the heap buffers the shim allocated; we have copied
    // them into owned Vecs above.
    unsafe { endor_oracle_free(&mut raw as *mut _) };

    Some(outcome)
}

fn cstr_field(buf: &[u8]) -> String {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_arithmetic_result_and_bytecode() {
        let o = run("1 + 2").expect("machine");
        assert!(o.completed, "1+2 should complete, got error {:?}", o.error);
        assert_eq!(o.result, "3");
        assert!(!o.bytecode.is_empty(), "bytecode should be captured");
        // A trivial program still costs a handful of dispatches.
        assert!(o.computrons > 0, "run computrons should be nonzero");
    }

    #[test]
    fn boolean_logic() {
        let o = run("(1 < 2) && (3 >= 3)").expect("machine");
        assert!(o.completed);
        assert_eq!(o.result, "true");
    }

    #[test]
    fn throws_are_not_failures() {
        let o = run("throw 7").expect("machine");
        assert!(!o.completed);
    }
}
