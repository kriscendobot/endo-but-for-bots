// stage-4b compartment differential corpus (child 3/5).
//
// Each program is compiled once on the C-XS oracle and then evaluated in
// TWO compartments over ONE machine's shared intrinsics
// (`Compartment::evaluate_with_symbols`). The bar is RESULT agreement
// (doctrine: accuracy over parity — the compartment differential
// certifies results): each compartment's completion value must equal the
// oracle's, so a program's intrinsic references resolve identically in
// every compartment (shared-intrinsics identity) and two compartments
// over one machine agree on every intrinsic-only value (cross-compartment
// values). The compartment evaluator seeds no globals here, so the same
// bytecode also reproduces the oracle's computron count bit-for-bit.
//
// Programs that reference the `Compartment` intrinsic itself
// (`new Compartment().evaluate(...)`, `typeof Compartment`) are the
// recorded scope fold (`compartment:intrinsic-surface`): endor models
// Compartment host-side (a Rust realm API), not as a guest-callable
// intrinsic, so they are excluded rather than diverged.

// Operators and literals evaluate the same in every compartment.
1 + 2 * 3
(10 - 4) / 2
5 % 3
2 ** 8
true && false
1 < 2 ? "a" : "b"
typeof 42
typeof "x"
void 0

// Intrinsic references relink to the shared intrinsics identically in
// every compartment.
Boolean(0)
Boolean(1)
Object.keys({a: 1, b: 2}).length
String(42)
Number("17")
Math.max(3, 7, 2)
Math.min(3, 7, 2)
Math.abs(-9)
Number.isInteger(5)
Number.isNaN(NaN)
parseInt("2a", 16)
JSON.stringify(7)

// Array/string intrinsic surfaces over the shared intrinsics.
[1, 2, 3].length
[1, 2, 3][2]
"hello".length
"hello".charAt(1)
"hello".slice(1, 3)
"ab".concat("cd")

// Values crossing compartment evaluations render identically.
[1, 2, 3, 4].reduce(function (a, b) { return a + b; }, 0)
[1, 2, 3].map(function (x) { return x * 2; }).length
