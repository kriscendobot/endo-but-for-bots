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
