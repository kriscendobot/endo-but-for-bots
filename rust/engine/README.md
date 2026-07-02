# Endor engine (rust/engine)

The oracle-locked transliteration of XS to Rust described in
[`designs/xs2rust-endor-engine.md`](../../designs/xs2rust-endor-engine.md).
An independent Cargo workspace (excluded from the repo-root workspace)
so it builds in-repo from the first commit (resolved question 9).

## Crates

| Crate | `unsafe` | Purpose |
|---|---|---|
| `endor-vm` | `#![forbid(unsafe_code)]` | Index-arena value/heap model, `Vec`-backed slot stack, `match`-dispatch interpreter over the stage-1 opcode subset, and the 16.16 fixed-point meter. |
| `endor-oracle` | audited FFI (the one exception) | Compiles JS to XS bytecode and runs it on C-XS, returning `(bytecode, result, run-only computrons)`. Dev/CI only; never linked into a shipped engine. |
| `endor-262` | `#![forbid(unsafe_code)]` | Dual-run harness: runs the same bytecode on `endor-vm` and the oracle, recording four-valued + computron agreement. |
| `endor-fuzz` | `#![forbid(unsafe_code)]` | cargo-fuzz targets 1 (differential source) and 2 (bytecode decoder), authored so the logic is a plain testable lib. |

## Building the oracle: the `c/moddable` pin

`endor-oracle` compiles the C-XS engine from the `c/moddable`
submodule. The design's Ground Truth names the pin
**`48ee02d8cfe0`** ("the pin lineage of the `c/moddable` submodule that
`rust/endo/xsnap/build.rs` compiles today"). Populate it with:

```sh
git -C c/moddable fetch --depth 1 --filter=blob:none \
    origin 48ee02d8cfe0dccb51ee2465cf6716b3468684a4
git -C c/moddable checkout 48ee02d8cfe0dccb51ee2465cf6716b3468684a4
```

The shallow sha-fetch above only works while the pin is an advertised
tip; GitHub now rejects it (`upload-pack: not our ref` — the `public`
branch has moved past the pin). Two working fallbacks (verified
2026-07-02):

```sh
# (a) full (non-shallow) fetch of public; the pin is an ancestor of it
git -C c/moddable fetch https://github.com/Moddable-OpenSource/moddable public
git -C c/moddable checkout 48ee02d8cfe0dccb51ee2465cf6716b3468684a4

# (b) fetch from any sibling checkout that already holds the pin
git -C c/moddable fetch /path/to/sibling/c/moddable 48ee02d8cfe0dccb51ee2465cf6716b3468684a4
git -C c/moddable checkout 48ee02d8cfe0dccb51ee2465cf6716b3468684a4
```

Caution: `c/moddable` in a fresh checkout is an **empty gitlink
directory with no `.git`**, so a `git -C c/moddable ...` there walks up
and operates on the *superproject*. `git clone` a moddable repo into
`c/moddable` (or `git init` it) before running any of the fetches
above.

Two frictions the supervisor should note (they do not affect the
stage-1 result, which is bit-exact against this pin, but they affect
reproducibility and the "path dependency on xsnap" phrasing):

1. The submodule **gitlink** recorded in the superproject is
   `5516726818906190d3a042d8be90219ce9d51b45`, which is **not fetchable
   from upstream** (`upload-pack: not our ref`). The design's stated
   pin `48ee02d8cfe0` *is* the current `public` HEAD and is what the
   oracle builds against. Correcting the gitlink is a submodule bump
   this stage deliberately does **not** make on its own.
2. There is an **API drift** between the two shas: at `48ee02d8cfe0`,
   `fxInitializeSharedCluster` takes a `txMachine*` argument (the
   oracle shim passes `C_NULL`, as `xst262.c` does), whereas the
   existing `xsnap` crate's `ffi.rs` declares it argument-free. Because
   of this drift, and because `xsnap`'s `lib.rs` `include_str!`s
   generated SES bundles that are gitignored and absent from a fresh
   checkout, `endor-oracle` links the C-XS sources **directly** (reusing
   xsnap's audited `xsnap-platform.{c,h}` and the identical feature
   defines) rather than as a Cargo path dependency on `xsnap`.

## Running the harness

```sh
cd rust/engine
cargo run -p endor-262 --bin harness          # stage-1 corpus
cargo run -p endor-262 --bin harness -- '1 + 2 * 3'   # ad-hoc program
cargo test  --workspace -- --test-threads=1   # includes the bar as a test
```
