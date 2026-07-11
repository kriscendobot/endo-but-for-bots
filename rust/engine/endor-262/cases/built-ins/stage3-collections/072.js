/*---
description: stage3-collections corpus line 72 converted to a test262 case
flags: [raw]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-collections.js line 72.
  Source: var m=new Map(); m.set(1,5); var t={n:100}; var got=0; m.forEach(function(v){got=this.n;},t); got
---*/
var m=new Map(); m.set(1,5); var t={n:100}; var got=0; m.forEach(function(v){got=this.n;},t); got
