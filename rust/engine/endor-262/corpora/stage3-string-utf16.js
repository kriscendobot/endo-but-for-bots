// Stage-3 UTF-16-swap child — surrogate-pair / index-heavy / lone-surrogate
// differential fixtures. Each program's completion is a NUMBER or BOOLEAN so
// the oracle transports it faithfully: the C-XS shim reads a string result
// from the pin's CESU-8 payload as UTF-8, which decodes an astral/lone-
// surrogate string VALUE lossily (U+FFFD) — so a string-valued completion is
// NOT a faithful differential and those semantics are proven at the endor-vm
// value layer instead (interp.rs `utf16_*` tests). Here the surrogate content
// is the INPUT and the result is a scalar the pin renders exactly.
//
// These assert RESULT parity (the governing check — divergent=0 on the
// completion value) against the pin. Cross-engine computron equality is
// NEITHER required NOR asserted: the UTF-16 recalibration re-bases string
// costs off code-unit length, so a multi-unit case may (and does) shift
// computrons vs the pin's CESU-8 byte metering. The property that must hold —
// determinism-per-release (identical endor computrons across repeated runs) —
// is asserted separately in the test, and a curated frozen-cost subset lives
// in `utf16_meter_expectations_are_the_frozen_recalibrated_costs`.

// --- charCodeAt / codePointAt across a surrogate boundary ---
// 𝒜 is U+1D49C, stored as the pair D835 DC9C.
"𝒜".length
"𝒜".charCodeAt(0)
"𝒜".charCodeAt(1)
"𝒜".codePointAt(0)
"𝒜".codePointAt(1)
"a𝒜b".length
"a𝒜b".charCodeAt(0)
"a𝒜b".charCodeAt(1)
"a𝒜b".charCodeAt(2)
"a𝒜b".charCodeAt(3)
"a𝒜b".codePointAt(0)
"a𝒜b".codePointAt(1)
"a𝒜b".codePointAt(2)
"a𝒜b".codePointAt(3)
"a𝒜b".charCodeAt(4)
"𝒜".codePointAt(0) === 119964
"a𝒜b".charCodeAt(1) === 0xD835
"a𝒜b".charCodeAt(2) === 0xDC9C

// --- index-heavy access (tight [i] / charCodeAt(i) loops) ---
// direct O(1) index at every position, including just past a supplementary char.
"a𝒜b"[0] === "a"
"a𝒜b"[3] === "b"
"a𝒜b"[4] === undefined
var s0 = "a𝒜b"; var t0 = 0; for (var i = 0; i < s0.length; i++) { t0 += s0.charCodeAt(i); } t0
var s1 = "𝒜𝒷𝒜𝒷"; var t1 = 0; for (var i = 0; i < s1.length; i++) { t1 += s1.charCodeAt(i); } t1
var s2 = "xxxxxxxxxx𝒜yyyyyyyyyy"; var c2 = 0; for (var i = 0; i < s2.length; i++) { if (s2.charCodeAt(i) >= 0xD800) { c2++; } } c2
var s3 = "𝒜".repeat(8); s3.length
var s4 = "a".repeat(50) + "𝒜" + "b".repeat(50); s4.length
var s5 = "a".repeat(50) + "𝒜" + "b".repeat(50); s5.charCodeAt(50) === 0xD835

// --- slicing across a surrogate boundary (splits a pair → lone surrogate) ---
"a𝒜b".slice(1, 2).length
"a𝒜b".slice(1, 2).charCodeAt(0)
"a𝒜b".slice(2, 3).charCodeAt(0)
"a𝒜b".slice(1, 3).length
"a𝒜b".slice(1, 3).codePointAt(0)
"a𝒜b".substring(1, 2).charCodeAt(0)
"a𝒜b".substring(2, 3).charCodeAt(0)
"a𝒜b".slice(1, 2).charCodeAt(0) === 0xD835
"a𝒜b".slice(2, 3).charCodeAt(0) === 0xDC9C

// --- string iterator / spread yields whole code points ---
[..."a𝒜b"].length
[..."a𝒜b"][1].length
[..."a𝒜b"][1].codePointAt(0)
[..."𝒜𝒷"].length
[..."abc"].length

// --- lone surrogates (WTF-16 — a JS string need not be well-formed) ---
"\uD834".length
"\uD834".charCodeAt(0)
"\uD834".codePointAt(0)
"\uDD1E".length
"\uDD1E".charCodeAt(0)
"\uDD1E".codePointAt(0)
"𝄞".length
"𝄞".charCodeAt(0)
"𝄞".codePointAt(0)
"A\uD800B".length
"A\uD800B".charCodeAt(1)
"A\uD800B"[2] === "B"
"A\uD800B".codePointAt(1)
("\uD834" + "\uDD1E").length
("\uD834" + "\uDD1E").codePointAt(0)
("A" + "\uD800" + "B").length
("A" + "\uD800" + "B").charCodeAt(1)
"\uD800\uD801".charCodeAt(0)
"\uD800\uD801".codePointAt(0)
"𝄞".codePointAt(0) === 119070
