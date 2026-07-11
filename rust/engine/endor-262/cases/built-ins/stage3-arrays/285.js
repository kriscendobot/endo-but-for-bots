/*---
description: stage3-arrays corpus line 285 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-arrays.js line 285.
  Source: [..."abc"].join("-")
---*/
assert.sameValue(([..."abc"].join("-")), "a-b-c");
