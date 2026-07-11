/*---
description: stage4-async-promises corpus line 2 converted to a test262 case
flags: [raw]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage4-async-promises.js line 2.
  Source: var x=0; var t={then:function(res){res(7)}}; Promise.resolve(t).then(function(v){x=v}); x
---*/
var x=0; var t={then:function(res){res(7)}}; Promise.resolve(t).then(function(v){x=v}); x
