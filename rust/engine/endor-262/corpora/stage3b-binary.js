// Stage-3b binary-data corpus (child 3/9): ArrayBuffer per the pin
// 48ee02d8cfe0 — construct over a byteLength, the byteLength accessor
// getter, and the zero-fill. One program per line; bit-exact (completion
// value AND computron count) against the C-XS oracle.

// Construct + byteLength accessor over a range of sizes (the frame is a
// constant; the backing-store chunk scales with the 8-byte-aligned size).
new ArrayBuffer(0).byteLength
new ArrayBuffer(1).byteLength
new ArrayBuffer(5).byteLength
new ArrayBuffer(8).byteLength
new ArrayBuffer(9).byteLength
new ArrayBuffer(16).byteLength
new ArrayBuffer(64).byteLength
new ArrayBuffer(255).byteLength
new ArrayBuffer(256).byteLength
new ArrayBuffer(1024).byteLength

// A missing argument defaults the byteLength to 0.
new ArrayBuffer().byteLength

// The buffer is a first-class object: bound to a variable, read back.
var b = new ArrayBuffer(32); b.byteLength
var b2 = new ArrayBuffer(12); b2.byteLength + b2.byteLength

// typeof an ArrayBuffer instance is "object"; the constructor is a function.
typeof new ArrayBuffer(4)
typeof ArrayBuffer

// Two buffers are independent objects (distinct byte lengths).
var p = new ArrayBuffer(3); var q = new ArrayBuffer(7); p.byteLength + q.byteLength

// A byteLength read repeated adds only its dispatch (the getter meters nothing).
var r = new ArrayBuffer(10); r.byteLength; r.byteLength; r.byteLength
