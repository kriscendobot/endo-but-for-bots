/*---
description: control-flow corpus line 9 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/control-flow.js line 9.
  Source: true ? false ? 1 : 2 : 3
---*/
assert.sameValue((true ? false ? 1 : 2 : 3), 2);
