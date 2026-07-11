/*---
description: stage3b-promises corpus line 22 converted to a test262 case
flags: [raw]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3b-promises.js line 22.
  Source: var x = 0; Promise.reject(7).then(undefined, function(e){ x = e; }); x
---*/
var x = 0; Promise.reject(7).then(undefined, function(e){ x = e; }); x
