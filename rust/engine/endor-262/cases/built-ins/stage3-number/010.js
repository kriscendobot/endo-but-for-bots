/*---
description: stage3-number corpus line 10 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-number.js line 10.
  Source: Number.isInteger(5.5)
---*/
assert.sameValue((Number.isInteger(5.5)), false);
