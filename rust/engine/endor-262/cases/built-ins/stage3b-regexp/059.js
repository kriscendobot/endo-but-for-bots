/*---
description: stage3b-regexp corpus line 59 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3b-regexp.js line 59.
  Source: "abc".match(/z/)
---*/
assert.sameValue(("abc".match(/z/)), null);
