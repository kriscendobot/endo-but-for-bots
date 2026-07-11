/*---
description: stage3-fundamentals corpus line 92 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-fundamentals.js line 92.
  Source: (new RangeError('r')) instanceof RangeError
---*/
assert.sameValue(((new RangeError('r')) instanceof RangeError), true);
