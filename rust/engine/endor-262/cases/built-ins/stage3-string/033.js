/*---
description: stage3-string corpus line 33 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-string.js line 33.
  Source: "ab".concat("cd", "ef")
---*/
assert.sameValue(("ab".concat("cd", "ef")), "abcdef");
