/*---
description: stage4-generators corpus line 10 converted to a test262 case
flags: [raw]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage4-generators.js line 10.
  Source: function* g(){ yield 1; yield 2; yield 3; } var s=0; for (var v of g()) s += v; s;
---*/
function* g(){ yield 1; yield 2; yield 3; } var s=0; for (var v of g()) s += v; s;
