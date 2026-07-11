/*---
description: stage3-fundamentals corpus line 78 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-fundamentals.js line 78.
  Source: (new Error('x')).message
---*/
assert.sameValue(((new Error('x')).message), "x");
