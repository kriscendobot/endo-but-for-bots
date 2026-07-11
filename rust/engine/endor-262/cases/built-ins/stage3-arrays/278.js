/*---
description: stage3-arrays corpus line 278 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-arrays.js line 278.
  Source: ["a","b","c"].toString()
---*/
assert.sameValue((["a","b","c"].toString()), "a,b,c");
