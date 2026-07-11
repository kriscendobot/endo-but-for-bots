/*---
description: stage3b-regexp corpus line 57 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3b-regexp.js line 57.
  Source: "abc123".search(/[0-9]/)
---*/
assert.sameValue(("abc123".search(/[0-9]/)), 3);
