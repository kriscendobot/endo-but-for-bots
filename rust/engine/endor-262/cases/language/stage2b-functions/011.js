/*---
description: stage2b-functions corpus line 11 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage2b-functions.js line 11.
  Source: (function(){var a=1,b=2; return a+b})()
---*/
assert.sameValue(((function(){var a=1,b=2; return a+b})()), 3);
