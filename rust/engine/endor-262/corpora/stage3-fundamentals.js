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

// Boolean results compose through the covered primitive grammar.
!Boolean(0)
!!Boolean('')
typeof Boolean(1)
Boolean(1) === true
Boolean(0) === false
Boolean(1) && Boolean(0)
var b = Boolean(1); b
var t = typeof Boolean; t
