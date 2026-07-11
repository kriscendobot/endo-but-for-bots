/*---
description: stage3-collections corpus line 89 converted to a test262 case
flags: [raw]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-collections.js line 89.
  Source: var s=new Set(); s.add(1); s.add(2); s.add(3); var t=0; for(var x of s){t+=x;} t
---*/
var s=new Set(); s.add(1); s.add(2); s.add(3); var t=0; for(var x of s){t+=x;} t
