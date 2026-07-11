/*---
description: stage3-string corpus line 19 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-string.js line 19.
  Source: "hello".at(-1)
---*/
assert.sameValue(("hello".at(-1)), "o");
