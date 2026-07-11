/*---
description: stage3-json corpus line 18 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-json.js line 18.
  Source: JSON.stringify(Math.max(3, 7))
---*/
assert.sameValue((JSON.stringify(Math.max(3, 7))), "7");
