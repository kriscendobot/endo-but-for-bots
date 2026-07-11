/*---
description: stage3-string corpus line 58 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-string.js line 58.
  Source: "  hi  ".trimStart()
---*/
assert.sameValue(("  hi  ".trimStart()), "hi  ");
