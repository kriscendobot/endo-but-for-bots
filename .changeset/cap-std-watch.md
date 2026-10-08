---
'@endo/daemon': minor
---

Add capability-scoped directory watches to the Rust/XS supervisor, with a portable snapshot-diff backend and kqueue wakeups on BSD-family platforms.

Upgrade considerations: the XS snapshot signature changes from `endo-xs 1` to `endo-xs 2` because the host callback table gained the watch callbacks. XS worker snapshots written by an earlier supervisor are rejected on restore, so suspended workers start fresh after an upgrade. The XS worker's message pump now also reads inbound envelopes when promise jobs fail to settle within a 100 ms slice, so a long-running microtask loop no longer blocks message delivery.
