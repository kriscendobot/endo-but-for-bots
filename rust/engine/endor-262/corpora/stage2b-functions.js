// Stage-2b user-function corpus (child 2 of the stage-2b orchestration):
// user functions end to end on child 1's allocation-faithful heap —
// `constructor_function`/`function` + `code` + `function_environment`
// definition, `call`/`run` frame switching with `argument` binding, and
// `end` popping into the calling frame. Every line is bit-exact (result
// AND computron) against the C-XS oracle: the call machinery is
// stack-based (dispatch-metered only), and the definition allocations
// (the function instance and default prototype, the body chunk, the name,
// and the per-declared-local cell) are metered faithfully at their C-XS
// sites (`endor_vm::interp` § the FUNCTION_* constants).
//
// One program per line; the completion value is the last expression.

// --- immediately-invoked function expressions, returning a value ---
(function(){return 1})()
(function(){return 1+2})()
(function(x){return x})(5)
(function(x){return x+1})(5)
(function(x){return x*x})(9)
(function(x,y){return x+y})(5,6)
(function(x,y){return x-y})(10,3)
(function(x,y,z){return x+y+z})(1,2,3)
(function(){})()

// --- local variables inside a function body ---
(function(){var a=1; return a})()
(function(){var a=1,b=2; return a+b})()
(function(){var a=1,b=2,c=3; return a+b+c})()
(function(x){var a=1; return x+a})(5)
(function(x,y){var t=x*y; return t+x})(3,4)

// --- arguments beyond / short of the parameter list ---
(function(x){return x})(5,6,7)
(function(x,y){return x})(5)

// --- functions stored in variables, then called ---
var f=function(x){return x*2}; f(3)
var f=function(x){return x*2}; f(3); f(10)
var g=function(a,b){return a*b+1}; g(6,7)

// --- named function declarations (hoisted), then called ---
function h(){return 42} h()
function sq(n){return n*n} sq(12)
function add(a,b){return a+b} add(20,22)

// --- nested calls: a function called inside another function's body ---
(function(){return (function(){return 1})()})()
(function(x){return (function(y){return y+1})(x)+x})(4)
(function(){var a=(function(){return 3})(); return a*a})()

// --- recursion (self-reference resolved through the hoisted global) ---
function fac(n){return n<2?1:n*fac(n-1)} fac(1)
function fac(n){return n<2?1:n*fac(n-1)} fac(3)
function fac(n){return n<2?1:n*fac(n-1)} fac(6)
function fac(n){return n<2?1:n*fac(n-1)} fac(8)
function fib(n){return n<2?n:fib(n-1)+fib(n-2)} fib(7)
function sum(n){return n<1?0:n+sum(n-1)} sum(10)

// --- functions called from a loop (calls do not accrue per-definition cost) ---
var s=0,i=0,f=function(x){return x*x}; while(i<5){s=s+f(i);i=i+1} s
var t=1,k=1,dbl=function(x){return x+x}; while(k<6){t=dbl(t);k=k+1} t
