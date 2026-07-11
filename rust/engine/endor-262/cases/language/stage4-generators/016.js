/*---
description: stage4-generators corpus line 16 converted to a test262 case
flags: [raw]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage4-generators.js line 16.
  Source: function* g(){ yield 1; yield 2; } var a=g(); a.return(99).value;
---*/
function* g(){ yield 1; yield 2; } var a=g(); a.return(99).value;
