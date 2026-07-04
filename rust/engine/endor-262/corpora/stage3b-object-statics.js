// Stage-3b child 5/9: the global runtime string->id intern table +
// Object statics / verifyProperty machinery. Each line is one program,
// bit-exact (result AND computron) against the C-XS pin.

// --- hasOwnProperty over the global intern table ---
// An own key (always a program symbol) is found: true.
var o = {a:1}; o.hasOwnProperty("a");
var o = {a:1,b:2}; o.hasOwnProperty("a") && o.hasOwnProperty("b");
// A genuinely-novel key misses the table, interning one metered key slot,
// and is not an own property: false.
var o = {a:1}; o.hasOwnProperty("b");
var o = {a:1}; o.hasOwnProperty("zzz");
// A well-known inherited name (a boot default key) interns without
// allocating and is correctly not an OWN property: false.
var o = {a:1}; o.hasOwnProperty("toString");
var o = {a:1}; o.hasOwnProperty("valueOf");
var o = {a:1}; o.hasOwnProperty("hasOwnProperty");
// The intern table is global and persistent: a repeated novel key does not
// re-allocate a second key slot.
var o = {a:1}; o.hasOwnProperty("zzz"); o.hasOwnProperty("zzz");
// Two distinct novel keys each intern their own slot.
var o = {a:1}; o.hasOwnProperty("zzz"); o.hasOwnProperty("yyy");
// A present own key that is only reachable by a computed string.
var o = {foo:10, bar:20}; o.hasOwnProperty("foo");
var o = {foo:10, bar:20}; o.hasOwnProperty("baz");
