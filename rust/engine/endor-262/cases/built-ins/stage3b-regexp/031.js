/*---
description: stage3b-regexp corpus line 31 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3b-regexp.js line 31.
  Source: /x/.exec("abcdefghij")
---*/
assert.sameValue((/x/.exec("abcdefghij")), null);
