// Stage-4b (the async-function surface) — the ASYNC_FUNCTION/START_ASYNC/AWAIT
// opcode surface over the promise keystone, bit-exact (result AND computron)
// against the C-XS oracle at the pin `48ee02d8cfe0`. Each program's completion
// value is read synchronously (before the microtask drain), so a resumed body's
// effect is not visible in the completion — but the whole crank, INCLUDING the
// reactions and the async resume run at the pump-loop drain, is metered on both
// sides, so a divergence in the resume path shows as a computron gap.
//
// The surface exercised (see interp.rs § async-function metering,
// ASYNC-AWAIT-HANDOFF.md): the async function define (no own .prototype;
// ASYNC_FUNCTION_DEFINE_DELTA), START_ASYNC (new_async_instance + the result
// promise; ASYNC_INSTANCE_METERING), AWAIT (the YIELD-shaped suspend; reuses
// GENERATOR_YIELD_METERING), the AsyncAwait native reaction resume at the drain,
// and await_schedule's two branches — the primitive/general path
// (ASYNC_AWAIT_GENERAL_METERING) and the native-promise fast path
// (ASYNC_AWAIT_FASTPATH_CREDIT). `await` inside a live `try` is the designated
// named skip (await:await-in-try), excluded from this corpus.
//
// The synchronous prefix of an async body runs to the first await/completion:
var x=0; async function f(){ x=1; return 42; } f(); x
// A plain await of a primitive: the body suspends one microtask turn, the
// completion "ac" is read before the drain runs the resume ("b10"):
var log=""; async function f(){ log+="a"; var y = await 10; log+="b"+y; } f(); log+="c"; log
// Two awaits: two suspend/resume turns through the drain:
var log=""; async function f(){ log+="1"; await 0; log+="2"; await 0; log+="3"; } f(); log+="!"; log
// A bare async call returning a value, no await (one settle at completion):
var p; async function f(){ return 5; } p=f(); 0
// A single await of a primitive:
var p; async function f(){ await 1; return 5; } p=f(); 0
// Two awaits in a row:
var p; async function f(){ await 1; await 1; return 5; } p=f(); 0
// The native-promise fast path: await of a Promise.resolve(v):
var p; async function f(){ var y = await Promise.resolve(7); return y; } p=f(); 0
// A bare await of a native promise:
var p; async function f(){ await Promise.resolve(1); return 5; } p=f(); 0
// Nested async: awaiting the result promise of another async call (fast path):
var p; async function g(){ return 3; } async function f(){ return await g(); } p=f(); 0
// An async arrow with an await in its expression body:
var f = async x => x + await 1; f(2); 0
// Awaiting a rejected promise: the rejection reaches the result promise (an
// unhandled rejection at the drain; the completion is still read before it):
var p; async function f(){ await Promise.reject(1); } p=f(); 0
// Await inside a loop — three suspend/resume turns:
var s=0; async function f(){ for (var i=0;i<3;i++){ s += await i; } } f(); s
// Await of a thenable object (adoption via the await general path):
var p; async function f(){ return await { then: function(r){ r(9); } }; } p=f(); 0
// An empty-return async body (settles undefined):
var log=""; async function f(){ log += "z"; return; } f(); log
