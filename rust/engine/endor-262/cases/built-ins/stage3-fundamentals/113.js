/*---
description: stage3-fundamentals corpus line 113 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-fundamentals.js line 113.
  Source: typeof new Boolean(1)
---*/
assert.sameValue((typeof new Boolean(1)), "object");
