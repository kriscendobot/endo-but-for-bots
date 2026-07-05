#![forbid(unsafe_code)]
//! endor-regexp: the safe, engine-internal transliteration of the XS
//! RegExp engine (`c/moddable` pin `xsre.c`), per the design's resolved
//! question 6 (the matcher is ported as an engine-internal module; the
//! JavaScript `RegExp` surface is child 9's integration).
//!
//! It delivers the two halves of the pin's engine:
//!
//! - [`compile`] — the `fxCompileRegExp` pipeline: recursive-descent
//!   parse into a term tree, a `measure` pass assigning each term its
//!   byte offset, and a `code` pass emitting the integer step stream.
//!   The compile meter (`XS_PARSE_REGEXP_METERING`) is carried through so
//!   child 9 can calibrate end-to-end.
//! - [`match_regexp`] — the `fxMatchRegExp` backtracking VM over that
//!   step stream, metering `XS_REGEXP_METERING` per dispatched step.
//!
//! Both are `#![forbid(unsafe_code)]`: the arena/`Vec` model removes the
//! raw pointers XS uses, so the compiler and matcher are compiler-checked
//! memory-safe. Only `endor-oracle` (the dev/CI differential harness)
//! links C.
//!
//! ## Scope and honest skips (the stage bar)
//!
//! This increment ports the core grammar over the **non-`u`/`v`,
//! non-`i`** subset: literals, `.`, character classes (`[...]` with
//! ranges, negation, `\d\D\w\W\s\S`, control/hex/`\uXXXX` escapes),
//! anchors (`^ $ \b \B`), groups (capturing and `(?:...)`), quantifiers
//! (`* + ? {n,m}`, greedy and lazy), disjunction (`|`), numeric
//! backreferences (`\1`..), and lookaround (`(?=) (?!) (?<=) (?<!)`).
//! Every deferred surface is a **named** [`compile::CompileError::Unsupported`],
//! never a wrong meter or a wrong value: the `i` flag (case folding), the
//! `u`/`v` flags (CESU-8 surrogate walk, unicode property escapes, V-mode
//! string sets), `\p{}`/`\P{}`, named captures (`(?<name>)` / `\k<name>`),
//! inline modifiers (`(?flags:)`), and astral (`> 0xFFFF`) code points.

mod encoding;
mod flags;
mod opcode;

pub mod compile;
pub mod matcher;

pub use compile::{compile, CompileError, Program};
pub use matcher::{match_regexp, MatchOutcome};
pub use flags::{
    XS_REGEXP_D, XS_REGEXP_G, XS_REGEXP_I, XS_REGEXP_M, XS_REGEXP_N, XS_REGEXP_S, XS_REGEXP_U,
    XS_REGEXP_V, XS_REGEXP_Y,
};

/// The result of compiling and running one pattern: a convenience over
/// [`compile`] + [`match_regexp`] mirroring what the oracle shim returns,
/// so a caller (and the parity suite) can compare in one shape.
#[derive(Debug, Clone)]
pub struct RunOutcome {
    /// The whole-match plus capture-group `(from, to)` byte offsets.
    pub outcome: MatchOutcome,
    /// The compiled program (counts + compile meter).
    pub program: Program,
}

/// Compile `pattern` under `flags` and match it over `subject` (a UTF-8
/// string) from byte offset `start`. Returns the compile error (syntax or
/// a named unsupported feature) on a compile failure.
pub fn run(pattern: &str, flags: &str, subject: &str, start: i32) -> Result<RunOutcome, CompileError> {
    let program = compile(pattern, flags)?;
    let outcome = match_regexp(&program, subject.as_bytes(), start);
    Ok(RunOutcome { outcome, program })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(pattern: &str, flags: &str, subject: &str) -> (bool, Vec<(i32, i32)>) {
        let r = run(pattern, flags, subject, 0).expect("compiles");
        (r.outcome.matched, r.outcome.captures)
    }

    #[test]
    fn literal_and_group() {
        let (m, c) = caps("b(c)", "", "abcd");
        assert!(m);
        assert_eq!(c[0], (1, 3));
        assert_eq!(c[1], (2, 3));
    }

    #[test]
    fn no_match() {
        let (m, c) = caps("xyz", "", "abcd");
        assert!(!m);
        assert_eq!(c[0], (-1, -1));
    }

    #[test]
    fn greedy_star() {
        let (m, c) = caps("a*", "", "aaab");
        assert!(m);
        assert_eq!(c[0], (0, 3));
    }

    #[test]
    fn lazy_star() {
        let (m, c) = caps("a*?b", "", "aaab");
        assert!(m);
        assert_eq!(c[0], (0, 4));
    }

    #[test]
    fn char_class_and_anchor() {
        let (m, c) = caps("^[0-9]+$", "", "12345");
        assert!(m);
        assert_eq!(c[0], (0, 5));
    }

    #[test]
    fn alternation() {
        let (m, c) = caps("cat|dog", "", "hotdog");
        assert!(m);
        assert_eq!(c[0], (3, 6));
    }

    #[test]
    fn backreference() {
        let (m, c) = caps("(ab)\\1", "", "abab");
        assert!(m);
        assert_eq!(c[0], (0, 4));
        assert_eq!(c[1], (0, 2));
    }

    #[test]
    fn lookahead() {
        let (m, c) = caps("a(?=b)", "", "ab");
        assert!(m);
        assert_eq!(c[0], (0, 1));
    }

    #[test]
    fn unsupported_i_flag_is_named() {
        match compile("abc", "i") {
            Err(CompileError::Unsupported(_)) => {}
            other => panic!("expected named Unsupported, got {:?}", other),
        }
    }
}
