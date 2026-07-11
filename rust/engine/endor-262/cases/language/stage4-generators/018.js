/*---
description: stage4-generators corpus line 18 converted to a test262 case
flags: [raw]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage4-generators.js line 18.
  Source: var o = { *gen(){ yield 1; yield 2; } }; var a=o.gen(); a.next().value + a.next().value;
---*/
var o = { *gen(){ yield 1; yield 2; } }; var a=o.gen(); a.next().value + a.next().value;
