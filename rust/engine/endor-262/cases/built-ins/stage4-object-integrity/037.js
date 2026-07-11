/*---
description: stage4-object-integrity corpus line 37 converted to a test262 case
flags: [raw]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage4-object-integrity.js line 37.
  Source: var o={}; Object.defineProperty(o,"x",{value:1,writable:false,enumerable:true,configurable:false}); Object.preventExtensions(o); Object.isFrozen(o);
---*/
var o={}; Object.defineProperty(o,"x",{value:1,writable:false,enumerable:true,configurable:false}); Object.preventExtensions(o); Object.isFrozen(o);
