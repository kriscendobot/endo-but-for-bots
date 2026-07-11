/*---
description: stage3-string corpus line 30 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-string.js line 30.
  Source: "abcdef".substring(3)
---*/
assert.sameValue(("abcdef".substring(3)), "def");
