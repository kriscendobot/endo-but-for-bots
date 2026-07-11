/*---
description: stage3b-regexp corpus line 70 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3b-regexp.js line 70.
  Source: "x".replace(/(x)/, "y")
---*/
assert.sameValue(("x".replace(/(x)/, "y")), "y");
