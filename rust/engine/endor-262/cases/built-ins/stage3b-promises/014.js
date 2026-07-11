/*---
description: stage3b-promises corpus line 14 converted to a test262 case
flags: [raw]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3b-promises.js line 14.
  Source: var x = 0; Promise.resolve(1).then(function(v){ x = v; }); x
---*/
var x = 0; Promise.resolve(1).then(function(v){ x = v; }); x
