/*---
description: stage3b-fundamentals-followup corpus line 48 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3b-fundamentals-followup.js line 48.
  Source: new AggregateError([1,2]) instanceof AggregateError
---*/
assert.sameValue((new AggregateError([1,2]) instanceof AggregateError), true);
