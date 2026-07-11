/*---
description: stage3-fundamentals corpus line 152 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-fundamentals.js line 152.
  Source: Symbol() ? 1 : 2
---*/
assert.sameValue((Symbol() ? 1 : 2), 1);
