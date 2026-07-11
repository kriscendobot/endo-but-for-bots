/*---
description: stage4-new-target corpus line 15 converted to a test262 case
flags: [raw]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage4-new-target.js line 15.
  Source: var t; function F(){ t = new.target; } new F(); F(); t === undefined;
---*/
var t; function F(){ t = new.target; } new F(); F(); t === undefined;
