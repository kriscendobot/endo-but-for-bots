/*---
description: stage3-fundamentals corpus line 77 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-fundamentals.js line 77.
  Source: typeof new Error('x')
---*/
assert.sameValue((typeof new Error('x')), "object");
