/*---
description: stage4-async-promises corpus line 16 converted to a test262 case
flags: [raw]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage4-async-promises.js line 16.
  Source: var x=0; Promise.resolve(1).then(function(v){return {then:function(r){r(v+40)}}}).then(function(v){x=v}); x
---*/
var x=0; Promise.resolve(1).then(function(v){return {then:function(r){r(v+40)}}}).then(function(v){x=v}); x
