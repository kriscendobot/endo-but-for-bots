/*---
description: stage3-json corpus line 1 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-json.js line 1.
  Source: JSON.stringify(42)
---*/
assert.sameValue((JSON.stringify(42)), "42");
