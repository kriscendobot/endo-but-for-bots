// Stage-3 child-4 (text-math-json) — Number + numeric-globals corpus: the
// Number statics/predicates, Number.prototype.toString (radix 10), the
// Number(...) coercion, and the global parseInt/parseFloat/isNaN/isFinite —
// each line bit-exact (result AND computron) against the C-XS oracle at the
// pin `48ee02d8cfe0`. The xsNumber.c bodies carry no mxMeterSome (a number/
// boolean result has no chunk); Number.prototype.toString(10) allocates its
// result chunk and carries the 33280-raw fxNumberToString host residual.
// Non-decimal radix, and a non-string parseInt/parseFloat argument, are honest
// NAMED skips this stage — never a wrong value.

// The numeric constants — the exact IEEE doubles.
Number.EPSILON
Number.MAX_SAFE_INTEGER
Number.MIN_SAFE_INTEGER
Number.MAX_VALUE
Number.MIN_VALUE
Number.NaN
Number.POSITIVE_INFINITY
Number.NEGATIVE_INFINITY

// isInteger / isFinite / isNaN / isSafeInteger — the kind-inspecting
// predicates (no coercion).
Number.isInteger(5)
Number.isInteger(5.5)
Number.isInteger(0)
Number.isInteger(NaN)
Number.isFinite(42)
Number.isFinite(1 / 0)
Number.isFinite(NaN)
Number.isNaN(NaN)
Number.isNaN(5)
Number.isNaN(0 / 0)
Number.isSafeInteger(9007199254740991)
Number.isSafeInteger(9007199254740992)
Number.isSafeInteger(3.5)

// Number.prototype.toString — radix 10 (the common path).
(255).toString()
(3.14).toString()
(0).toString()
(-42).toString()
(1000000).toString()
(1.5e21).toString()

// Number(...) coercion — numeric identity, boolean/null/undefined, and the
// whole-string parse (decimal, whitespace, 0x, empty, invalid).
Number(42)
Number(3.14)
Number(true)
Number(null)
Number("42")
Number("3.14")
Number("  10  ")
Number("0x1F")
Number("1e3")
Number("")
Number("abc")
Number("Infinity")

// parseInt — the integer prefix parse (default and explicit radix).
parseInt("42")
parseInt("42px")
parseInt("0xff")
parseInt("101", 2)
parseInt("z", 36)
parseInt("  -17  ")
parseInt("z")
parseInt("")
parseInt("3.9")

// parseFloat — the float prefix parse.
parseFloat("3.14")
parseFloat("3.14abc")
parseFloat("  .5e2xyz")
parseFloat("42")
parseFloat("Infinity")
parseFloat("abc")
parseFloat("-0.5")

// isNaN / isFinite — the global ToNumber-then-classify.
isNaN(NaN)
isNaN(42)
isNaN("abc")
isNaN("42")
isFinite(42)
isFinite(1 / 0)
isFinite("100")

// numeric work in a loop — parse + predicate over a running accumulator.
var acc = 0; for (var i = 0; i < 6; i++) { acc = acc + parseInt("10"); } acc
Number.isInteger(parseInt("123"))
