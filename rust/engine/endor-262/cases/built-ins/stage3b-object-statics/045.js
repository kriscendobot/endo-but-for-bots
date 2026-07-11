/*---
description: stage3b-object-statics corpus line 45 converted to a test262 case
flags: [raw]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3b-object-statics.js line 45.
  Source: var o={a:1}; ("zzz" in o)===false && o.hasOwnProperty("zzz")===false;
---*/
var o={a:1}; ("zzz" in o)===false && o.hasOwnProperty("zzz")===false;
