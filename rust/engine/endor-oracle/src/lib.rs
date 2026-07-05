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
    meter_raw: u32,
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
            meter_raw: 0,
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
    fn endor_oracle_regexp(
        pattern: *const c_char,
        modifier: *const c_char,
        subject: *const c_char,
        subject_len: u32,
        start: i32,
        out: *mut EndorRegExpResultRaw,
    ) -> c_int;
}

const ENDOR_MAX_CAPTURES: usize = 64;

#[repr(C)]
struct EndorRegExpResultRaw {
    ok: u32,
    matched: u32,
    capture_count: u32,
    name_count: u32,
    captures: [i32; 2 * ENDOR_MAX_CAPTURES],
    compile_computrons: u32,
    compile_meter_raw: u32,
    match_computrons: u32,
    match_meter_raw: u32,
    error: [u8; 256],
}

impl Default for EndorRegExpResultRaw {
    fn default() -> Self {
        EndorRegExpResultRaw {
            ok: 0,
            matched: 0,
            capture_count: 0,
            name_count: 0,
            captures: [-1; 2 * ENDOR_MAX_CAPTURES],
            compile_computrons: 0,
            compile_meter_raw: 0,
            match_computrons: 0,
            match_meter_raw: 0,
            error: [0u8; 256],
        }
    }
}

/// The reference outcome of compiling and running one XSRE pattern on
/// the C-XS matcher (`fxCompileRegExp` + `fxMatchRegExp`).
///
/// Offsets are in the subject's UTF-8/CESU-8 **byte** space — the same
/// space the Rust port operates in — so a fixture compares directly
/// with no UTF-16 conversion. `captures[0]` is `(from, to)` of the
/// whole match; `captures[i]` is capture group `i`; an unset capture is
/// `(-1, -1)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegExpOutcome {
    /// `true` if the pattern compiled; `false` on a syntax error
    /// (message in `error`).
    pub compiled: bool,
    /// `true` if the pattern matched at or after `start`.
    pub matched: bool,
    /// Total captures including the whole match at index 0 (`code[1]`).
    pub capture_count: u32,
    /// Named-capture count (`code[2]`).
    pub name_count: u32,
    /// `(from, to)` byte-offset pairs, `capture_count` of them; an
    /// unset capture is `(-1, -1)`.
    pub captures: Vec<(i32, i32)>,
    /// Compile meter: `XS_PARSE_REGEXP_METERING * pattern size >> 16`.
    pub compile_computrons: u64,
    /// Raw compile meter (16.16 fixed point).
    pub compile_meter_raw: u32,
    /// Match meter: `XS_REGEXP_METERING` per step dispatched, `>> 16`.
    pub match_computrons: u64,
    /// Raw match meter (16.16 fixed point).
    pub match_meter_raw: u32,
    /// Compile error message (valid when `!compiled`).
    pub error: String,
}

/// Compile `pattern` with `flags` (the modifier string, e.g. `"gi"`)
/// and run the XSRE matcher over `subject` starting at byte offset
/// `start`, returning the matcher's reference behavior.
///
/// Returns `None` only on a machine-level failure (out of memory
/// creating the machine); a syntax error is a normal `RegExpOutcome`
/// with `compiled == false`.
pub fn regexp(pattern: &str, flags: &str, subject: &str, start: i32) -> Option<RegExpOutcome> {
    let pattern_c = std::ffi::CString::new(pattern).ok()?;
    let flags_c = std::ffi::CString::new(flags).ok()?;
    // A JS string may contain a literal NUL, which CString rejects; pass
    // bytes + a trailing NUL and let the matcher scan to it (the byte
    // length is carried in the ABI for the caller's own assertions).
    let mut subject_bytes = subject.as_bytes().to_vec();
    subject_bytes.push(0);
    let mut raw = EndorRegExpResultRaw::default();
    // Safety: all pointers are valid for the duration of the call; the C
    // side writes only within `raw`.
    let rc = unsafe {
        endor_oracle_regexp(
            pattern_c.as_ptr(),
            flags_c.as_ptr(),
            subject_bytes.as_ptr() as *const c_char,
            (subject_bytes.len() - 1) as u32,
            start,
            &mut raw as *mut _,
        )
    };
    if rc != 0 {
        return None;
    }
    let n = (raw.capture_count as usize).min(ENDOR_MAX_CAPTURES);
    let captures = (0..n)
        .map(|i| (raw.captures[2 * i], raw.captures[2 * i + 1]))
        .collect();
    Some(RegExpOutcome {
        compiled: raw.ok != 0,
        matched: raw.matched != 0,
        capture_count: raw.capture_count,
        name_count: raw.name_count,
        captures,
        compile_computrons: raw.compile_computrons as u64,
        compile_meter_raw: raw.compile_meter_raw,
        match_computrons: raw.match_computrons as u64,
        match_meter_raw: raw.match_meter_raw,
        error: cstr_field(&raw.error),
    })
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
    /// Raw run-only meterIndex (16.16 fixed point), for diagnosing
    /// fractional (built-in step) metering.
    pub meter_raw: u32,
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
        meter_raw: raw.meter_raw,
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

    #[test]
    fn regexp_literal_match_captures_and_meter() {
        // /b(c)/ over "abcd": whole match "bc" at bytes [1,3), group 1
        // "c" at [2,3). One capture group plus the whole match.
        let o = regexp("b(c)", "", "abcd", 0).expect("machine");
        assert!(o.compiled, "should compile: {}", o.error);
        assert!(o.matched);
        assert_eq!(o.capture_count, 2);
        assert_eq!(o.captures[0], (1, 3));
        assert_eq!(o.captures[1], (2, 3));
        // The matcher dispatches at least one step, so the match meter
        // is nonzero.
        assert!(o.match_meter_raw > 0, "match meter should be nonzero");
        assert!(o.compile_meter_raw > 0, "compile meter should be nonzero");
    }

    #[test]
    fn regexp_no_match_reports_unset() {
        let o = regexp("xyz", "", "abcd", 0).expect("machine");
        assert!(o.compiled);
        assert!(!o.matched);
        // The whole-match capture is unset on a miss.
        assert_eq!(o.captures[0], (-1, -1));
    }

    #[test]
    fn regexp_syntax_error_is_not_a_failure() {
        // An unterminated group is a compile error, reported as
        // compiled == false with a message — not a machine failure.
        let o = regexp("(", "", "abc", 0).expect("machine");
        assert!(!o.compiled);
        assert!(!o.error.is_empty(), "should carry an error message");
    }

    #[test]
    fn regexp_start_offset_is_honored() {
        // Starting the scan past the first "a" finds the second one.
        let o = regexp("a", "", "aba", 1).expect("machine");
        assert!(o.matched);
        assert_eq!(o.captures[0], (2, 3));
    }
}
