/*---
description: stage3b-object-statics corpus line 25 converted to a test262 case
flags: [raw]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3b-object-statics.js line 25.
  Source: var o={a:7}; var d=Object.getOwnPropertyDescriptor(o,"a"); d.value===7 && d.writable===true && d.enumerable===true && d.configurable===true;
---*/
var o={a:7}; var d=Object.getOwnPropertyDescriptor(o,"a"); d.value===7 && d.writable===true && d.enumerable===true && d.configurable===true;
