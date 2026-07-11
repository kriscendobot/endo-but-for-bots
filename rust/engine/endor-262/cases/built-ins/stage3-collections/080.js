/*---
description: stage3-collections corpus line 80 converted to a test262 case
flags: [raw]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-collections.js line 80.
  Source: var m=new Map(); m.set(1,2); var it=m.entries(); it.next().value[1]
---*/
var m=new Map(); m.set(1,2); var it=m.entries(); it.next().value[1]
