/*---
description: stage2b-functions corpus line 7 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage2b-functions.js line 7.
  Source: (function(x,y){return x-y})(10,3)
---*/
assert.sameValue(((function(x,y){return x-y})(10,3)), 7);
