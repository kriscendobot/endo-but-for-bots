// Stage-4 (async/await over the job queue) — the promise KEYSTONE: the
// native-handler double-settle calibration and the surfaces it unblocks, all
// bit-exact (result AND computron) against the C-XS oracle at the pin
// `48ee02d8cfe0`. The stage-3b promises child left thenable adoption and the
// combinators blocked on calibrating the pin's two-level `[[AlreadyResolved]]`
// guard behavior: resolving a promise with a thenable does NOT settle it — it
// acquires a *second* resolving pair (with its own fresh guard) and queues a
// `PromiseResolveThenableJob`, so the promise stays pending until the thenable
// settles it, and the guard makes every double-settle (res twice, res+rej,
// rej+res) a metered no-op. This child instruments the pin and freezes those
// costs:
//   - `mxGetID(_then)` probe on any reference resolve = one dispatch (1<<16);
//   - the thenable-branch frame residual (PROMISE_RESOLVE_THENABLE_METERING);
//   - the `fxOnThenable` drain-job frame (PROMISE_THENABLE_JOB_FRAME_METERING);
//   - `Promise.resolve(nativePromise)` identity = `mxGetID(_constructor)` +
//     `fxIsSameValue` (2.5<<16).
// interp.rs § PROMISE_* metering. The reactions run at the pump-loop drain (the
// host-driven microtask drain the endor embedding performs after a crank), so a
// crank's whole cost is the consensus-relevant unit, metered on both sides.
//
// Honest NAMED skips (never a wrong value, never a divergence): a reaction
// handler / thenable `then` that THROWS (`promise:handler-throw` /
// `promise:thenable-then-throw` — the clean re-entrant-throw unwind is the
// throw-family increment), `resolve(promise-itself)` (`promise:resolve-self`, a
// catchable TypeError), and — the stage-4 async-function surface deferred to a
// follow-up child — `async function`/`await`/`for-await-of`/async generators.

// --- thenable adoption: Promise.resolve(thenable) -------------------------
var x=0; Promise.resolve({then:function(res){res(7)}}).then(function(v){x=v}); x
var x=0; var t={then:function(res){res(7)}}; Promise.resolve(t).then(function(v){x=v}); x
var x=0; Promise.resolve({then:function(res){res(7)}}); x
var x=0; Promise.resolve({then:function(res,rej){rej(3)}}).then(function(v){x=1},function(e){x=e}); x

// --- non-thenable references fulfill with the object (the `.then` probe) ---
var x=0; Promise.resolve({}).then(function(v){x=1}); x
var x=0; Promise.resolve({then:5}).then(function(v){x=1}); x

// --- thenable adoption: an executor resolving with a thenable -------------
var x=0; new Promise(function(res){res({then:function(r){r(9)}})}).then(function(v){x=v}); x
var x=0; var r; var p=new Promise(function(res){r=res}); p.then(function(v){x=v}); r({then:function(res){res(55)}}); x

// --- THE double-settle keystone: a thenable that settles twice ------------
var x=0; Promise.resolve({then:function(res,rej){res(7);res(8)}}).then(function(v){x=v}); x
var x=0; Promise.resolve({then:function(res,rej){res(7);rej(9)}}).then(function(v){x=v},function(e){x=e}); x
var x=0; Promise.resolve({then:function(res,rej){rej(9);res(7)}}).then(function(v){x=v},function(e){x=e}); x

// --- a resolving pair's own double-settle (res twice from an executor) -----
new Promise(function(resolve){ resolve(5); resolve(6); })
var x=0; var a,b; new Promise(function(res,rej){a=res;b=rej}); x

// --- long then-chains: each handler's result feeds the next reaction -------
var x=0; Promise.resolve(1).then(function(v){return v+1}).then(function(v){return v+1}).then(function(v){x=v}); x
var x=0; Promise.resolve(1).then(function(v){return v*2}).then(function(v){return v*2}).then(function(v){return v*2}).then(function(v){x=v}); x

// --- a handler that RETURNS a thenable: the derived adopts it --------------
var x=0; Promise.resolve(1).then(function(v){return {then:function(r){r(v+40)}}}).then(function(v){x=v}); x
var x=0; new Promise(function(res){res(1)}).then(function(v){return {then:function(r){r(v)}}}).then(function(v){x=v}); x

// (A handler returning a NATIVE promise — adopting a value whose `.then` is
// `%Promise.prototype%.then` — is the honest named skip
// `promise:adopt-native-thenable`: it needs the resolving functions registered
// as native reaction handlers, a separate increment. Object-literal thenables
// with a user `then` are covered above.)

// --- Promise.resolve identity over a native promise -----------------------
var x=0; Promise.resolve(Promise.resolve(3)).then(function(v){x=v}); x
var p=Promise.resolve(3); Promise.resolve(p)
