// Stage-2b closure corpus (child 2 of the stage-2b orchestration):
// closures via heap cells — capture AND mutation, across returned inner
// functions, curried functions, captured parameters, multiple captured
// cells, and independent cells per activation. Every line is bit-exact
// (result AND computron) against the C-XS oracle.
//
// The captured binding is a shared heap cell: `new_closure` allocates it
// (`fxNewSlot`, metered), a closure-kind scope slot indirects to it,
// `store` captures it into the inner function's closure environment
// (`fxNewSlot`, metered), and `retrieve` imports it into the callee frame
// — so `get_closure`/`pull_closure` on both the defining frame and the
// closure read and write the one cell, and a mutation persists across
// calls and is visible to every capturer. Each fresh activation of the
// enclosing function allocates a fresh cell, so distinct closures over
// distinct activations do not alias (the two-counter case).
//
// One program per line; the completion value is the last expression.

// --- capture + mutation: a counter closure, called repeatedly ---
var mk=function(){var c=0; return function(){c=c+1; return c}}; var f=mk(); f()
var mk=function(){var c=0; return function(){c=c+1; return c}}; var f=mk(); f(); f()
var mk=function(){var c=0; return function(){c=c+1; return c}}; var f=mk(); f(); f(); f()

// --- capture of a parameter ---
var mk=function(n){return function(){n=n+1; return n}}; var g=mk(10); g(); g()
var add=function(x){return function(y){return x+y}}; add(3)(4)

// --- multiple captured cells in one closure ---
var mk=function(){var a=0,b=0; return function(){a=a+1; b=b+2; return a+b}}; var f=mk(); f(); f()

// --- a closure called within the enclosing scope (shared cell) ---
var out=function(){var c=5; var inc=function(){c=c+1; return c}; return inc()+inc()}; out()

// --- independent activations get independent cells (no aliasing) ---
var counter=function(){var n=0; return function(){return n=n+1}}; var c1=counter(),c2=counter(); c1(); c1(); c2()

// --- deeper currying / capture chains ---
var adder=function(x){return function(y){return function(z){return x+y+z}}}; adder(1)(2)(3)
var acc=function(){var s=0; return function(v){s=s+v; return s}}; var a=acc(); a(10); a(20); a(5)
