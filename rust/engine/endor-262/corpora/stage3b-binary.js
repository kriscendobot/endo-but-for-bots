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

// ArrayBuffer.isView over a view, a buffer, and a primitive.
ArrayBuffer.isView(new Uint8Array(4))
ArrayBuffer.isView(new ArrayBuffer(4))
ArrayBuffer.isView(42)

// --- DataView: construct over a buffer + endian-aware get/set.

// Construct + byteLength/byteOffset/buffer accessors.
var dvb = new ArrayBuffer(8); new DataView(dvb).byteLength
var dvb2 = new ArrayBuffer(8); new DataView(dvb2, 2).byteOffset
var dvb3 = new ArrayBuffer(8); new DataView(dvb3, 2, 4).byteLength
var dvb4 = new ArrayBuffer(8); var dv4 = new DataView(dvb4); dv4.buffer === dvb4

// get/set per type; big-endian (default) round-trips.
var d1 = new DataView(new ArrayBuffer(8)); d1.setInt8(0, 100); d1.getInt8(0)
var d2 = new DataView(new ArrayBuffer(8)); d2.setUint8(0, 200); d2.getUint8(0)
var d3 = new DataView(new ArrayBuffer(8)); d3.setInt16(0, 12345); d3.getInt16(0)
var d4 = new DataView(new ArrayBuffer(8)); d4.setUint16(0, 60000); d4.getUint16(0)
var d5 = new DataView(new ArrayBuffer(8)); d5.setInt32(0, 1000000); d5.getInt32(0)
var d6 = new DataView(new ArrayBuffer(8)); d6.setUint32(0, 4000000000); d6.getUint32(0)
var d7 = new DataView(new ArrayBuffer(8)); d7.setFloat32(0, 1.5); d7.getFloat32(0)
var d8 = new DataView(new ArrayBuffer(8)); d8.setFloat64(0, 3.141592653589793); d8.getFloat64(0)

// Endianness: little-endian round-trip, and a cross-endian read.
var e1 = new DataView(new ArrayBuffer(8)); e1.setInt16(0, 256, true); e1.getInt16(0, true)
var e2 = new DataView(new ArrayBuffer(8)); e2.setInt16(0, 256); e2.getInt16(0, true)
var e3 = new DataView(new ArrayBuffer(8)); e3.setUint32(0, 1, true); e3.getUint32(0)

// A big-endian write is observable byte-for-byte through a Uint8Array view.
var cb = new ArrayBuffer(4); var cv = new DataView(cb); cv.setInt32(0, 66051); new Uint8Array(cb)[0]
var cb2 = new ArrayBuffer(4); var cv2 = new DataView(cb2); cv2.setInt32(0, 66051, true); new Uint8Array(cb2)[0]

// A write at a nonzero offset reads back at that offset.
var f1 = new DataView(new ArrayBuffer(8)); f1.setInt32(4, 777); f1.getInt32(4)
