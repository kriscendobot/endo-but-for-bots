/*---
description: arithmetic corpus line 29 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/arithmetic.js line 29.
  Source: ((2 + 3) * (4 - 1)) % 7
---*/
assert.sameValue((((2 + 3) * (4 - 1)) % 7), 1);
