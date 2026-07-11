/*---
description: stage3b-promises corpus line 25 converted to a test262 case
flags: [raw]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3b-promises.js line 25.
  Source: var x = 0; Promise.resolve(1).catch(function(e){ x = e; }).then(function(v){ x = v; }); x
---*/
var x = 0; Promise.resolve(1).catch(function(e){ x = e; }).then(function(v){ x = v; }); x
