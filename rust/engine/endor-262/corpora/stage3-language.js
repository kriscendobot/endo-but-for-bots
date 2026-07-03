// Stage-3 child-1 (language) corpus: the language opcodes and string
// values this child adds, each bit-exact (result AND computron) against
// the C-XS oracle at the pin 48ee02d8cfe0. Every line is one program; the
// last expression is the completion value.
//
// Covers: chunk-backed CESU-8 string values (literals, concatenation with
// its ToString + fxConcatString chunk metering, equality, relational
// comparison), `typeof` over every covered kind, the numeric opcodes
// `increment`/`decrement`/`to_numeric`/exponentiation with XS's exact
// int-boundary promotion, `this`, `let`/`const` closures (including a loop
// body's per-iteration reset/refresh cells), and the nullish/optional
// chaining branches `branch_coalesce` (`??`) and `branch_chain` (`?.`).

// --- string literals + rendering ---
"hello"
""
"a string with spaces"

// --- concatenation (ToString + fxConcatString chunk metering) ---
"foo" + "bar"
"a" + "b" + "c"
"n=" + 42
1 + "z"
"x" + 5
"" + "tail"
"pi=" + 3.5

// --- equality and relational over strings (content-byte compare) ---
"abc" === "abc"
"abc" !== "abd"
"a" < "b"
"b" > "a"
"ab" <= "ab"
"z" >= "a"
"abc" === "abcd"

// --- typeof over every covered kind ---
typeof 1
typeof 3.5
typeof "s"
typeof true
typeof void 0
typeof null
typeof {}
typeof this
typeof (1 + 2)
typeof ("a" + "b")
var f = function(){}; typeof f

// --- empty-string truthiness ---
"" ? 1 : 2
"x" ? 1 : 2
!""
!"x"

// --- increment / decrement / exponentiation ---
var i = 5; i++; i
var j = 5; ++j
var k = 5; k--; k
var m = 5; --m
2 ** 10
3 ** 3
let e = 2; e **= 3; e

// --- let / const closures ---
let a = 1; a
const b = 2; b
let s = "hi"; s + "!"
for (let i = 0; i < 3; i = i + 1) {} 9
let c = 0; for (let i = 0; i < 5; i = i + 1) { c = c + i } c

// --- nullish coalescing (??) ---
null ?? 5
(void 0) ?? "d"
var z = 0; z ?? 9
let u; u ?? 7
let v = 3; v ?? 7

// --- optional chaining (?.) ---
var o = { a: 1 }; o?.a
var o2 = null; o2?.a
var o3 = { a: { b: 7 } }; o3?.a?.b
