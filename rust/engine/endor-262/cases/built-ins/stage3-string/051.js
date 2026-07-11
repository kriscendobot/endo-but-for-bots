/*---
description: stage3-string corpus line 51 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-string.js line 51.
  Source: "hello".endsWith("hel", 3)
---*/
assert.sameValue(("hello".endsWith("hel", 3)), true);
