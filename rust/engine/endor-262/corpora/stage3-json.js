// Stage-3 child-4 (text-math-json) — JSON corpus: JSON.stringify over a
// top-level primitive, bit-exact (result AND computron) against the C-XS
// oracle at the pin `48ee02d8cfe0`. The stringifier's working buffer is a
// C-malloc (unmetered); only the final fxNewChunk(offset) meters, plus the
// measured setup residual (82432 raw) and, for a produced value, the
// name/serialize residual (16384 raw) — both value-independent.
//
// Structured (object/array) stringify carries per-node allocation costs (the
// keys instance, per-key strings, the recursive property frames) not yet
// modeled to a clean constant; it is an honest NAMED skip this stage (the
// serialized RESULT is correct — only the computron count is deferred), as is
// JSON.parse. Never a wrong value, never a divergence.

// scalars — the value-independent produced-primitive residual.
JSON.stringify(42)
JSON.stringify(-17)
JSON.stringify(0)
JSON.stringify(3.14)
JSON.stringify(true)
JSON.stringify(false)
JSON.stringify(null)

// strings — the JSON escaper (quotes, backslash, control-char escapes).
JSON.stringify("hi")
JSON.stringify("")
JSON.stringify("a\"b")
JSON.stringify("back\\slash")
JSON.stringify("tab\tnewline\nreturn\r")
JSON.stringify("bell\bform\f")
JSON.stringify("plain text 123")

// undefined — serializes to nothing (result undefined, setup metered only).
JSON.stringify(undefined)

// a stringify of a computed primitive (the value comes from a prior op).
JSON.stringify(1 + 2)
JSON.stringify("a".concat("b"))
JSON.stringify(Math.max(3, 7))
