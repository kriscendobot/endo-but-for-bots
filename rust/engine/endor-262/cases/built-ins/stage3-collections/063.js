/*---
description: stage3-collections corpus line 63 converted to a test262 case
flags: [raw]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-collections.js line 63.
  Source: var o={}; var s=new WeakSet(); s.add(o); s.add(o); s.has(o)
---*/
var o={}; var s=new WeakSet(); s.add(o); s.add(o); s.has(o)
