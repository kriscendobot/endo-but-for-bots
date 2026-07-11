/*---
description: stage3-string corpus line 25 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-string.js line 25.
  Source: "abcdef".slice(-4, -1)
---*/
assert.sameValue(("abcdef".slice(-4, -1)), "cde");
