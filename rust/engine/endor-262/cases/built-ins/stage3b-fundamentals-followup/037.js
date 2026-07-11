/*---
description: stage3b-fundamentals-followup corpus line 37 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3b-fundamentals-followup.js line 37.
  Source: typeof Symbol.keyFor(Symbol("local"))
---*/
assert.sameValue((typeof Symbol.keyFor(Symbol("local"))), "undefined");
