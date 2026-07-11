/*---
description: stage3b-fundamentals-followup corpus line 36 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3b-fundamentals-followup.js line 36.
  Source: Symbol.keyFor(Symbol.for("registered"))
---*/
assert.sameValue((Symbol.keyFor(Symbol.for("registered"))), "registered");
