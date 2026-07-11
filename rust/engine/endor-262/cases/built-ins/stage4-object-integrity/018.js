/*---
description: stage4-object-integrity corpus line 18 converted to a test262 case
flags: [raw]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage4-object-integrity.js line 18.
  Source: var o={a:1}; Object.seal(o); o.a=7; o.a;
---*/
var o={a:1}; Object.seal(o); o.a=7; o.a;
