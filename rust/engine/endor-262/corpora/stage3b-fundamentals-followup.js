// Stage-3b fundamentals-followup corpus (child 4/9): the post-arrays
// fundamentals follow-up per the pin 48ee02d8cfe0. One program per line;
// bit-exact (completion value AND computron count) against the C-XS oracle.

// --- Function .length (the declared arity) ---------------------------
// XS sets `.length` from `begin`'s parameter-count operand in the `code`
// opcode (`fxNewFunctionLength(the, variable, *(code+1))`); reading the own
// `length` data property meters nothing beyond the GET_PROPERTY dispatch.
function f0(){} f0.length
function f1(a){} f1.length
function f2(a,b){} f2.length
function f5(a,b,c,d,e){} f5.length
function add(a,b){return a+b} add.length
var fe=function(a,b){}; fe.length
var o={m:function(a,b,c){}}; o.m.length
function f(a){} function g(a,b,c){} f.length + g.length
function f2b(a,b){} f2b.length + f2b.length

// --- Function .name (the own name; inferred for a var initializer) ---
function foo(){} foo.name
function longNameHere(){} longNameHere.name
var fn=function(){}; fn.name
var g3=function(x,y,z){}; g3.name
function add2(a,b){return a+b} add2.name
function rec(n){return n} rec.name
function both(a,b){} var n=both.name; var l=both.length; l

// --- Function.prototype.apply with a real (dense) array argument -----
// XS reads the array's `length` (`mxGetID`), then each element
// (`mxGetIndex`), and forwards them as the call arguments; the array-path
// setup + per-element read/forward is the measured
// APPLY_ARRAY_BASE_METERING + n * APPLY_ARRAY_PER_ELEMENT_METERING.
function add(a,b){return a+b} add.apply(undefined,[3,4])
function add3(a,b,c){return a+b+c} add3.apply(null,[1,2,3])
function first(a){return a} first.apply(undefined,[42])
function none(){return 7} none.apply(undefined,[])
function sum4(a,b,c,d){return a+b+c+d} sum4.apply(undefined,[1,2,3,4])
function mul(a,b){return a*b} var args=[6,7]; mul.apply(undefined,args)
function sub3(a,b,c){return a-b-c} sub3.apply(undefined,[10,2,3])
function extra(a,b){return b} extra.apply(undefined,[1,2,3])
function pick5(a,b,c,d,e){return a+e} pick5.apply(undefined,[1,2,3,4,5])
function id1(a){return a} var arr=[9]; id1.apply(null,arr)

// --- Symbol.prototype.toString + String(symbol) coercion ------------
// fxSymbolToString builds "Symbol(" + description + ")"; String(sym) is
// the one explicit symbol->string coercion the spec allows.
Symbol("d").toString()
Symbol().toString()
var sym=Symbol("hi"); sym.toString()
Symbol("abcdefgh").toString()
String(Symbol("y"))
String(Symbol())
var z=Symbol("z"); z.valueOf()===z

// --- Symbol.for / Symbol.keyFor (the global registry) ---------------
// Symbol.for(k) returns the same symbol identity on repeat calls;
// keyFor recovers a registered symbol's key, or undefined for a local.
Symbol.for("k")===Symbol.for("k")
Symbol.for("a")===Symbol.for("b")
Symbol.keyFor(Symbol.for("registered"))
typeof Symbol.keyFor(Symbol("local"))
Symbol.for("x").toString()
var gg=Symbol.for("gg"); Symbol.keyFor(gg)
String(Symbol.for("k"))
Symbol("a")===Symbol("a")

// --- AggregateError -------------------------------------------------
// AggregateError(errors, message): the base error (message from arg 1)
// plus an own `errors` Array built by iterating arg 0 (dense-array form).
new AggregateError([]).name
new AggregateError([], "oops").message
new AggregateError([]).errors.length
var errs3=[1,2,3]; new AggregateError(errs3).errors.length
var errs2=[10,20]; var ae=new AggregateError(errs2,"m"); ae.errors.length
new AggregateError([]) instanceof Error
new AggregateError([1,2]) instanceof AggregateError
var aef=new AggregateError([7,8,9]); aef.errors[0]+aef.errors[2]
AggregateError([], "x").message
var ae1=new AggregateError([5]); ae1.errors[0]
new AggregateError([], "boom").name

// --- Function.prototype.bind ----------------------------------------
// bind creates a bound function: .length = max(0, target.length - bound),
// .name = "bound "+name; calling it prepends the bound this + bound args.
function fb2(a,b){return a+b} var gb0=fb2.bind(undefined); gb0(3,4)
function fb2b(a,b){return a+b} var gb1=fb2b.bind(undefined,10); gb1(5)
function fb3(a,b,c){return a+b+c} var gb2=fb3.bind(null,1,2); gb2(3)
function flen(a,b){return a+b} flen.bind(undefined).length
function flen2(a,b){return a} flen2.bind(undefined,9).length
function fname(a){} fname.bind(undefined).name
function fthis(){return this.v} var ov={v:42}; var gt=fthis.bind(ov); gt()
function fadd(x,y,z){return x+y+z} var ga=fadd.bind(null); ga(1,2,3)
function f4(a,b,c,d){return a+b+c+d} var g4=f4.bind(undefined,1,2,3,4); g4()
function f5(a,b,c,d,e){return 0} f5.bind(undefined,1,2).length
function fmul(a,b){return a*b} var gm=fmul.bind(null,6); gm(7)
function fd(a){return a} var gd=fd.bind(undefined,5); var hd=gd.bind(undefined); hd.name
