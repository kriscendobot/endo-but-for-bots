// Stage-2 BEHAVIORAL corpus (result agreement, not yet computron-exact).
// Exercises the program frame, scope slots, var bindings, and
// backward-branch control flow (loops) over compiler-emitted bytecode.
// See endor_262::stage2_behavioral_corpus for why these are result-only.
// One program per line; the last expression is the completion value.

// --- var bindings and scope ---
var x = 5; x + 1
var a = 10; var b = 20; a * b - 5
var x = 3; x = x + 4; x
var a = 1, b = 2, c = 3; a + b + c

// --- while loops (backward branch) ---
var i = 0; while (i < 3) i = i + 1; i
var i = 0; while (i < 5) i = i + 1; i
var s = 0; var i = 0; while (i < 4) { s = s + i; i = i + 1 } s

// --- for loops ---
var s = 0; for (var i = 0; i < 4; i = i + 1) s = s + i; s
var p = 1; for (var i = 1; i < 5; i = i + 1) p = p * i; p

// --- do-while ---
var s = 0, i = 0; do { s = s + i; i = i + 1 } while (i < 4); s

// --- nested conditionals feeding a loop ---
var n = 0; for (var i = 0; i < 6; i = i + 1) { if (i > 2) n = n + i } n
