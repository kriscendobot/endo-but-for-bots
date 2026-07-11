/*---
description: stage3-math corpus line 43 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-math.js line 43.
  Source: Math.min(0, -0)
---*/
assert.sameValue((Math.min(0, -0)), -0);
