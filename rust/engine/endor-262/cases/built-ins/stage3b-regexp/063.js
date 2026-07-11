/*---
description: stage3b-regexp corpus line 63 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3b-regexp.js line 63.
  Source: "abc".match(/b(c)/)[1]
---*/
assert.sameValue(("abc".match(/b(c)/)[1]), "c");
