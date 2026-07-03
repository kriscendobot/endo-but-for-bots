// Stage-3 child-4 (text-math-json) — Math corpus: the `Math` namespace
// object, its numeric constants, and every modeled `Math.*` static — each
// line bit-exact (result AND computron) against the C-XS oracle at the pin
// `48ee02d8cfe0`. The `xsMath.c` bodies carry no `mxMeterSome` and allocate
// no chunk, so a Math call's whole cost is the native host frame (measured
// zero over the `RUN` opcode endor already meters); the NaN result is the
// canonical `f64::NAN` (`C_NAN`), which the design flags consensus-critical.

// `Math` is a namespace object, not a function.
typeof Math

// The numeric constants — the exact IEEE doubles from math.h.
Math.E
Math.LN10
Math.LN2
Math.LOG10E
Math.LOG2E
Math.PI
Math.SQRT1_2
Math.SQRT2

// abs / floor / ceil / round / trunc — the integer-folding results.
Math.abs(-5)
Math.abs(-3.5)
Math.abs(0)
Math.floor(3.7)
Math.floor(-3.2)
Math.ceil(3.2)
Math.ceil(-3.7)
Math.round(2.5)
Math.round(-2.5)
Math.round(0.4)
Math.trunc(-4.7)
Math.trunc(4.7)

// sign — ±1, ±0, NaN.
Math.sign(-3)
Math.sign(3)
Math.sign(0)
Math.sign(-0)

// sqrt / cbrt / pow / hypot — the algebraically-exact / correctly-rounded ops.
Math.sqrt(2)
Math.sqrt(16)
Math.cbrt(27)
Math.cbrt(-8)
Math.pow(2, 10)
Math.pow(2, 0.5)
Math.pow(1, Infinity)
Math.pow(-1, Infinity)
Math.hypot(3, 4)
Math.hypot(5, 12)
Math.hypot(1, 2, 2)

// max / min — the running extremum, the integer fast path, and the ±0
// tie-break (`max(+0,-0)===+0`, `min(+0,-0)===-0`).
Math.max(1, 2, 3)
Math.max(1, 2, 3.5)
Math.max()
Math.min(4, -0, 0)
Math.min()
Math.max(0, -0)
Math.min(0, -0)

// The transcendentals — one system-libm ULP, matching the oracle's libm.
Math.sin(1)
Math.cos(0)
Math.tan(1.2)
Math.asin(0.3)
Math.acos(0.3)
Math.atan(0.7)
Math.atan2(1, 1)
Math.sinh(1.5)
Math.cosh(1.5)
Math.tanh(0.9)
Math.asinh(1.7)
Math.acosh(2.3)
Math.atanh(0.4)
Math.exp(1)
Math.expm1(0.001)
Math.log(7)
Math.log1p(0.001)
Math.log2(8)
Math.log10(1000)

// clz32 / imul / fround — the 32-bit and float32 integer ops.
Math.clz32(1)
Math.clz32(0)
Math.imul(3, 4)
Math.imul(-5, 7)
Math.fround(1.1)
Math.fround(0)

// NaN canonicalization and the small/large exponent renderings that exercise
// Number::toString's fixed-vs-exponential threshold.
Math.abs(NaN)
Math.sqrt(-1)
Math.cos(1.5707963267948966)
Math.sin(3.141592653589793)
