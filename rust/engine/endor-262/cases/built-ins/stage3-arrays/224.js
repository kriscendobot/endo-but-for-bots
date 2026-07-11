/*---
description: stage3-arrays corpus line 224 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-arrays.js line 224.
  Source: [1,2,3].findIndex(function(x){return x>1})
---*/
assert.sameValue(([1,2,3].findIndex(function(x){return x>1})), 1);
