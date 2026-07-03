// Stage-2b exception corpus (child 3 of the stage-2b orchestration):
// exceptions as XS's jump-buffer chain — try/catch/finally, throw, and
// uncaught propagation to the host boundary — over compiler-emitted
// bytecode. Every line is bit-exact (result/thrown-value AND computron)
// against the C-XS oracle.
//
// `catch` pushes a jump (target + stack/scope/frame cuts) onto the chain;
// `throw` sets `mxException` and `fxJump`s to the innermost jump, which
// restores those cuts and resumes at the catch target with a meter check;
// `uncatch` pops the chain when the try body completes normally;
// `exception` binds the thrown value into the catch clause. `fxJump` and
// the `c_malloc(txJump)` are unmetered, so a caught throw carries only its
// per-opcode dispatch metering (`endor_vm::interp` § exceptions). An
// UNCAUGHT throw escapes every JS jump to the host: the escaping opcode is
// not metered (its `mxBreak` is bypassed by the longjmp) and a fixed
// host-boundary constant is accrued (§ THROW_HOST_ESCAPE_METERING), so the
// oracle's run-only computron count at the throw and its `String(exception)`
// both match endor's `Halt::Throw` — the tightened shared-abort bar
// (`endor_262::DualRun::is_bit_exact`, observation 3).
//
// One program per line; the completion value is the last expression, or —
// for an uncaught throw — the thrown value coerced to `String()`.

// --- try with no throw: the handler is never entered ---
try { 1 } catch (e) { 2 }
try { 1 + 2 } catch (e) { 99 }

// --- catch binds and uses the thrown value ---
try { throw 5 } catch (e) { e }
try { throw 5 } catch (e) { e * 2 }
try { throw 1 + 2 } catch (e) { e }
try { throw 7 } catch (e) { e + 1 }

// --- try/finally and try/catch/finally (the status-temporary skeleton) ---
try { throw 7 } catch (e) { e } finally { }
var r = 0; try { r = 1 } catch (e) { r = 2 } finally { r = r + 10 } r
var r = 0; try { throw 3 } catch (e) { r = e } finally { r = r + 10 } r
var x = 0; try { throw 5 } catch (e) { x = e } finally { x = x + 1 } x
var s = 0; try { s = 1; throw 9 } catch (e) { s = s + e } finally { s = s + 100 } s

// --- nested try/catch ---
try { throw 1 } catch (e) { try { throw e + 1 } catch (f) { f } }

// --- a throw crossing a function-call frame, caught in the caller ---
function f(x) { if (x < 0) throw x; return x } try { f(-3) } catch (e) { -e }
function f() { throw 1 } function g() { f() } try { g() } catch (e) { e + 5 }
function g() { try { throw 2 } finally { } } try { g() } catch (e) { e }

// --- throw of a computed / heap value ---
var o = { a: 1 }; try { throw o.a } catch (e) { e + 1 }
try { var x = 1; throw x } catch (e) { e }

// --- try/catch inside a loop (backward branch + jump chain interleaved) ---
var s = 0; var i = 0; while (i < 5) { try { if (i == 2) throw i; s = s + 1 } catch (e) { s = s + 100 } i = i + 1 } s

// --- uncaught throws propagating to the host (shared abort, bit-exact) ---
throw 7
throw 1 + 2 * 3
1; throw 7
try { throw 1 } finally { }
try { throw 2 } catch (e) { throw e + 1 }
function f() { throw 42 } f()
var o = {}; o.a = 3; throw o.a
