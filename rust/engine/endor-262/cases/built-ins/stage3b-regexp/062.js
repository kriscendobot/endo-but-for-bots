/*---
description: stage3b-regexp corpus line 62 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3b-regexp.js line 62.
  Source: "abc".match(/a/).index
---*/
assert.sameValue(("abc".match(/a/).index), 0);
