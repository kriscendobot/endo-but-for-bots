/*---
description: stage2b-functions corpus line 24 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage2b-functions.js line 24.
  Source: (function(x){return (function(y){return y+1})(x)+x})(4)
---*/
assert.sameValue(((function(x){return (function(y){return y+1})(x)+x})(4)), 9);
