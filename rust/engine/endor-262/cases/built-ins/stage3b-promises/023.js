/*---
description: stage3b-promises corpus line 23 converted to a test262 case
flags: [raw]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3b-promises.js line 23.
  Source: var x = 0; Promise.reject(2).then(function(v){ x = 1; }).then(undefined, function(e){ x = e; }); x
---*/
var x = 0; Promise.reject(2).then(function(v){ x = 1; }).then(undefined, function(e){ x = e; }); x
