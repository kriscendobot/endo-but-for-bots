// Stage-3b (xsre integration, child 9) — the JavaScript RegExp surface over
// child 8's matcher, bit-exact (result AND computron) against the C-XS oracle
// at the pin `48ee02d8cfe0`. Construction (`fx_RegExp` + `fxInitializeRegExp`)
// is allocation-driven (`fxNewRegExpInstance`'s four `fxNewSlot`s) plus the
// `fxCompileRegExp` parse meter (`parser->size * XS_PARSE_REGEXP_METERING`) and
// a calibrated ctor frame + per-source-byte residual; `exec`/`test` carry the
// `fxMatchRegExp` step meter (`XS_REGEXP_METERING` per step) plus the
// result-array `fxNewSlot`/`fxNewChunk` clusters and a calibrated exec/test
// frame (test drives the full exec, as `fxExecuteRegExp` does); the g/y
// stateful path adds the `fxCache*Offset` remap residual (interp.rs § REGEXP_*).
//
// Honest NAMED skips (never a wrong value, never a divergence): a RegExp-valued
// pattern argument, named groups (`(?<n>)`), a syntax-error / not-yet-ported
// pattern feature (each a catchable throw XS models but endor does not meter
// this stage), and a non-ASCII subject under a stateful (g/y) flag. Each
// self-names `Halt::Unsupported`.

// --- construct: the literal and the constructor form ----------------------
/abc/
/abc/g
/a(b)c/
new RegExp("abc")
new RegExp("abc", "g")
new RegExp("")
new RegExp("a|b|c")
/[a-z]+/
/\d{2,4}/
/^ab*c$/

// --- source (escape-on-read; empty renders as the (?:) source) ------------
/abc/.source
/a\/b/.source
new RegExp("").source
/abc/g.toString()
/a(b)c/gi.toString()

// --- flags: the composite string + the per-flag boolean getters -----------
/abc/g.flags
/abc/gi.flags
/abc/.flags
/abc/g.global
/abc/.global
/abc/i.ignoreCase
/abc/m.multiline
/abc/s.dotAll
/abc/y.sticky
/abc/gimsy.flags

// --- exec: match, no-match, captures --------------------------------------
/abc/.exec("abc")
/abc/.exec("xyz")
/b(c)/.exec("abc")
/(a)(b)(c)/.exec("abc")
/abc/.exec("zzzabc")
/x/.exec("abcdefghij")
/a(b)?c/.exec("ac")
/[0-9]+/.exec("num=42!")

// --- exec: the result-array named slots -----------------------------------
/b(c)/.exec("abc").index
/b(c)/.exec("abc").input
/b(c)/.exec("abc")[1]
/b(c)/.exec("abc").length

// --- test -----------------------------------------------------------------
/abc/.test("abc")
/abc/.test("xyz")
/^a/.test("abc")
/c$/.test("abc")
/[0-9]/.test("num=42")
/xyz/.test("abcdefghij")

// --- lastIndex: the stateful global/sticky drive --------------------------
/a/g.lastIndex
/a/g.exec("banana").index
/a/g.test("xax")

// --- alternation, quantifiers, anchors, classes ---------------------------
/colou?r/.test("color")
/colou?r/.test("colour")
/\bword\b/.test("a word here")
/(foo|bar)+/.exec("foobarfoo")
/\s+/.exec("a   b")

// --- String.prototype.search (Symbol.search protocol → the RegExp worker) --
"abc".search(/b/)
"abc".search(/a/)
"abc".search(/z/)
"hello world".search(/o/)
"hello world".search(/\s/)
"abc123".search(/[0-9]/)

// --- String.prototype.match (non-global; Symbol.match protocol) -----------
"abc".match(/b(c)/)
"abc".match(/z/)
"aXbXc".match(/X(.)/)
"2026-07".match(/(\d+)-(\d+)/)
"abc".match(/a/).index
"abc".match(/b(c)/)[1]

// --- String.prototype.replace (non-global, literal replacement) -----------
"abc".replace(/b/, "X")
"abc".replace(/z/, "X")
"abc".replace(/a/, "XY")
"abc".replace(/c/, "")
"hello".replace(/l/, "L")
"a1b2".replace(/[0-9]/, "#")
"x".replace(/(x)/, "y")
"ab".replace(/(a)(b)/, "z")

// --- String.prototype.split (Symbol.split → the sticky splitter) ----------
"a,b,c".split(/,/)
"abc".split(/,/)
"a1b2c".split(/[0-9]/)
"".split(/,/)
"axbxc".split(/(x)/)
"a,b".split(/,/, 5)
"a,b,c".split(/,/).length
"a1b2c".split(/([0-9])/)
