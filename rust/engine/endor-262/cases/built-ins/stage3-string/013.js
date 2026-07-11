/*---
description: stage3-string corpus line 13 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-string.js line 13.
  Source: "hello".codePointAt(1)
---*/
assert.sameValue(("hello".codePointAt(1)), 101);
