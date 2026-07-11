/*---
description: stage3-fundamentals corpus line 89 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-fundamentals.js line 89.
  Source: (new TypeError('t')) instanceof TypeError
---*/
assert.sameValue(((new TypeError('t')) instanceof TypeError), true);
