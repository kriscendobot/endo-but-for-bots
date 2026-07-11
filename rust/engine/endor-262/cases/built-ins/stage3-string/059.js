/*---
description: stage3-string corpus line 59 converted to a test262 case
flags: [noStrict]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3-string.js line 59.
  Source: "  hi  ".trimEnd()
---*/
assert.sameValue(("  hi  ".trimEnd()), "  hi");
