// Stage-4b lockdown/harden corpus (child 4/5): the Hardened-JavaScript
// `harden(x)`/`petrify(x)` globals ported from `xsLockdown.c`. One program per
// line; the completion value is the last expression. Asserted RESULT-exact
// against the C-XS pin (the oracle shim installs the harden/lockdown/petrify/
// mutabilities globals xst.c/xstFuzz.c install). `xsLockdown.c` calls no
// `mxMeter`, so the metering is allocation-driven; computron parity over a
// transitive `harden` walk is structurally unavailable because endor models
// intrinsics sparsely (only program-referenced names) — the freeze *result* is
// faithful, the transitive object count is not. Result-gated, every program
// completing on both engines to the same value.

// harden freezes the target: a data property becomes non-writable, so a sloppy
// reassignment is silently ignored (the value is unchanged).
var o={a:1}; harden(o); o.a=2; o.a
// harden makes the object frozen (Object.isFrozen agrees).
var o={a:1}; harden(o); Object.isFrozen(o)
// ...sealed and non-extensible too.
var o={a:1}; harden(o); Object.isSealed(o)
var o={a:1}; harden(o); Object.isExtensible(o)
// harden returns its argument (so `var r = harden(x)` aliases x).
var o={a:1}; harden(o)===o
var o={a:1}; var r=harden(o); r.a
// A non-extensible hardened object rejects a new key (sloppy silent no-op).
var o={a:1}; harden(o); o.z=9; o.z
// A non-configurable hardened property refuses deletion (sloppy `delete` → the
// property stays).
var o={a:1}; harden(o); delete o.a; o.a
// harden is idempotent: a second harden is a no-op, the object stays frozen.
var o={a:1}; harden(o); harden(o); o.a=5; o.a
// A non-reference argument passes through unchanged.
harden(3)
harden("s")
harden(true)
// An empty object hardens (frozen, no own keys).
var o={}; harden(o); Object.isFrozen(o)
// A multi-key object: every own data property is frozen.
var o={a:1,b:2}; harden(o); Object.isFrozen(o)
var o={a:1,b:2,c:3}; harden(o); o.a=9; o.b=9; o.c=9; o.a+o.b+o.c
// harden is transitive: a nested object reachable from the target is frozen too.
var o={a:1,b:{c:2}}; harden(o); Object.isFrozen(o.b)
var o={a:1,b:{c:2}}; harden(o); o.b.c=9; o.b.c
// ...to arbitrary depth.
var o={a:{b:{c:3}}}; harden(o); Object.isFrozen(o.a.b)
var o={a:{b:{c:3}}}; harden(o); o.a.b.c=9; o.a.b.c
// A shared referent reached by two paths is hardened once (the visited set).
var s={x:1}; var o={p:s,q:s}; harden(o); Object.isFrozen(o.p)
// petrify freezes a single object (frozen, own writes rejected).
var o={a:1}; petrify(o); o.a=9; o.a
var o={a:1}; petrify(o); Object.isFrozen(o)
var o={}; petrify(o); Object.isFrozen(o)
var o={a:1,b:2}; petrify(o); o.a=8; o.b=8; o.a+o.b
// petrify is NON-transitive: the target's own `b` property is frozen (can't be
// reassigned) but the object it references is not, so a deep write still lands.
var o={a:1,b:{c:2}}; petrify(o); o.b.c=9; o.b.c
// petrify returns its argument; a non-reference passes through.
var o={a:1}; petrify(o)===o
petrify(5)
// typeof of the installed globals.
typeof harden
typeof petrify
