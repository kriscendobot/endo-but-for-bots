// Stage-3b (promises) — Promise, the job queue, and the pump-loop latch,
// bit-exact (result AND computron) against the C-XS oracle at the pin
// `48ee02d8cfe0`. xsPromise.c calls `mxMeter` exactly once in the whole file
// (the unhandled-rejection list walk, unreachable here), so promise metering is
// allocation-driven: the `fxNewSlot` clusters of `fxNewPromiseInstance` (6),
// `fxPushPromiseFunctions` (13), `fxNewPromiseCapability` (the derived promise +
// pair + 8), `fxPromiseThen`'s reaction (6, +1 THENS link when pending), and
// `fxQueueJob` (6 per job), plus each entry point's calibrated native frame and
// the reaction/executor bodies the re-entrant dispatch meters (interp.rs
// § PROMISE_* metering). The reactions run at the pump-loop drain — the
// host-driven microtask drain the endor embedding performs after the script
// settles, mirrored in the oracle shim's post-`fxRunScript` `fxRunPromiseJobs`
// loop — so a crank's cost (message delivery plus its microtask drain) is the
// consensus-relevant unit, metered on both sides.
//
// Honest NAMED skips (never a wrong value, never a divergence): thenable
// adoption (`resolve` with a reference / `Promise.resolve(object)`), a reaction
// handler that throws or returns a reference, `.finally`, the `all`/`race`/
// `allSettled`/`any` combinators, and — stage 4's charter — async/await. Each
// self-names `Halt::Unsupported`.

// --- construct: a pending promise, executor never settles -----------------
new Promise(function(r){})
new Promise(function(){})
new Promise(function(resolve, reject){})

// --- construct: the executor settles synchronously with a primitive -------
new Promise(function(resolve){ resolve(5); })
new Promise(function(resolve, reject){ reject(5); })
new Promise(function(resolve){ resolve(5); resolve(6); })
new Promise(function(a, reject){ reject(7); })

// --- Promise.resolve / Promise.reject statics -----------------------------
Promise.resolve(5)
Promise.reject(5)
Promise.resolve(0)
Promise.reject(0)

// --- .then on a pending promise: the reaction registers, never fires ------
var p1 = new Promise(function(){}); p1.then(function(v){})
var p2 = new Promise(function(){}); p2.then(function(v){}, function(e){})

// --- .then on an already-settled promise: one job, drained ----------------
var x = 0; Promise.resolve(1).then(function(v){ x = v; }); x
var p3 = new Promise(function(resolve){ resolve(1); }); p3.then(function(v){})
var x = 5; var p4 = new Promise(function(res){ res(3); }); p4.then(function(v){ x = v; }); x

// --- .then whose executor resolves AFTER the reaction is registered -------
var x = 0; var r; new Promise(function(res){ r = res; }).then(function(v){ x = v; }); r(9); x

// --- reaction returns a primitive: the derived promise chains -------------
var x = 0; Promise.resolve(1).then(function(v){ return v + 1; }).then(function(v){ x = v; }); x
var x = 0; Promise.resolve(1).then(function(v){ x = v; return v * 2; }).then(function(w){ x = x + w; }); x

// --- pass-through: a fulfilled promise through a handler-less then --------
var x = 0; Promise.resolve(1).then().then(function(v){ x = v; }); x
var x = 0; Promise.resolve(1).then(undefined, function(e){}).then(function(v){ x = v; }); x

// --- rejection reaches the onRejected handler -----------------------------
var x = 0; Promise.reject(7).then(undefined, function(e){ x = e; }); x
var x = 0; Promise.reject(2).then(function(v){ x = 1; }).then(undefined, function(e){ x = e; }); x

// --- .catch(onRejected) === .then(undefined, onRejected) ------------------
var x = 0; Promise.reject(7).catch(function(e){ x = e; }); x
var x = 0; Promise.resolve(1).catch(function(e){ x = e; }).then(function(v){ x = v; }); x
var x = 0; Promise.reject(2).catch(function(e){ return e + 1; }).then(function(v){ x = v; }); x
