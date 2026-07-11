/*---
description: stage3b-regexp corpus line 13 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3b-regexp.js line 13.
  Source: new RegExp("").source
---*/
assert.sameValue((new RegExp("").source), "(?:)");
