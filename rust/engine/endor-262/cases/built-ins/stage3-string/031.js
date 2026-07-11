/*---
description: stage3-string corpus line 31 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-string.js line 31.
  Source: "abcdef".substring(0, 100)
---*/
assert.sameValue(("abcdef".substring(0, 100)), "abcdef");
