/*---
description: stage3b-regexp corpus line 55 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3b-regexp.js line 55.
  Source: "hello world".search(/o/)
---*/
assert.sameValue(("hello world".search(/o/)), 4);
