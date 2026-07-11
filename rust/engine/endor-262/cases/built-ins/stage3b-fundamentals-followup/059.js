/*---
description: stage3b-fundamentals-followup corpus line 59 converted to a test262 case
flags: [raw]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage3b-fundamentals-followup.js line 59.
  Source: function fthis(){return this.v} var ov={v:42}; var gt=fthis.bind(ov); gt()
---*/
function fthis(){return this.v} var ov={v:42}; var gt=fthis.bind(ov); gt()
