/*---
description: stage3b-object-statics corpus line 56 converted to a test262 case
flags: [raw]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3b-object-statics.js line 56.
  Source: var o={}; Object.defineProperty(o,"x",{value:7,writable:true,enumerable:true,configurable:false}); var d=Object.getOwnPropertyDescriptor(o,"x"); d.configurable;
---*/
var o={}; Object.defineProperty(o,"x",{value:7,writable:true,enumerable:true,configurable:false}); var d=Object.getOwnPropertyDescriptor(o,"x"); d.configurable;
