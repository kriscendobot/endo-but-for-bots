/*---
description: stage3b-regexp corpus line 78 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3b-regexp.js line 78.
  Source: "a,b,c".split(/,/).length
---*/
assert.sameValue(("a,b,c".split(/,/).length), 3);
