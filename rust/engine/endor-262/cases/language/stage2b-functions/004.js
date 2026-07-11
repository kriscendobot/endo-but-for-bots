/*---
description: stage2b-functions corpus line 4 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage2b-functions.js line 4.
  Source: (function(x){return x+1})(5)
---*/
assert.sameValue(((function(x){return x+1})(5)), 6);
