/*---
description: stage4-async-promises corpus line 9 converted to a test262 case
flags: [raw]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage4-async-promises.js line 9.
  Source: var x=0; Promise.resolve({then:function(res,rej){res(7);res(8)}}).then(function(v){x=v}); x
---*/
var x=0; Promise.resolve({then:function(res,rej){res(7);res(8)}}).then(function(v){x=v}); x
