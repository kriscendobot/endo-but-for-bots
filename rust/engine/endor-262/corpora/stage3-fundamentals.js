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

// The real Error hierarchy (fx_Error): constructing and throwing real Error
// objects with XS's name/message/toString semantics — this graduates
// abort-value parity from primitive throws to real Error objects.
new Error()
new Error('boom')
Error('boom')
new TypeError('bad')
new RangeError('r')
new SyntaxError('s')
new ReferenceError('x')
new EvalError('e')
new URIError('u')
new Error(5)
new Error('a' + 'b')
typeof new Error('x')
(new Error('x')).message
(new Error('x')).name
var e1 = new Error('m'); e1.message
var e2 = new TypeError('t'); e2.name
throw new Error()
throw new Error('boom')
throw new TypeError('nope')
throw new RangeError('z')

// instanceof completion (fxOrdinaryHasInstance): a prototype-chain identity
// walk. Object/error/user-constructor chains, subtype relationships, and
// primitive/negative left operands.
({}) instanceof Object
({}) instanceof Error
(new Error('x')) instanceof Error
(new TypeError('t')) instanceof TypeError
(new TypeError('t')) instanceof Error
(new Error()) instanceof TypeError
(new RangeError('r')) instanceof RangeError
(1) instanceof Object
's' instanceof Object
true instanceof Object
null instanceof Object
function C0(){}; (new C0()) instanceof C0
function C1(){}; (new C1()) instanceof Object
function C2(){}; function D2(){}; (new C2()) instanceof D2

// `in` completion (fxHasAt) — the own-present case (a `true` endor can decide
// soundly; an absent/inherited key self-names rather than risk a wrong false).
'a' in {a:1}
'a' in {a:1, b:2}
'b' in {a:1, b:2}
var oi = {x:1}; 'x' in oi
var oj = {a:1, b:2, c:3}; 'c' in oj

// Primitive wrapper construction (new Boolean/Number/String) — the wrapper
// stringifies as its wrapped primitive; and the Number/String call forms.
new Boolean(1)
new Boolean(0)
new Boolean()
new Number(5)
new Number()
new Number(3.5)
new String('x')
new String()
typeof new Boolean(1)
(new Boolean(1)) instanceof Boolean
(new Number(5)) instanceof Number
Number(5)
Number(true)
Number(null)
String('hi')

// Native prototype-method dispatch (resolved up the prototype chain, called
// with the receiver as `this`): Object.prototype toString/valueOf/
// hasOwnProperty, Function.prototype.toString, Error.prototype.toString, and
// the wrapper valueOf/toString.
({}).toString()
({a:1}).valueOf()
({a:1}).hasOwnProperty('a')
(function(){}).toString()
function named1(){}; named1.toString()
(new Error('m')).toString()
(new TypeError('t')).toString()
(new Boolean(1)).valueOf()
(new Boolean(1)).toString()
(new Number(5)).toString()
(new String('hi')).toString()

// Function.prototype.call (the re-entrant trampoline): invoke the receiver
// with a rebound `this` and forwarded arguments. (A primitive thisArg needs
// sloppy `this`-boxing, not yet modeled, and self-names.)
var fc0 = function () { return 1 }; fc0.call()
var fc1 = function () { return 1 }; fc1.call(null)
var fc2 = function () { return this.x }; fc2.call({ x: 5 })
var fc3 = function (a, b) { return a + b }; fc3.call(null, 2, 3)
function idc(x) { return x }; idc.call(null, 42)
var fc4 = function (a, b, c) { return a + b + c }; fc4.call(null, 1, 2, 3)
