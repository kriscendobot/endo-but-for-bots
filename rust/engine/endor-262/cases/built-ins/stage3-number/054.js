/*---
description: stage3-number corpus line 54 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-number.js line 54.
  Source: parseFloat("abc")
---*/
assert.sameValue((parseFloat("abc")), NaN);
