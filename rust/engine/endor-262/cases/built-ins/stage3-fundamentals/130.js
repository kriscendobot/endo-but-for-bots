/*---
description: stage3-fundamentals corpus line 130 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-fundamentals.js line 130.
  Source: (new String('hi')).toString()
---*/
assert.sameValue(((new String('hi')).toString()), "hi");
