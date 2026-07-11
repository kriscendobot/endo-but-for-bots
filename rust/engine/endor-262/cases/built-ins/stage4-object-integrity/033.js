/*---
description: stage4-object-integrity corpus line 33 converted to a test262 case
flags: [raw]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage4-object-integrity.js line 33.
  Source: var o={a:1}; Object.freeze(o); delete o.a; o.a;
---*/
var o={a:1}; Object.freeze(o); delete o.a; o.a;
