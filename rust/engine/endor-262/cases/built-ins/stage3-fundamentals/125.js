/*---
description: stage3-fundamentals corpus line 125 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-fundamentals.js line 125.
  Source: (new Error('m')).toString()
---*/
assert.sameValue(((new Error('m')).toString()), "Error: m");
