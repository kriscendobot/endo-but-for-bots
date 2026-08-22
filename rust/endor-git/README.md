# Endor Git bindings

`endor-git` is the local-storage binding described by
[`designs/endor-git-bindings.md`](../../designs/endor-git-bindings.md).
It exposes synchronous object, tree, reference compare-and-swap, and
verification operations without granting network, checkout, hook, shell, or
credential-helper authority.

Git object identifiers retain their SHA-1 or SHA-256 format in a dedicated
`GitObjectId` type.
They are intentionally unrelated to Endor `ContentStore` identifiers.

## Backends

`Libgit2Repository` serializes an ordinary bare or working repository behind a
mutex.
`Libgit2Backend` installs a caller-provided `BackendStorage` as both a custom
libgit2 ODB and refdb.
Raw pointers, callback allocation, panic containment, libgit2-owned buffers,
iterators, and shutdown are confined to `src/ffi.rs`.

The supplied `InMemoryBackend` is a conformance adapter, not durable storage.
Minion Town can implement `BackendStorage` with its partitioned CAS and SQLite
transaction without exposing either schema to this crate.

## Dependency and authority profile

The exact `git2` and `libgit2-sys` releases are pinned in `Cargo.toml` and
`Cargo.lock`.
The enabled `vendored-libgit2` and `unstable-sha256` features compile libgit2's
vendored C source through Cargo's `cc` build and link it statically.
Default features are disabled, so HTTPS, SSH, OpenSSL, libssh2, and credential
helpers do not enter the dependency graph.

## Verification

```sh
cargo test -p endor-git --all-targets
cargo clippy -p endor-git --all-targets -- -D warnings
cargo build --release -p endor-git --example endor-git-link-audit
rust/endor-git/scripts/link-audit.sh \
  target/release/examples/endor-git-link-audit
rust/endor-git/scripts/sanitizer-check.sh
```

The conformance suite runs against filesystem and custom backends in both
object formats.
It covers known object IDs, loose object and tree round trips, callback panic
conversion, reference iteration, namespace restriction, and a two-writer
compare-and-swap race.

`GitBlockingPool` supplies the shared bounded sync-to-async bridge.
The pointer ownership and callback rules are recorded in [SAFETY.md](SAFETY.md).

## Cross-builds

The release wrapper records the Rust, Cargo, Zig, target, CPU, and optional
macOS SDK inputs before building.

```sh
rust/endor-git/scripts/cross-build.sh x86_64-unknown-linux-gnu.2.28
rust/endor-git/scripts/cross-build.sh x86_64-unknown-linux-musl
rust/endor-git/scripts/cross-build.sh x86_64-pc-windows-gnu
```

Linux uses `cargo zigbuild`.
Windows GNU uses checked-in `zig cc` and `zig ar` wrappers because
`cargo-zigbuild` does not claim that target.
Darwin uses `cargo zigbuild` only when `SDKROOT` names an explicitly
provisioned SDK.
Every cross-built artifact still requires the native execution gate described
in the design.

See [GAPS.md](GAPS.md) for the structured delivery and release-gate status.
