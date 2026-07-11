/*---
description: stage3b-regexp corpus line 66 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3b-regexp.js line 66.
  Source: "abc".replace(/a/, "XY")
---*/
assert.sameValue(("abc".replace(/a/, "XY")), "XYbc");
