/*---
description: stage2b-exceptions corpus line 12 converted to a test262 case
flags: [raw]
features: [endor-dual-run, endor-meter-exact, endor-meter-determinism]
info: |
  Converted from corpora/stage2b-exceptions.js line 12.
  Source: try { throw 1 } catch (e) { try { throw e + 1 } catch (f) { f } }
---*/
try { throw 1 } catch (e) { try { throw e + 1 } catch (f) { f } }
