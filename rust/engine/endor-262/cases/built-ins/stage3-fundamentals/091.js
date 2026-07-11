/*---
description: stage3-fundamentals corpus line 91 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-fundamentals.js line 91.
  Source: (new Error()) instanceof TypeError
---*/
assert.sameValue(((new Error()) instanceof TypeError), false);
