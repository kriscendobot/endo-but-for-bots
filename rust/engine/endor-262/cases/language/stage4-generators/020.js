/*---
description: stage4-generators corpus line 20 converted to a test262 case
flags: [raw]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage4-generators.js line 20.
  Source: function* g(){ yield "a"; yield "b"; } var a=g(); typeof a.next;
---*/
function* g(){ yield "a"; yield "b"; } var a=g(); typeof a.next;
