/*---
description: stage2b-functions corpus line 23 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage2b-functions.js line 23.
  Source: (function(){return (function(){return 1})()})()
---*/
assert.sameValue(((function(){return (function(){return 1})()})()), 1);
