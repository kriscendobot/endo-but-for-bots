/*---
description: control-flow corpus line 1 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/control-flow.js line 1.
  Source: 1 < 2 ? 10 : 20
---*/
assert.sameValue((1 < 2 ? 10 : 20), 10);
