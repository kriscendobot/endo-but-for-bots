/*---
description: stage4-new-target corpus line 12 converted to a test262 case
flags: [raw]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage4-new-target.js line 12.
  Source: function F(){ return new.target === F; } new F();
---*/
function F(){ return new.target === F; } new F();
