/*---
description: stage2b-functions corpus line 15 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage2b-functions.js line 15.
  Source: (function(x){return x})(5,6,7)
---*/
assert.sameValue(((function(x){return x})(5,6,7)), 5);
