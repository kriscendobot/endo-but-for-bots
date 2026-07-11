/*---
description: stage3-collections corpus line 74 converted to a test262 case
flags: [raw]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-collections.js line 74.
  Source: var s=new Set(); s.add(1); s.add(2); s.add(3); var r=0; s.forEach(function(v){r+=v;}); r
---*/
var s=new Set(); s.add(1); s.add(2); s.add(3); var r=0; s.forEach(function(v){r+=v;}); r
