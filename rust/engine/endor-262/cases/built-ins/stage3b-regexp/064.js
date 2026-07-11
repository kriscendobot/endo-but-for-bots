/*---
description: stage3b-regexp corpus line 64 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3b-regexp.js line 64.
  Source: "abc".replace(/b/, "X")
---*/
assert.sameValue(("abc".replace(/b/, "X")), "aXc");
