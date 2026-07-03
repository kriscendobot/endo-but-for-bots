// Stage-3 child-2 (fundamentals) corpus: the intrinsic constructors as
// first-class values, `typeof` over them, and the `Boolean` primitive
// coercion — every line bit-exact (result AND computron) against the C-XS
// oracle. Built-in construction (`new`), `instanceof`/`in`, and the
// object-returning constructor calls land in later increments; they are
// honestly skipped (self-naming) until then, never faked here.

// The fundamentals constructors bind as global native-function values, and
// stringify through Function.prototype.toString exactly as C-XS renders a
// host function.
Object
Function
Boolean
Symbol
Number
String
Error
EvalError
RangeError
ReferenceError
SyntaxError
TypeError
URIError

// typeof of a native constructor is "function" (pure dispatch).
typeof Object
typeof Boolean
typeof Symbol
typeof Error
typeof TypeError
typeof Number
typeof String
typeof Function

// Boolean(value): ToBoolean, metering-neutral beyond the call's dispatch.
Boolean(1)
Boolean(0)
Boolean(-1)
Boolean(0.0)
Boolean('')
Boolean('x')
Boolean('0')
Boolean(null)
Boolean(2 > 1)
Boolean(3 < 1)
Boolean(Boolean(1))

// The primitive value globals — undefined/NaN/Infinity — read with no
// built-in step (pure dispatch).
undefined
NaN
Infinity
typeof undefined
typeof NaN
typeof Infinity
-Infinity
NaN !== NaN
undefined === undefined
Boolean(undefined)
Boolean(NaN)

// Boolean results compose through the covered primitive grammar.
!Boolean(0)
!!Boolean('')
typeof Boolean(1)
Boolean(1) === true
Boolean(0) === false
Boolean(1) && Boolean(0)
var b = Boolean(1); b
var t = typeof Boolean; t

// Constructor calls (`new f()`): the construct frame geometry — `new`'s
// uninitialized `this` placeholder, fxRunConstructor's fresh instance,
// `end` returning `this` — with the fixed host-frame metering. Native
// (wrapper) constructors like `new Boolean` are a later increment.
function F0() {}; new F0()
function F1() { this.x = 1 }; var o1 = new F1(); o1.x
function F2(a) { this.x = a }; (new F2(5)).x
function F3() { this.a = 1; this.b = 2 }; var o3 = new F3(); o3.a + o3.b
function F4() { return 7 }; new F4()
function P(x) { this.x = x }; function mk() { return new P(9).x }; mk()
function Pair(a, b) { this.a = a; this.b = b }; var p = new Pair(3, 4); p.a * p.b

// The native Object constructor — empty-object call and construct forms
// (fx_Object), both allocating a fresh ordinary object.
Object()
new Object()
typeof Object()
Object().x
new Object().x
var oo = Object(); oo.a = 1; oo.a
var on = new Object(); on.x = 5; on.x
