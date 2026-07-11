/*---
description: stage3b-object-statics corpus line 19 converted to a test262 case
flags: [raw]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3b-object-statics.js line 19.
  Source: var o = {p:1, q:2}; Object.keys(o).length === 2 && o.hasOwnProperty("p");
---*/
var o = {p:1, q:2}; Object.keys(o).length === 2 && o.hasOwnProperty("p");
