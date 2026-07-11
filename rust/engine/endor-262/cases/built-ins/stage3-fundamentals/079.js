/*---
description: stage3-fundamentals corpus line 79 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-fundamentals.js line 79.
  Source: (new Error('x')).name
---*/
assert.sameValue(((new Error('x')).name), "Error");
