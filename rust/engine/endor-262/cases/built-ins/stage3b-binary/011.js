/*---
description: stage3b-binary corpus line 11 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3b-binary.js line 11.
  Source: new ArrayBuffer().byteLength
---*/
assert.sameValue((new ArrayBuffer().byteLength), 0);
