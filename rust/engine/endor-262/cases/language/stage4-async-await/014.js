/*---
description: stage4-async-await corpus line 14 converted to a test262 case
flags: [raw]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage4-async-await.js line 14.
  Source: var log=""; async function f(){ log += "z"; return; } f(); log
---*/
var log=""; async function f(){ log += "z"; return; } f(); log
