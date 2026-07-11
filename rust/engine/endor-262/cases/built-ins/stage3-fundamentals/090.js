/*---
description: stage3-fundamentals corpus line 90 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-fundamentals.js line 90.
  Source: (new TypeError('t')) instanceof Error
---*/
assert.sameValue(((new TypeError('t')) instanceof Error), true);
