/*---
description: stage3-arrays corpus line 216 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-arrays.js line 216.
  Source: [].map(function(x){return x}).join()
---*/
assert.sameValue(([].map(function(x){return x}).join()), "");
