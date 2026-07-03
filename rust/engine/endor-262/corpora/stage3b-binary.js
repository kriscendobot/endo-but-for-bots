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

// --- TypedArray family: construct over a length, element access, accessors.

// Length-form construct + length/byteLength accessors across element widths.
new Uint8Array(8).length
new Int8Array(8).length
new Uint8ClampedArray(8).length
new Int16Array(4).length
new Uint16Array(4).byteLength
new Int32Array(4).byteLength
new Uint32Array(4).length
new Float32Array(3).byteLength
new Float64Array(2).byteLength
new Uint8Array().length
new Int32Array(0).length

// typeof a TypedArray instance and its constructor.
typeof new Uint8Array(4)
typeof Uint8Array

// Element write then read — the exotic index behavior, per element type.
var a = new Uint8Array(4); a[0] = 42; a[0]
var a2 = new Int32Array(3); a2[1] = 1000; a2[2] = 2000; a2[1] + a2[2]
var a3 = new Float64Array(2); a3[0] = 3.5; a3[1] = 1.25; a3[0] + a3[1]
var a4 = new Float32Array(1); a4[0] = 1.5; a4[0]

// Coercion corners: Uint8 wrap, Int8 wrap, Uint8Clamped clamp + round-half-even.
var w = new Uint8Array(1); w[0] = 300; w[0]
var s = new Int8Array(1); s[0] = 200; s[0]
var c = new Uint8ClampedArray(1); c[0] = 300; c[0]
var c2 = new Uint8ClampedArray(1); c2[0] = -5; c2[0]
var c3 = new Uint8ClampedArray(1); c3[0] = 2.5; c3[0]
var n = new Int16Array(1); n[0] = -1; n[0]
var u = new Uint32Array(1); u[0] = 4294967295; u[0]

// A zero-initialized element reads back 0; an out-of-bounds index is undefined.
var z = new Uint8Array(2); z[0]
var o = new Uint8Array(2); o[5] = 9; o[5]

// Buffer-form construct: a view over an existing ArrayBuffer (shared store).
var b3 = new ArrayBuffer(16); new Int32Array(b3).length
var b4 = new ArrayBuffer(16); new Int32Array(b4, 4).length
var b5 = new ArrayBuffer(16); new Int32Array(b5, 4, 2).byteLength
var b6 = new ArrayBuffer(8); var v6 = new Int32Array(b6, 4); v6.byteOffset
var b7 = new ArrayBuffer(8); var v7 = new Uint8Array(b7); v7.buffer === b7

// Fill a small typed array in a loop (the metering hot path).
var g = new Uint8Array(4); var i = 0; while (i < 4) { g[i] = i * 2; i = i + 1; } g[0] + g[1] + g[2] + g[3]
