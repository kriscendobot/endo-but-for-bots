/*---
description: stage2b-exceptions corpus line 15 converted to a test262 case
flags: [raw]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage2b-exceptions.js line 15.
  Source: function g() { try { throw 2 } finally { } } try { g() } catch (e) { e }
---*/
function g() { try { throw 2 } finally { } } try { g() } catch (e) { e }
