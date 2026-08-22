# Endor Git bindings gap report

This report separates implemented binding behavior from release claims that
require target infrastructure or Minion Town work.

| Area | Status | Evidence or remaining gap |
|---|---|---|
| Safe filesystem contract | Implemented | SHA-1 and SHA-256 object, tree, ref compare-and-swap, and verification tests run through `Libgit2Repository`. |
| Custom ODB and refdb seam | Implemented for the public contract | `Libgit2Backend` installs both databases through one audited FFI module; the in-memory adapter runs the same conformance cases. |
| Callback safety | Implemented baseline | Panics are caught before the C boundary, converted to a sanitized error, and callback allocations are released by libgit2-owned free callbacks. On 2026-08-22 the six-case conformance binary passed with vendored C compiled by GCC 13 AddressSanitizer and run with its runtime preloaded; CI repeats this observation. |
| Static vendoring and local-only authority | Implemented | Exact crate versions, disabled default features, vendored libgit2, SHA-256 feature, example artifact, and linkage audit are checked in. |
| Streaming writes and pack ingestion | Not implemented | The narrow application contract is complete, but the custom ODB `writestream` and bounded `writepack` callbacks remain before Minion Town can ingest received packs directly. |
| Refdb operations outside the narrow contract | Intentionally unsupported | Rename, delete, and reflog callbacks return named `GIT_ENOTSUPPORTED` errors. Symbolic writes are rejected. |
| Endor state-directory and `ContentStore` materializer | Not implemented | This crate keeps the identity boundary, but no daemon public Git capability or Git-tree materializer is added in this tranche. |
| Linux Zig cross-build | x86_64 observed; ARM native run pending | Rust 1.95.0, `cargo-zigbuild` 0.23.0, and Zig 0.15.2 produced glibc 2.28 and static musl x86_64 artifacts on 2026-08-22. Both ran the linked-version smoke program; audits found no dynamic libgit2, OpenSSL, libssh2, libcurl, or zlib. Each ARM artifact still needs its architecture-native corpus run. |
| Windows GNU Zig lane | Attempted and blocked | On 2026-08-22 the checked-in `zig cc`/`zig ar`/`dlltool` wrappers compiled Rust dependencies, zlib, and vendored libgit2, then Zig 0.15.2 failed the final `x86_64-pc-windows-gnu` link because it could not supply the `msvcrt` import library under Rust's GNU link arguments. The non-gating CI probe preserves this exact escalation point. No artifact or MSVC claim is made. |
| macOS Zig lane | Infrastructure-gated | Cross-linking requires a legally provisioned pinned SDK via `SDKROOT`; signing and notarization remain native macOS release work. |
| Native matrix | Scripted | GitHub-hosted Linux, Windows, and macOS tests exercise the same crate; architecture coverage beyond the hosted runner remains a release gate. |
| Link audit | Implemented script | Linux, macOS, and Windows inspection rejects dynamic libgit2, OpenSSL, libssh2, libcurl, and unexpected zlib dependencies. |
| Reproducibility | Implemented baseline script | Two clean release builds compare stripped example artifacts; signing envelopes and cross-host normalization remain release work. |
| Smart HTTP corpus | Outside this crate and not implemented | Minion Town still owns authentication, protocol v2 framing, request limits, pack bounds, and transcript capture. |

The Windows, macOS SDK, ARM native-run, pack-bound, sanitizer, and smart-HTTP
rows are release blockers, not silently dropped targets.
