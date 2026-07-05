//! The XSRE pattern compiler: a faithful transliteration of the
//! `fxCompileRegExp` pipeline in `xsre.c` — recursive-descent parse into
//! a term tree, a `measure` pass that assigns each term its byte offset
//! in the code array, and a `code` pass that emits the integer step
//! stream the [`crate::matcher`] VM interprets.
//!
//! Offsets (`step`, `completion`, `loop_off`, `sequel`) are kept in
//! **bytes** exactly as C-XS keeps them (`sizeof(txInteger) == 4`), so
//! the compile meter (`parser->size * XS_PARSE_REGEXP_METERING`) is
//! bit-exact and the emitted graph is structurally identical, which in
//! turn makes the matcher's per-step meter bit-exact.
//!
//! Honest scope (the stage bar names deferred surfaces): the `i`, `u`,
//! and `v` flags, `\p{}`/`\P{}` unicode property escapes, named captures
//! (`(?<name>)` / `\k<name>`), inline modifiers (`(?flags:...)`), and
//! astral (`> 0xFFFF`) code points are compiled to a named
//! [`CompileError::Unsupported`], never to a wrong meter or a wrong
//! value.

use crate::flags::*;
use crate::opcode::*;
use crate::encoding::{utf8_decode, C_EOF};

/// Why a pattern did not compile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompileError {
    /// A genuine syntax error — the same outcome C-XS reports through
    /// `fxCompileRegExp` returning 0. The string is a short reason.
    Syntax(String),
    /// A pin feature this stage has not ported yet. Named, never a wrong
    /// answer (the stage's honest-skip bar).
    Unsupported(&'static str),
}

/// A compiled pattern: the integer code array plus the derived counts
/// the matcher and its scratch data need, and the compile-phase meter.
#[derive(Debug, Clone)]
pub struct Program {
    /// The emitted step stream. Byte offset `b` lives at word `b / 4`.
    pub code: Vec<i32>,
    /// `code[1]`: total captures including the whole match at index 0.
    pub capture_count: usize,
    /// `code[2]`: named-capture count.
    pub name_count: usize,
    /// `code[3]`: assertion (lookaround) count.
    pub assertion_count: usize,
    /// `code[4]`: quantifier count.
    pub quantifier_count: usize,
    /// Compile meter in raw 16.16 fixed point:
    /// `size_bytes * XS_PARSE_REGEXP_METERING`.
    pub compile_meter_raw: u64,
}

impl Program {
    /// `compile_meter_raw >> 16`, the integer compile computrons.
    pub fn compile_computrons(&self) -> u64 {
        self.compile_meter_raw >> 16
    }
    /// The flags word the compiler resolved (`code[0]`), which may add
    /// `XS_REGEXP_N` when the pattern declares a named capture.
    pub fn flags(&self) -> u32 {
        self.code[0] as u32
    }
}

type NodeId = usize;

/// A term-tree node. Layout fields (`step`/`completion`/`loop_off`) are
/// filled by [`Compiler::measure`] and read by [`Compiler::code`],
/// mirroring the `txTermPart` header and the per-term struct fields.
#[derive(Debug)]
struct Node {
    kind: Kind,
    step: i32,
    completion: i32,
    loop_off: i32,
}

#[derive(Debug)]
enum Kind {
    /// A sorted list of half-open ranges: `chars[0]` = endpoint count,
    /// then `chars[1..=count]` = `[b0,e0), [b1,e1), ...`.
    CharSet { chars: Vec<i32> },
    Empty,
    LineBegin,
    LineEnd,
    WordBreak,
    WordContinue,
    Disjunction { left: NodeId, right: NodeId },
    Sequence { left: NodeId, right: NodeId },
    Capture { term: NodeId, capture_index: i32 },
    CaptureReference { capture_index: i32 },
    Assertion { term: NodeId, not: bool, direction: i32, assertion_index: i32 },
    Quantifier {
        term: NodeId,
        min: i32,
        max: i32,
        greedy: bool,
        capture_index: i32,
        capture_count: i32,
        quantifier_index: i32,
    },
}

/// The XS `0x7FFFFFFF` open-max sentinel (`*`, `+`, `{n,}`).
const MAX_QUANTIFIER: i32 = 0x7FFF_FFFF;

struct Compiler {
    pattern: Vec<u8>, // NUL-terminated
    offset: usize,
    character: i64,
    flags: u32,
    capture_index: i32,
    name_index: i32,
    assertion_index: i32,
    quantifier_index: i32,
    size: i64, // parser->size, in bytes
    nodes: Vec<Node>,
    code: Vec<i32>,
}

type PResult<T> = Result<T, CompileError>;

/// Compile `pattern` under the `flags` modifier string (e.g. `"gm"`).
///
/// Returns the [`Program`] on success, or a [`CompileError`] — a genuine
/// syntax error, or a named unsupported feature (the stage's honest
/// skip).
pub fn compile(pattern: &str, flags: &str) -> PResult<Program> {
    let mut parser_flags: u32 = 0;
    // Flag modifier parse (fxCompileRegExp head).
    for c in flags.bytes() {
        match c {
            b'g' if parser_flags & XS_REGEXP_G == 0 => parser_flags |= XS_REGEXP_G,
            b'i' if parser_flags & XS_REGEXP_I == 0 => parser_flags |= XS_REGEXP_I,
            b'm' if parser_flags & XS_REGEXP_M == 0 => parser_flags |= XS_REGEXP_M,
            b's' if parser_flags & XS_REGEXP_S == 0 => parser_flags |= XS_REGEXP_S,
            b'u' if parser_flags & (XS_REGEXP_U | XS_REGEXP_V) == 0 => parser_flags |= XS_REGEXP_U,
            b'y' if parser_flags & XS_REGEXP_Y == 0 => parser_flags |= XS_REGEXP_Y,
            b'd' if parser_flags & XS_REGEXP_D == 0 => parser_flags |= XS_REGEXP_D,
            b'v' if parser_flags & (XS_REGEXP_U | XS_REGEXP_V) == 0 => parser_flags |= XS_REGEXP_V,
            _ => return Err(CompileError::Syntax("invalid flags".into())),
        }
    }
    // Honest-skip the flags whose match/compile machinery (case folding,
    // CESU-8 surrogate walk, unicode property sets, V-mode string sets)
    // is a named later increment.
    if parser_flags & XS_REGEXP_I != 0 {
        return Err(CompileError::Unsupported("i flag (case folding)"));
    }
    if parser_flags & (XS_REGEXP_U | XS_REGEXP_V) != 0 {
        return Err(CompileError::Unsupported("u/v flag (unicode)"));
    }

    let mut pattern_bytes = pattern.as_bytes().to_vec();
    pattern_bytes.push(0);
    let mut c = Compiler {
        pattern: pattern_bytes,
        offset: 0,
        character: 0,
        flags: parser_flags,
        capture_index: 0,
        name_index: 0,
        assertion_index: 0,
        quantifier_index: 0,
        size: 0,
        nodes: Vec::new(),
        code: Vec::new(),
    };
    c.compile_pattern()
}

impl Compiler {
    fn compile_pattern(&mut self) -> PResult<Program> {
        self.next()?;
        let term = self.disjunction_parse(C_EOF)?;
        // Named captures would set XS_REGEXP_N and force a re-parse; this
        // increment honest-skips them before reaching here.
        self.capture_index += 1;
        // parser->size = (5 + nameIndex) * sizeof(txInteger).
        self.size = (5 + self.name_index as i64) * 4;
        self.measure(term, 1);

        // Compile meter fires here in C, over the final size (before the
        // trailing match word is accounted, matching fxCompileRegExp).
        let match_offset = self.size;
        self.size += 4;
        let compile_meter_raw = (self.size as u64) * XS_PARSE_REGEXP_METERING;

        // Allocate and zero the code buffer.
        let total_words = (self.size / 4) as usize;
        self.code = vec![0; total_words];
        self.code[0] = self.flags as i32;
        self.code[1] = self.capture_index;
        self.code[2] = self.name_index;
        self.code[3] = self.assertion_index;
        self.code[4] = self.quantifier_index;
        // Named-capture id slots [5 .. 5+nameIndex) stay 0 here (only the
        // JS surface reads them; the matcher does not).

        self.emit(term, 1, match_offset as i32);
        self.code[(match_offset / 4) as usize] = CX_MATCH_STEP;

        Ok(Program {
            code: std::mem::take(&mut self.code),
            capture_count: self.capture_index as usize,
            name_count: self.name_index as usize,
            assertion_count: self.assertion_index as usize,
            quantifier_count: self.quantifier_index as usize,
            compile_meter_raw,
        })
    }

    // ---- pattern lexer primitives (fxPatternParser*) ----

    fn read8(&self, offset: usize) -> u8 {
        self.pattern.get(offset).copied().unwrap_or(0)
    }

    /// `fxPatternParserNext`, the non-`UV` BMP path. Astral code points
    /// (which C splits into surrogates) are the named skip here.
    fn next(&mut self) -> PResult<()> {
        let (ch, p) = utf8_decode(&self.pattern, self.offset);
        if ch != C_EOF {
            self.offset = p;
            if ch > 0xFFFF {
                return Err(CompileError::Unsupported("astral code point in pattern"));
            }
            self.character = ch;
        } else {
            self.character = C_EOF;
        }
        Ok(())
    }

    /// `fxPatternParserDecimal`: fold the current digit into `value`.
    fn decimal(&self, value: &mut u32) -> bool {
        let c = self.character;
        if (b'0' as i64..=b'9' as i64).contains(&c) {
            *value = value.wrapping_mul(10).wrapping_add((c - b'0' as i64) as u32);
            true
        } else {
            false
        }
    }

    fn error(&self, msg: &str) -> CompileError {
        CompileError::Syntax(msg.to_string())
    }

    fn add_node(&mut self, kind: Kind) -> NodeId {
        self.nodes.push(Node { kind, step: 0, completion: 0, loop_off: 0 });
        self.nodes.len() - 1
    }

    // ---- character-set builders (fxCharSet*) ----

    fn charset_single(&mut self, character: i64) -> NodeId {
        self.add_node(Kind::CharSet {
            chars: vec![2, character as i32, (character + 1) as i32],
        })
    }

    fn charset_empty(&mut self) -> NodeId {
        self.add_node(Kind::CharSet { chars: vec![0] })
    }

    fn charset_any(&mut self) -> NodeId {
        let chars = if self.flags & XS_REGEXP_S != 0 {
            vec![2, 0x0000, 0x7FFF_FFFF]
        } else {
            vec![8, 0x0000, 0x000A, 0x000B, 0x000D, 0x000E, 0x2028, 0x2030, 0x7FFF_FFFF]
        };
        self.add_node(Kind::CharSet { chars })
    }

    fn charset_digits(&mut self) -> NodeId {
        self.add_node(Kind::CharSet { chars: vec![2, b'0' as i32, b'9' as i32 + 1] })
    }

    fn charset_words(&mut self) -> NodeId {
        // Non-`i` path (the `i` path is the deferred fold increment).
        self.add_node(Kind::CharSet {
            chars: vec![
                8,
                b'0' as i32, b'9' as i32 + 1,
                b'A' as i32, b'Z' as i32 + 1,
                b'_' as i32, b'_' as i32 + 1,
                b'a' as i32, b'z' as i32 + 1,
            ],
        })
    }

    fn charset_spaces(&mut self) -> NodeId {
        self.add_node(Kind::CharSet {
            chars: vec![
                20, 0x0009, 0x000D + 1, 0x0020, 0x0020 + 1, 0x00A0, 0x00A0 + 1, 0x1680, 0x1680 + 1,
                0x2000, 0x200A + 1, 0x2028, 0x2029 + 1, 0x202F, 0x202F + 1, 0x205F, 0x205F + 1,
                0x3000, 0x3000 + 1, 0xFEFF, 0xFEFF + 1,
            ],
        })
    }

    /// `fxCharSetNot`: complement over `[0, 0x7FFFFFFF)`.
    fn charset_not(&mut self, set: NodeId) -> PResult<NodeId> {
        let src = self.charset_of(set)?;
        let mut out = vec![0i32];
        let mut character = 0i32;
        let mut i = 1usize;
        let count = src[0] as usize;
        while i < 1 + count {
            let begin = src[i];
            let end = src[i + 1];
            i += 2;
            if character < begin {
                out.push(character);
                out.push(begin);
            }
            character = end;
        }
        if character < 0x7FFF_FFFF {
            out.push(character);
            out.push(0x7FFF_FFFF);
        }
        out[0] = (out.len() - 1) as i32;
        Ok(self.add_node(Kind::CharSet { chars: out }))
    }

    /// `fxCharSetCombine` (character-set half only; the V-mode string
    /// operands are the deferred increment). Merge two sorted endpoint
    /// lists under a union/subtract/intersect op.
    fn charset_combine(&mut self, set1: NodeId, set2: NodeId, op: i32) -> PResult<NodeId> {
        let c1 = self.charset_of(set1)?.clone();
        let c2 = self.charset_of(set2)?.clone();
        let count1 = c1[0] as usize;
        let count2 = c2[0] as usize;
        let mut i1 = 1usize;
        let lim1 = 1 + count1;
        let mut i2 = 1usize;
        let lim2 = 1 + count2;
        let mut out = vec![0i32];
        let mut flag = 0i32;
        let mut old = 0i32;
        while i1 < lim1 && i2 < lim2 {
            let test = c1[i1] - c2[i2];
            let mut character = 0i32;
            if test <= 0 {
                character = c1[i1];
                flag ^= 1;
                i1 += 1;
            }
            if test >= 0 {
                character = c2[i2];
                flag ^= 2;
                i2 += 1;
            }
            if flag == op || old == op {
                out.push(character);
            }
            old = flag;
        }
        if op & 2 == 0 {
            while i1 < lim1 {
                out.push(c1[i1]);
                i1 += 1;
            }
        }
        if op & 1 == 0 {
            while i2 < lim2 {
                out.push(c2[i2]);
                i2 += 1;
            }
        }
        out[0] = (out.len() - 1) as i32;
        Ok(self.add_node(Kind::CharSet { chars: out }))
    }

    /// `fxCharSetRange`: build `[a-b]` from two singletons.
    fn charset_range(&mut self, set1: NodeId, set2: NodeId) -> PResult<NodeId> {
        let c1 = self.charset_of(set1)?.clone();
        let c2 = self.charset_of(set2)?.clone();
        if c1[0] == 0 {
            return Ok(set2);
        }
        if c2[0] == 0 {
            return Ok(set1);
        }
        if c1[0] != 2 || c2[0] != 2 {
            return Err(self.error("invalid range"));
        }
        if c1[1] + 1 != c1[2] || c2[1] + 1 != c2[2] {
            return Err(self.error("invalid range"));
        }
        if c1[1] > c2[1] {
            return Err(self.error("invalid range"));
        }
        // The `i`-flag fold branch is the deferred increment; `i` is
        // rejected at compile entry, so this is the plain-range path.
        Ok(self.add_node(Kind::CharSet { chars: vec![2, c1[1], c2[2]] }))
    }

    /// Borrow a node's charset endpoints, erroring if the node is not a
    /// plain charset (e.g. a V-mode string set, which is deferred).
    fn charset_of(&self, id: NodeId) -> PResult<&Vec<i32>> {
        match &self.nodes[id].kind {
            Kind::CharSet { chars } => Ok(chars),
            _ => Err(CompileError::Unsupported("non-charset operand")),
        }
    }

    /// `fxCharSetParseEscape`: the `\`-escapes valid as a character set
    /// (both bare and inside `[...]`). `\p`/`\P` are the named skip.
    fn charset_parse_escape(&mut self, punctuator: bool) -> PResult<NodeId> {
        let result = match self.character {
            c if c == C_EOF => return Err(self.error("invalid escape")),
            c if c == b'd' as i64 => {
                let r = self.charset_digits();
                self.next()?;
                r
            }
            c if c == b'D' as i64 => {
                let d = self.charset_digits();
                let r = self.charset_not(d)?;
                self.next()?;
                r
            }
            c if c == b's' as i64 => {
                let r = self.charset_spaces();
                self.next()?;
                r
            }
            c if c == b'S' as i64 => {
                let s = self.charset_spaces();
                let r = self.charset_not(s)?;
                self.next()?;
                r
            }
            c if c == b'w' as i64 => {
                let r = self.charset_words();
                self.next()?;
                r
            }
            c if c == b'W' as i64 => {
                let w = self.charset_words();
                let r = self.charset_not(w)?;
                self.next()?;
                r
            }
            c if c == b'p' as i64 || c == b'P' as i64 => {
                return Err(CompileError::Unsupported("\\p / \\P unicode property escape"));
            }
            _ => {
                self.pattern_escape(punctuator)?;
                let r = self.charset_single(self.character);
                self.next()?;
                r
            }
        };
        Ok(result)
    }

    /// `fxPatternParserEscape`: resolve the control/hex/unicode/identity
    /// escape at `self.character`, updating `self.character` in place.
    fn pattern_escape(&mut self, punctuator: bool) -> PResult<()> {
        match self.character {
            c if c == C_EOF => {}
            c if c == b'f' as i64 => self.character = 0x0C,
            c if c == b'n' as i64 => self.character = 0x0A,
            c if c == b'r' as i64 => self.character = 0x0D,
            c if c == b't' as i64 => self.character = 0x09,
            c if c == b'v' as i64 => self.character = 0x0B,
            c if c == b'c' as i64 => {
                self.next()?;
                let value = self.character;
                if (b'a' as i64..=b'z' as i64).contains(&value)
                    || (b'A' as i64..=b'Z' as i64).contains(&value)
                {
                    self.character = value % 32;
                } else {
                    return Err(self.error("invalid escape"));
                }
            }
            c if c == b'0' as i64 => {
                let n = self.read8(self.offset);
                if !(b'0'..=b'9').contains(&n) {
                    self.character = 0;
                } else {
                    return Err(self.error("invalid escape"));
                }
            }
            c if c == b'x' as i64 => {
                if let Some((ch, off)) = self.parse_hex_escape(self.offset) {
                    self.character = ch;
                    self.offset = off;
                }
                // Non-UV: a bad \x is an identity escape ('x'); nothing to do.
            }
            c if c == b'u' as i64 => {
                if let Some((ch, off)) = self.parse_unicode_escape(self.offset) {
                    self.character = ch;
                    self.offset = off;
                }
                // Non-UV: a bad \u is an identity escape ('u').
            }
            // Syntax-character and forward-slash identity escapes.
            c if is_syntax_char(c) => {}
            _ => {
                if punctuator {
                    // Class-context punctuator escapes.
                    if self.character == b'b' as i64 {
                        self.character = 0x08;
                    }
                    // The remaining class punctuators are identity escapes.
                }
                // Non-UV: any other escape is an identity escape.
            }
        }
        Ok(())
    }

    /// `fxParseHexEscape`: two hex digits at `offset` → `(char, offset')`.
    fn parse_hex_escape(&self, offset: usize) -> Option<(i64, usize)> {
        let mut value: u32 = 0;
        let mut p = offset;
        for _ in 0..2 {
            let d = hex_digit(self.read8(p))?;
            value = value * 16 + d;
            p += 1;
        }
        Some((value as i64, p))
    }

    /// `fxParseUnicodeEscape`, the non-`UV` four-hex form (`\uXXXX`).
    fn parse_unicode_escape(&self, offset: usize) -> Option<(i64, usize)> {
        let mut value: u32 = 0;
        let mut p = offset;
        for _ in 0..4 {
            let d = hex_digit(self.read8(p))?;
            value = value * 16 + d;
            p += 1;
        }
        Some((value as i64, p))
    }

    /// `fxCharSetParseItem`: one item within `[...]`.
    fn charset_parse_item(&mut self) -> PResult<NodeId> {
        if self.character == b'-' as i64 {
            let r = self.charset_single(b'-' as i64);
            self.next()?;
            Ok(r)
        } else if self.character == b'\\' as i64 {
            self.next()?;
            if self.character == b'b' as i64 {
                self.next()?;
                Ok(self.charset_single(8))
            } else if self.character == b'-' as i64 {
                self.next()?;
                Ok(self.charset_single(b'-' as i64))
            } else {
                self.charset_parse_escape(false)
            }
        } else if self.character == b']' as i64 {
            Ok(self.charset_empty())
        } else {
            let r = self.charset_single(self.character);
            self.next()?;
            Ok(r)
        }
    }

    /// `fxCharSetParseList`: the body of a non-`v` `[...]` class.
    fn charset_parse_list(&mut self) -> PResult<NodeId> {
        let mut not = false;
        let mut former: Option<NodeId> = None;
        let mut result: NodeId = self.charset_empty();
        if self.character == b'^' as i64 {
            self.next()?;
            not = true;
        }
        while self.character != C_EOF {
            result = self.charset_parse_item()?;
            if self.character == b'-' as i64 {
                self.next()?;
                if self.character == b']' as i64 {
                    let dash = self.charset_single(b'-' as i64);
                    result = self.charset_combine(result, dash, MX_CHARSET_UNION_OP)?;
                } else {
                    let hi = self.charset_parse_item()?;
                    result = self.charset_range(result, hi)?;
                }
            }
            if let Some(prev) = former {
                result = self.charset_combine(prev, result, MX_CHARSET_UNION_OP)?;
            }
            former = Some(result);
            if self.character == b']' as i64 {
                break;
            }
        }
        if not {
            result = self.charset_not(result)?;
        }
        Ok(result)
    }

    // ---- quantifier parsing (fxQuantifierParse*) ----

    fn quantifier_parse(&mut self, term: NodeId, capture_index: i32) -> PResult<NodeId> {
        let (min, max) = match self.character {
            c if c == b'*' as i64 => {
                self.next()?;
                (0, MAX_QUANTIFIER)
            }
            c if c == b'+' as i64 => {
                self.next()?;
                (1, MAX_QUANTIFIER)
            }
            c if c == b'?' as i64 => {
                self.next()?;
                (0, 1)
            }
            c if c == b'{' as i64 => {
                if let Some((min, max)) = self.quantifier_parse_brace()? {
                    if min > max {
                        return Err(self.error("invalid quantifier"));
                    }
                    (min, max)
                } else {
                    return Ok(term);
                }
            }
            _ => return Ok(term),
        };
        let greedy = if self.character == b'?' as i64 {
            self.next()?;
            false
        } else {
            true
        };
        let capture_count = self.capture_index - capture_index;
        let quantifier_index = self.quantifier_index;
        self.quantifier_index += 1;
        Ok(self.add_node(Kind::Quantifier {
            term,
            min,
            max,
            greedy,
            capture_index,
            capture_count,
            quantifier_index,
        }))
    }

    /// `fxQuantifierParseBrace`: `{n}` / `{n,}` / `{n,m}` with backtrack.
    fn quantifier_parse_brace(&mut self) -> PResult<Option<(i32, i32)>> {
        let saved_offset = self.offset;
        self.next()?;
        let min = match self.quantifier_parse_digits()? {
            Some(v) => v,
            None => return Ok(self.brace_backtrack(saved_offset)),
        };
        let max;
        if self.character == b',' as i64 {
            self.next()?;
            if self.character == b'}' as i64 {
                max = MAX_QUANTIFIER;
            } else {
                match self.quantifier_parse_digits()? {
                    Some(v) => max = v,
                    None => return Ok(self.brace_backtrack(saved_offset)),
                }
            }
        } else {
            max = min;
        }
        if self.character != b'}' as i64 {
            return Ok(self.brace_backtrack(saved_offset));
        }
        self.next()?;
        Ok(Some((min, max)))
    }

    fn brace_backtrack(&mut self, saved_offset: usize) -> Option<(i32, i32)> {
        self.character = b'{' as i64;
        self.offset = saved_offset;
        None
    }

    fn quantifier_parse_digits(&mut self) -> PResult<Option<i32>> {
        let mut value: u32 = 0;
        if self.decimal(&mut value) {
            self.next()?;
            while self.decimal(&mut value) {
                self.next()?;
            }
        } else {
            return Ok(None);
        }
        if value > 0x7FFF_FFFF {
            value = 0x7FFF_FFFF;
        }
        Ok(Some(value as i32))
    }

    // ---- the recursive-descent grammar (fxDisjunctionParse etc.) ----

    fn disjunction_parse(&mut self, character: i64) -> PResult<NodeId> {
        let mut result = self.sequence_parse(character)?;
        if self.character == b'|' as i64 {
            self.next()?;
            let left = result;
            let right = self.disjunction_parse(character)?;
            result = self.add_node(Kind::Disjunction { left, right });
        }
        if self.character != character {
            return Err(self.error("invalid sequence"));
        }
        Ok(result)
    }

    fn sequence_parse(&mut self, character: i64) -> PResult<NodeId> {
        // Collect the ordered atoms, then fold into a right-nested
        // sequence spine. C threads a mutable `formerBranch->right` into a
        // right-nested `Seq(a0, Seq(a1, ... an))`; because a `Sequence`
        // node emits no bytes of its own and simply chains `left` then
        // `right`, the right-nested fold reproduces the identical measure
        // offsets and emitted step chain (each atom's sequel is the next
        // atom's step; the last atom's sequel is the outer sequel).
        let mut atoms: Vec<NodeId> = Vec::new();
        while self.character != C_EOF && self.character != character {
            if self.character == b'|' as i64 {
                break;
            }
            let current_index = self.capture_index;
            atoms.push(self.term_parse(current_index)?);
        }
        if atoms.is_empty() {
            return Ok(self.add_node(Kind::Empty));
        }
        let mut result = *atoms.last().unwrap();
        for &atom in atoms.iter().rev().skip(1) {
            result = self.add_node(Kind::Sequence { left: atom, right: result });
        }
        Ok(result)
    }

    /// One atom (+ its optional quantifier) of a sequence — the big
    /// dispatch in `fxSequenceParse`.
    fn term_parse(&mut self, current_index: i32) -> PResult<NodeId> {
        let ch = self.character;
        if ch == b'^' as i64 {
            self.next()?;
            Ok(self.add_node(Kind::LineBegin))
        } else if ch == b'$' as i64 {
            self.next()?;
            Ok(self.add_node(Kind::LineEnd))
        } else if ch == b'\\' as i64 {
            self.next()?;
            self.backslash_atom(current_index)
        } else if ch == b'.' as i64 {
            let any = self.charset_any();
            self.next()?;
            self.quantifier_parse(any, current_index)
        } else if ch == b'*' as i64 || ch == b'+' as i64 || ch == b'?' as i64 {
            Err(self.error("invalid character"))
        } else if ch == b'(' as i64 {
            self.group_atom(current_index)
        } else if ch == b')' as i64 {
            Err(self.error("invalid character"))
        } else if ch == b'[' as i64 {
            self.next()?;
            let current = self.charset_parse_list()?;
            if self.character != b']' as i64 {
                return Err(self.error("invalid range"));
            }
            self.next()?;
            self.quantifier_parse(current, current_index)
        } else if ch == b'|' as i64 {
            // Handled by disjunction_parse; sequence stops here. This is
            // unreachable because the while-guard covers `character`, but
            // the '|' case is an explicit break in C.
            Err(self.error("invalid character"))
        } else {
            // Ordinary character (with the Annex-B `{` non-quantifier
            // tolerance the non-`UV` path allows).
            if ch == b'{' as i64 {
                if let Some(_) = self.quantifier_parse_brace()? {
                    return Err(self.error("invalid quantifier"));
                }
            }
            let single = self.charset_single(self.character);
            self.next()?;
            self.quantifier_parse(single, current_index)
        }
    }

    /// The `\`-prefixed atoms in atom position (assertions, references,
    /// escaped charsets).
    fn backslash_atom(&mut self, current_index: i32) -> PResult<NodeId> {
        if self.character == b'b' as i64 {
            self.next()?;
            Ok(self.add_node(Kind::WordBreak))
        } else if self.character == b'B' as i64 {
            self.next()?;
            Ok(self.add_node(Kind::WordContinue))
        } else if self.character == b'k' as i64 && self.flags & (XS_REGEXP_U | XS_REGEXP_V | XS_REGEXP_N) != 0 {
            Err(CompileError::Unsupported("\\k<name> named backreference"))
        } else if (b'1' as i64..=b'9' as i64).contains(&self.character) {
            let mut value: u32 = (self.character - b'0' as i64) as u32;
            self.next()?;
            while self.decimal(&mut value) {
                self.next()?;
            }
            let node = self.add_node(Kind::CaptureReference { capture_index: value as i32 });
            self.quantifier_parse(node, current_index)
        } else {
            // \0, control, hex, \u, \d\w\s, identity escapes → a charset.
            let set = self.charset_parse_escape(false)?;
            self.quantifier_parse(set, current_index)
        }
    }

    /// The `(`-prefixed atoms: capturing / non-capturing groups and
    /// lookaround assertions.
    fn group_atom(&mut self, mut current_index: i32) -> PResult<NodeId> {
        self.next()?;
        if self.character == b'?' as i64 {
            self.next()?;
            if self.character == b'=' as i64 {
                self.next()?;
                let term = self.disjunction_parse(b')' as i64)?;
                self.next()?;
                let ai = self.assertion_index;
                self.assertion_index += 1;
                Ok(self.add_node(Kind::Assertion { term, not: false, direction: 1, assertion_index: ai }))
            } else if self.character == b'!' as i64 {
                self.next()?;
                let term = self.disjunction_parse(b')' as i64)?;
                self.next()?;
                let ai = self.assertion_index;
                self.assertion_index += 1;
                Ok(self.add_node(Kind::Assertion { term, not: true, direction: 1, assertion_index: ai }))
            } else if self.character == b':' as i64 {
                self.next()?;
                let current = self.disjunction_parse(b')' as i64)?;
                self.next()?;
                self.quantifier_parse(current, current_index)
            } else if self.character == b'<' as i64 {
                self.next()?;
                if self.character == b'=' as i64 {
                    self.next()?;
                    let term = self.disjunction_parse(b')' as i64)?;
                    self.next()?;
                    let ai = self.assertion_index;
                    self.assertion_index += 1;
                    Ok(self.add_node(Kind::Assertion { term, not: false, direction: -1, assertion_index: ai }))
                } else if self.character == b'!' as i64 {
                    self.next()?;
                    let term = self.disjunction_parse(b')' as i64)?;
                    self.next()?;
                    let ai = self.assertion_index;
                    self.assertion_index += 1;
                    Ok(self.add_node(Kind::Assertion { term, not: true, direction: -1, assertion_index: ai }))
                } else {
                    Err(CompileError::Unsupported("(?<name>) named capture"))
                }
            } else {
                Err(CompileError::Unsupported("(?flags:) inline modifiers"))
            }
        } else {
            self.capture_index += 1;
            current_index += 1;
            let term = self.disjunction_parse(b')' as i64)?;
            self.next()?;
            let capture = self.add_node(Kind::Capture { term, capture_index: current_index });
            self.quantifier_parse(capture, current_index - 1)
        }
    }

    // ---- the measure pass (fx*Measure) ----

    fn measure(&mut self, id: NodeId, direction: i32) {
        // Split-borrow: read the kind's child ids first, mutate offsets
        // after. We recurse by id, so the arena stays coherent.
        match self.child_shape(id) {
            Shape::Term => {
                self.nodes[id].step = self.size as i32;
                self.size += 8; // mxTermStepSize
            }
            Shape::CharSet(count) => {
                self.nodes[id].step = self.size as i32;
                self.size += 8 + ((1 + count) as i64) * 4;
            }
            Shape::Disjunction(left, right) => {
                self.nodes[id].step = self.size as i32;
                self.size += 12; // mxDisjunctionStepSize
                self.measure(left, direction);
                self.measure(right, direction);
            }
            Shape::Sequence(left, right) => {
                if direction == 1 {
                    self.measure(left, direction);
                    let s = self.nodes[left].step;
                    self.nodes[id].step = s;
                    self.measure(right, direction);
                } else {
                    self.measure(right, direction);
                    let s = self.nodes[right].step;
                    self.nodes[id].step = s;
                    self.measure(left, direction);
                }
            }
            Shape::Capture(term) => {
                self.nodes[id].step = self.size as i32;
                self.size += 12; // mxCaptureStepSize
                self.measure(term, direction);
                self.nodes[id].completion = self.size as i32;
                self.size += 16; // mxCaptureCompletionSize
            }
            Shape::CaptureReference => {
                self.nodes[id].step = self.size as i32;
                self.size += 16; // mxCaptureReferenceStepSize
            }
            Shape::Assertion { term, not, direction: dir } => {
                self.nodes[id].step = self.size as i32;
                self.size += if not { 16 } else { 12 };
                self.measure(term, dir);
                self.nodes[id].completion = self.size as i32;
                self.size += if not { 8 } else { 12 };
            }
            Shape::Quantifier(term) => {
                self.nodes[id].step = self.size as i32;
                self.size += 20; // mxQuantifierStepSize
                self.nodes[id].loop_off = self.size as i32;
                self.size += 24; // mxQuantifierLoopSize
                self.measure(term, direction);
                self.nodes[id].completion = self.size as i32;
                self.size += 24; // mxQuantifierCompletionSize
            }
        }
    }

    // ---- the code pass (fx*Code) ----

    fn emit(&mut self, id: NodeId, direction: i32, sequel: i32) {
        match self.child_shape(id) {
            Shape::Term => {
                let opcode = match &self.nodes[id].kind {
                    Kind::Empty => CX_EMPTY_STEP,
                    Kind::LineBegin => CX_LINE_BEGIN_STEP,
                    Kind::LineEnd => CX_LINE_END_STEP,
                    Kind::WordBreak => CX_WORD_BREAK_STEP,
                    Kind::WordContinue => CX_WORD_CONTINUE_STEP,
                    _ => unreachable!(),
                };
                let at = (self.nodes[id].step / 4) as usize;
                self.code[at] = opcode;
                self.code[at + 1] = sequel;
            }
            Shape::CharSet(count) => {
                let chars: Vec<i32> = match &self.nodes[id].kind {
                    Kind::CharSet { chars } => chars.clone(),
                    _ => unreachable!(),
                };
                let at = (self.nodes[id].step / 4) as usize;
                self.code[at] = if direction == 1 { CX_CHARSET_FORWARD_STEP } else { CX_CHARSET_BACKWARD_STEP };
                self.code[at + 1] = sequel;
                self.code[at + 2] = count;
                for i in 0..count as usize {
                    self.code[at + 3 + i] = chars[1 + i];
                }
            }
            Shape::Disjunction(left, right) => {
                let at = (self.nodes[id].step / 4) as usize;
                self.code[at] = CX_DISJUNCTION_STEP;
                self.code[at + 1] = self.nodes[left].step;
                self.code[at + 2] = self.nodes[right].step;
                self.emit(left, direction, sequel);
                self.emit(right, direction, sequel);
            }
            Shape::Sequence(left, right) => {
                if direction == 1 {
                    let right_step = self.nodes[right].step;
                    self.emit(left, direction, right_step);
                    self.emit(right, direction, sequel);
                } else {
                    let left_step = self.nodes[left].step;
                    self.emit(right, direction, left_step);
                    self.emit(left, direction, sequel);
                }
            }
            Shape::Capture(term) => {
                let (step, completion, capture_index) = {
                    let n = &self.nodes[id];
                    let ci = match &n.kind {
                        Kind::Capture { capture_index, .. } => *capture_index,
                        _ => unreachable!(),
                    };
                    (n.step, n.completion, ci)
                };
                let term_step = self.nodes[term].step;
                let at = (step / 4) as usize;
                self.code[at] = if direction == 1 { CX_CAPTURE_FORWARD_STEP } else { CX_CAPTURE_BACKWARD_STEP };
                self.code[at + 1] = term_step;
                self.code[at + 2] = capture_index;
                self.emit(term, direction, completion);
                let ct = (completion / 4) as usize;
                self.code[ct] = if direction == 1 { CX_CAPTURE_FORWARD_COMPLETION } else { CX_CAPTURE_BACKWARD_COMPLETION };
                self.code[ct + 1] = sequel;
                self.code[ct + 2] = capture_index;
                // No name in this increment: the name-id operand is -1.
                self.code[ct + 3] = -1;
            }
            Shape::CaptureReference => {
                let capture_index = match &self.nodes[id].kind {
                    Kind::CaptureReference { capture_index } => *capture_index,
                    _ => unreachable!(),
                };
                let at = (self.nodes[id].step / 4) as usize;
                self.code[at] = if direction == 1 { CX_CAPTURE_REFERENCE_FORWARD_STEP } else { CX_CAPTURE_REFERENCE_BACKWARD_STEP };
                self.code[at + 1] = sequel;
                self.code[at + 2] = capture_index;
                self.code[at + 3] = -1; // nameIndex (numeric ref)
            }
            Shape::Assertion { term, not, direction: dir } => {
                let (step, completion, ai) = {
                    let n = &self.nodes[id];
                    let ai = match &n.kind {
                        Kind::Assertion { assertion_index, .. } => *assertion_index,
                        _ => unreachable!(),
                    };
                    (n.step, n.completion, ai)
                };
                let term_step = self.nodes[term].step;
                let at = (step / 4) as usize;
                if not {
                    self.code[at] = CX_ASSERTION_NOT_STEP;
                    self.code[at + 1] = term_step;
                    self.code[at + 2] = ai;
                    self.code[at + 3] = sequel;
                } else {
                    self.code[at] = CX_ASSERTION_STEP;
                    self.code[at + 1] = term_step;
                    self.code[at + 2] = ai;
                }
                self.emit(term, dir, completion);
                let ct = (completion / 4) as usize;
                if not {
                    self.code[ct] = CX_ASSERTION_NOT_COMPLETION;
                    self.code[ct + 1] = ai;
                } else {
                    self.code[ct] = CX_ASSERTION_COMPLETION;
                    self.code[ct + 1] = sequel;
                    self.code[ct + 2] = ai;
                }
            }
            Shape::Quantifier(term) => {
                let (step, loop_off, completion, greedy, quantifier_index, capture_index, capture_count, min, max) = {
                    let n = &self.nodes[id];
                    match &n.kind {
                        Kind::Quantifier { min, max, greedy, capture_index, capture_count, quantifier_index, .. } => (
                            n.step, n.loop_off, n.completion, *greedy, *quantifier_index, *capture_index, *capture_count, *min, *max,
                        ),
                        _ => unreachable!(),
                    }
                };
                let term_step = self.nodes[term].step;
                let at = (step / 4) as usize;
                self.code[at] = CX_QUANTIFIER_STEP;
                self.code[at + 1] = loop_off;
                self.code[at + 2] = quantifier_index;
                self.code[at + 3] = min;
                self.code[at + 4] = max;
                let lp = (loop_off / 4) as usize;
                self.code[lp] = if greedy { CX_QUANTIFIER_GREEDY_LOOP } else { CX_QUANTIFIER_LAZY_LOOP };
                self.code[lp + 1] = term_step;
                self.code[lp + 2] = quantifier_index;
                self.code[lp + 3] = sequel;
                self.code[lp + 4] = capture_index + 1;
                self.code[lp + 5] = capture_index + capture_count;
                self.emit(term, direction, completion);
                let ct = (completion / 4) as usize;
                self.code[ct] = CX_QUANTIFIER_COMPLETION;
                self.code[ct + 1] = loop_off;
                self.code[ct + 2] = quantifier_index;
                self.code[ct + 3] = sequel;
                self.code[ct + 4] = capture_index + 1;
                self.code[ct + 5] = capture_index + capture_count;
            }
        }
    }

    /// Classify a node into its measure/code shape, reading child ids and
    /// charset count without holding a borrow across the recursive calls.
    fn child_shape(&self, id: NodeId) -> Shape {
        match &self.nodes[id].kind {
            Kind::CharSet { chars } => Shape::CharSet(chars[0]),
            Kind::Empty | Kind::LineBegin | Kind::LineEnd | Kind::WordBreak | Kind::WordContinue => {
                Shape::Term
            }
            Kind::Disjunction { left, right } => Shape::Disjunction(*left, *right),
            Kind::Sequence { left, right } => Shape::Sequence(*left, *right),
            Kind::Capture { term, .. } => Shape::Capture(*term),
            Kind::CaptureReference { .. } => Shape::CaptureReference,
            Kind::Assertion { term, not, direction, .. } => {
                Shape::Assertion { term: *term, not: *not, direction: *direction }
            }
            Kind::Quantifier { term, .. } => Shape::Quantifier(*term),
        }
    }
}

enum Shape {
    Term,
    CharSet(i32),
    Disjunction(NodeId, NodeId),
    Sequence(NodeId, NodeId),
    Capture(NodeId),
    CaptureReference,
    Assertion { term: NodeId, not: bool, direction: i32 },
    Quantifier(NodeId),
}

fn hex_digit(c: u8) -> Option<u32> {
    match c {
        b'0'..=b'9' => Some((c - b'0') as u32),
        b'a'..=b'f' => Some((c - b'a' + 10) as u32),
        b'A'..=b'F' => Some((c - b'A' + 10) as u32),
        _ => None,
    }
}

/// The syntax characters that are identity escapes (`fxPatternParserEscape`
/// explicit cases) regardless of `punctuator`.
fn is_syntax_char(c: i64) -> bool {
    matches!(
        c as u8 as char,
        '^' | '$' | '\\' | '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '/'
    ) && (0..=0x7F).contains(&c)
}
