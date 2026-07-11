/*---
description: stage3-fundamentals corpus line 88 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-fundamentals.js line 88.
  Source: (new Error('x')) instanceof Error
---*/
assert.sameValue(((new Error('x')) instanceof Error), true);
