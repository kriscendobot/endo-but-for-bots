/*---
description: stage3-json corpus line 11 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-json.js line 11.
  Source: JSON.stringify("back\\slash")
---*/
assert.sameValue((JSON.stringify("back\\slash")), "\"back\\\\slash\"");
