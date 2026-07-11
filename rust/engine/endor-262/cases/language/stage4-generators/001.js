/*---
description: stage4-generators corpus line 1 converted to a test262 case
flags: [raw]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage4-generators.js line 1.
  Source: function* g(){ yield 1; yield 2; } var a=g(); a.next().value;
---*/
function* g(){ yield 1; yield 2; } var a=g(); a.next().value;
