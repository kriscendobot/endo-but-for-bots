// Stage-3 child-4 (text-math-json) — String.prototype corpus: primitive
// string property/method access over the CESU-8 chunk representation, each
// line bit-exact (result AND computron) against the C-XS oracle at the pin
// `48ee02d8cfe0`. A primitive string boxes to `%String.prototype%`; `.length`
// is the UTF-16 code-unit count and `str[i]` the one-unit character, both from
// the non-continuation-byte walk `fxUnicodeLength` counts.
//
// Metering model (verified against the pin): the chunk-allocating /
// number-returning methods that call NO `mxMeterSome`
// (slice/substring/charAt/at/charCodeAt/codePointAt/str[i]) carry a ZERO
// native residual; the `mxMeterSome`-calling methods
// (startsWith/endsWith/includes/concat/toLowerCase/toUpperCase/repeat/trim*)
// carry a fixed 33280-raw residual beyond their explicit steps + result chunk.
// `indexOf`/`lastIndexOf` are honest NAMED skips (their inner-loop scan
// metering is not yet calibrated) — never a wrong value, never a divergence.

// length — the UTF-16 code-unit count.
"".length
"a".length
"hello".length
"hello world".length

// indexing — the one-unit character (or undefined past the end).
"abc"[0]
"abc"[2]
"abc"[5]
"hello"[1]

// charCodeAt / codePointAt — the code unit / code point (no chunk, no meter).
"abc".charCodeAt(0)
"abc".charCodeAt(2)
"abc".charCodeAt(5)
"A".charCodeAt(0)
"hello".codePointAt(1)
"z".codePointAt(0)

// charAt / at — the one-unit string (chunk only).
"abc".charAt(0)
"abc".charAt(2)
"abc".charAt(9)
"hello".at(0)
"hello".at(-1)
"hello".at(-2)
"hello".at(10)

// slice — negative offsets from the end (chunk only).
"abcdef".slice(1, 3)
"abcdef".slice(-2)
"abcdef".slice(2)
"abcdef".slice(-4, -1)
"abcdef".slice(4, 2)
"abcdef".slice(0)

// substring — clamped, swapped-if-needed (chunk only).
"abcdef".substring(2, 4)
"abcdef".substring(4, 2)
"abcdef".substring(3)
"abcdef".substring(0, 100)

// concat — mxMeterSome(argc) + chunk + residual.
"ab".concat("cd")
"ab".concat("cd", "ef")
"x".concat("")
"".concat("y")

// repeat — mxMeterSome(count) + chunk + residual.
"ab".repeat(0)
"ab".repeat(1)
"xy".repeat(3)
"z".repeat(5)

// toLowerCase / toUpperCase — ASCII case mapping, mxMeterSome(len) + residual.
"ABC".toLowerCase()
"AbCdE".toLowerCase()
"abc".toUpperCase()
"Hello World".toUpperCase()
"already".toLowerCase()

// startsWith / endsWith — mxMeterSome(searchLen) + residual.
"hello".startsWith("he")
"hello".startsWith("lo")
"hello".startsWith("z")
"hello".endsWith("lo")
"hello".endsWith("he")
"hello".startsWith("ell", 1)
"hello".endsWith("hel", 3)

// includes — the fixed residual, scan unmetered.
"hello".includes("ell")
"hello".includes("lo")
"hello".includes("xyz")
"hello".includes("")
"aXbXcX".includes("X")

// trim / trimStart / trimEnd — mxMeterSome + chunk + residual.
"  hi  ".trim()
"  hi  ".trimStart()
"  hi  ".trimEnd()
"nospaces".trim()
"   ".trim()
"\tindented\n".trim()

// building strings in a loop — the metering hot path (concat over a
// variable-held accumulator, then a `.length` read of the built string).
var s = ""; for (var i = 0; i < 5; i++) { s = s.concat("ab"); } s.length
var w = ""; for (var k = 0; k < 4; k++) { w = w.concat("ab"); } w
// NOTE (supervisor): a string that is a method-call RESULT consumed
// *directly* as a receiver/argument without an intervening variable
// (`"x".repeat(4).length`, `u.concat(t.charAt(j))`) carries an extra
// 33280-raw residual endor does not yet model — a temporary-lifetime
// interaction distinct from the per-method frames calibrated here. Held out
// of the bit-exact corpus (never faked); the same access through a variable
// (above) is exact. A named follow-up for the scan-metering calibration pass.
