/*---
description: stage3b-fundamentals-followup corpus line 42 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3b-fundamentals-followup.js line 42.
  Source: new AggregateError([]).name
---*/
assert.sameValue((new AggregateError([]).name), "AggregateError");
