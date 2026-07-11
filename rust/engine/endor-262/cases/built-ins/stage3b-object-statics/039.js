/*---
description: stage3b-object-statics corpus line 39 converted to a test262 case
flags: [raw]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3b-object-statics.js line 39.
  Source: var o={a:1}; var k="zzz"; o[k]===undefined && o.hasOwnProperty(k)===false;
---*/
var o={a:1}; var k="zzz"; o[k]===undefined && o.hasOwnProperty(k)===false;
