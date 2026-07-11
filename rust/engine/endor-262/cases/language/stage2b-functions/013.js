/*---
description: stage2b-functions corpus line 13 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage2b-functions.js line 13.
  Source: (function(x){var a=1; return x+a})(5)
---*/
assert.sameValue(((function(x){var a=1; return x+a})(5)), 6);
