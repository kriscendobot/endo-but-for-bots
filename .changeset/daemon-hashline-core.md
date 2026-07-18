---
'@endo/daemon': minor
---

Add `hashline.js`, the pure shared core of the hash-anchored line-based
edit format of `designs/cli-edit-verb.md`: CRC32 per-line anchors
(2-char hex, 4-char above 4096 lines, blank lines seeded with their
line number), the SHA-256 whole-file compare-and-swap, the textual
`hashline` patch parser and the `hashline-json` envelope validator,
the hashline-annotated read rendering, the deterministic bottom-up
splice with same-line composition, and the opt-in bounded `reapply`
anchor-relocation search. `applyEditPatch` returns the design's
structured `EditResult` (`file-rev-mismatch`, `hash-mismatch` with
both-width reports, `ambiguous-reapply`, `patch-syntax`) instead of
throwing, and takes an injected `sha256Hex` power so the module stays
pure. The daemon-side `EndoMount.edit` / `EndoGuest.edit` wiring and
the CLI verb build on this in follow-up work.
