/*---
description: stage3-string corpus line 28 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-string.js line 28.
  Source: "abcdef".substring(2, 4)
---*/
assert.sameValue(("abcdef".substring(2, 4)), "cd");
