/*---
description: logic corpus line 40 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/logic.js line 40.
  Source: 1 > 2 || 3 > 4
---*/
assert.sameValue((1 > 2 || 3 > 4), false);
