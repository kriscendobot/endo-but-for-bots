/*---
description: stage3-number corpus line 12 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-number.js line 12.
  Source: Number.isInteger(NaN)
---*/
assert.sameValue((Number.isInteger(NaN)), false);
