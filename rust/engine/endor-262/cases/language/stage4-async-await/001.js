/*---
description: stage4-async-await corpus line 1 converted to a test262 case
flags: [raw]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage4-async-await.js line 1.
  Source: var x=0; async function f(){ x=1; return 42; } f(); x
---*/
var x=0; async function f(){ x=1; return 42; } f(); x
