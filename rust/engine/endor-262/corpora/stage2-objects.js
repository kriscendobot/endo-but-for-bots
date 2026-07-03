// Stage-2b object/property corpus: object literals, own-property
// get/set/define, over the allocation-faithful object heap. BIT-EXACT
// (result AND computron) against the C-XS oracle: the instance
// (fxNewObject), each own-property allocation (fxNewSlot + property-table
// growth, 536), the object-literal define (NEW_PROPERTY: +one built-in
// step), and the dynamic assignment (SET_PROPERTY create/overwrite) are
// metered exactly where XS accrues them, and property reads (GET_PROPERTY,
// like GET_VARIABLE) meter nothing. See endor_262::stage2_corpus. One
// program per line; the last expression is the completion value.

// --- empty object + dynamic property creation (SET_PROPERTY) ---
var o = {}; 1
var o = {}; o.a = 5; o.a
var o = {}; o.a = 1; o.b = 2; o.a + o.b
var o = {}; o.a = 1; o.b = 2; o.c = 3; o.a + o.b + o.c

// --- object literals (NEW_PROPERTY) ---
var o = {a: 1}; o.a
var o = {a: 1, b: 2}; o.a + o.b
var o = {x: 1, y: 2, z: 3}; o.x + o.y + o.z

// --- overwrite an existing own property (SET_PROPERTY, no allocation) ---
var o = {a: 1}; o.a = 5; o.a
var o = {a: 10}; o.a = o.a + 5; o.a
var o = {a: 1}; o.a = o.a + 1; o.a = o.a + 1; o.a

// --- two objects; distinct instances on the heap ---
var o = {a: 1}; var p = {b: 2}; o.a + p.b

// --- object property mutated across a loop (heap + control flow) ---
var o = {n: 0}; var i = 0; while (i < 4) { o.n = o.n + i; i = i + 1 } o.n
