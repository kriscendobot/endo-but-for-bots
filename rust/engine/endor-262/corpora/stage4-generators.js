// Stage-4 child 3/8: generator functions and the iteration protocol
// (the pin's `xsGenerator.c` sync half). Each line is one program, bit-exact
// (result AND computron) against the C-XS pin. The suspend/resume of the
// interpreter activation is heap state in a `generators` side table: a
// generator instance holds its lifecycle state (suspended-start /
// suspended-yield / executing / completed) and its saved frame (scope + own
// value-stack temporaries + resume cursor). `START_GENERATOR` snapshots the
// fresh activation and returns the instance; `YIELD` snapshots and unwinds to
// the `.next` driver; `.next(v)` reinstalls the frame and runs a nested
// dispatch to the next `yield` (the `BRANCH_STATUS` resume epilogue) or `END`.
//
// Metering is allocation-driven (xsGenerator.c calls no `mxMeter`): each
// generator operation carries a calibrated frozen constant over the identical
// bytecode both engines dispatch — a generator-function define's extra
// `.prototype` cluster, `START_GENERATOR`'s instance slots, `YIELD`'s
// saved-stack chunk, a completion `fxNewGeneratorResult`, and the per-resume
// re-entry residual. A *yield*'s `{value, done}` object is built by the body's
// own `OBJECT`/`NEW_PROPERTY` bytecode, so it carries no result constant.
//
// Honest named skips (self-naming `Halt::Unsupported`, never a wrong value or a
// silent divergence): `yield*` delegation (`YIELD_STAR`), `.throw(e)` and
// `.return(v)` into a *suspended* body (throw-into-suspended / finally
// unwinding through the catch/finally jump chain), a `yield` inside a live
// `try` (`generator:yield-in-try`), a generator built with `new`
// (`generator:new-target`), and async generators / `await` (child 4).

// --- next() yields the sequence, then completes ---
function* g(){ yield 1; yield 2; } var a=g(); a.next().value;
function* g(){ yield 1; yield 2; } var a=g(); a.next(); a.next().value;
function* g(){ yield 1; yield 2; } var a=g(); a.next(); a.next(); a.next().done;
function* g(){ yield 1; } var a=g(); var r=a.next(); r.value + "," + r.done;
function* g(){ } var a=g(); a.next().done;

// --- return value: {value: retval, done: true} on completion ---
function* g(){ yield 1; return 9; } var a=g(); a.next(); var r=a.next(); r.value+","+r.done;
function* g(){ return 42; } var a=g(); var r=a.next(); r.value + "/" + r.done;

// --- the sent value is the yield expression's result ---
function* g(){ var x = yield 1; yield x + 5; } var a=g(); a.next(); a.next(7).value;
function* g(){ var s=0; s += yield 1; s += yield 2; return s; } var a=g(); a.next(); a.next(10); a.next(20).value;

// --- for-of drives next() until done ---
function* g(){ yield 1; yield 2; yield 3; } var s=0; for (var v of g()) s += v; s;
function* g(){ yield 10; yield 20; } var r=[]; for (var v of g()) r.push(v); r.join(",");
function* g(){ for (var i=0;i<3;i++) yield i*i; } var s=0; for (var v of g()) s+=v; s;
function* count(n){ var i=0; while(i<n) yield i++; } var s=0; for (var v of count(4)) s+=v; s;

// --- spread over a generator ---
function* g(){ yield 1; yield 2; yield 3; } var a=[...g()]; a.length + ":" + a.join("-");

// --- generator function expression ---
var g = function*(){ yield 5; yield 6; }; var a=g(); a.next().value + a.next().value;

// --- return() before the first next / on a suspended-start body ---
function* g(){ yield 1; yield 2; } var a=g(); a.return(99).value;
function* g(){ yield 1; } var a=g(); a.next(); a.next(); a.next().done;

// --- object-literal generator method `*m()` (named via fxRenameFunction) ---
var o = { *gen(){ yield 1; yield 2; } }; var a=o.gen(); a.next().value + a.next().value;
var o = { *gen(){ yield 5; } }; o.gen.name;

// --- the generator object exposes the prototype methods ---
function* g(){ yield "a"; yield "b"; } var a=g(); typeof a.next;
