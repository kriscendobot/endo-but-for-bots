/*---
description: stage3-number corpus line 51 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-number.js line 51.
  Source: parseFloat("  .5e2xyz")
---*/
assert.sameValue((parseFloat("  .5e2xyz")), 50);
