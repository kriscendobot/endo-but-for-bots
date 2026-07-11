/*---
description: stage3b-promises corpus line 19 converted to a test262 case
flags: [raw]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3b-promises.js line 19.
  Source: var x = 0; Promise.resolve(1).then(function(v){ x = v; return v * 2; }).then(function(w){ x = x + w; }); x
---*/
var x = 0; Promise.resolve(1).then(function(v){ x = v; return v * 2; }).then(function(w){ x = x + w; }); x
