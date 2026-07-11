/*---
description: stage3-json corpus line 10 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-json.js line 10.
  Source: JSON.stringify("a\"b")
---*/
assert.sameValue((JSON.stringify("a\"b")), "\"a\\\"b\"");
