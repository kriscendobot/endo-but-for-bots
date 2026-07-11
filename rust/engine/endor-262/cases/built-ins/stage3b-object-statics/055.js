/*---
description: stage3b-object-statics corpus line 55 converted to a test262 case
flags: [raw]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3b-object-statics.js line 55.
  Source: var o={}; Object.defineProperty(o,"x",{value:7,writable:true,enumerable:false,configurable:true}); var d=Object.getOwnPropertyDescriptor(o,"x"); d.enumerable;
---*/
var o={}; Object.defineProperty(o,"x",{value:7,writable:true,enumerable:false,configurable:true}); var d=Object.getOwnPropertyDescriptor(o,"x"); d.enumerable;
