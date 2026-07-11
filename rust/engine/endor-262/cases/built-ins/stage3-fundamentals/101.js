/*---
description: stage3-fundamentals corpus line 101 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-fundamentals.js line 101.
  Source: 'a' in {a:1, b:2}
---*/
assert.sameValue(('a' in {a:1, b:2}), true);
