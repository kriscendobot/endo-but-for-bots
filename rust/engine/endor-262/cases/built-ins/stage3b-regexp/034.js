/*---
description: stage3b-regexp corpus line 34 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3b-regexp.js line 34.
  Source: /b(c)/.exec("abc").index
---*/
assert.sameValue((/b(c)/.exec("abc").index), 1);
