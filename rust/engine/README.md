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

# The real test262 language/ dual-run runner (stage-2 acceptance bar).
# Run per subtree — the C-XS oracle accumulates memory across a whole-tree
# walk, so `expressions`/`statements` in separate processes bound the RSS.
cargo run -p endor-262 --bin test262-language -- expressions
cargo run -p endor-262 --bin test262-language -- statements/for
# The stage-3 built-ins sections run through the same binary:
cargo run -p endor-262 --bin test262-language -- built-ins/Boolean
```

The stage-scoped curated corpora under `endor-262/corpora/` are the
bootstrap (stage-1 arithmetic/logic/control-flow; stage-2 var/loop/object;
stage-2b functions/closures/exceptions; stage-3 language string values +
numeric/chaining opcodes, and fundamentals — the intrinsic constructors as
first-class values, `Boolean`/`Object` native calls, the value globals
`undefined`/`NaN`/`Infinity`, and `new` constructor calls; stage-3 arrays —
the Array exotic object: literals with holes, computed index get/set over the
item chunk, the `length` accessor get/set, the dense `Array.prototype` methods
— the non-callback `push`/`pop`/`shift`/`unshift`/`indexOf`/`lastIndexOf`/
`includes`/`fill`/`slice`/`join`/`toString`/`at`/`reverse`/`concat`/
`copyWithin`/`with`/`splice`/`toSpliced`/`toReversed`/`flat` and the re-entrant
callback methods `forEach`/`map`/`filter`/`some`/`every`/`find`/`findIndex`/
`findLast`/`findLastIndex`/`reduce`/`reduceRight`/`flatMap` (driven through a
re-entrant `run_callback` substrate) — the `Array(...)` constructor +
`Array.isArray`, the `values`/`keys`/`entries` array iterators, the string
iterator (`for-of` / spread over a string, yielding each BMP code point — astral
content self-names an honest skip), and the iteration protocol — `for-of`,
`for-in`, and array/string spread `[...arr]` / `[...str]`),
all bit-exact
(result AND computron) against the oracle. Methods whose metering is
data-dependent or routes through un-modeled machinery (`sort`/`toSorted` —
comparator-driven; `toLocaleString`; the `Array.from`/`fromAsync`/`of` statics)
are bound as **honest named skips** (`Halt::Unsupported`) rather than left to
resolve to `undefined` and throw — so a reference is a NAMED skip, never a
completion divergence or a wrong value. The stage's built-ins/Array dual-run
reflects this: `total=2625 covered=403 divergent=0 skipped=2222` (every skip
named), and the iteration protocol grows `statements/for-in` to `covered=19`
and `statements/for-of` to `covered=79`, both `divergent=0`. The stage-3
**text-math-json** child adds `String.prototype` over the CESU-8 chunk (a
primitive string boxes to `%String.prototype%`: `.length` is the UTF-16
code-unit count, `str[i]` the one-unit character; `charCodeAt`/`codePointAt`/
`charAt`/`at`/`slice`/`substring`/`concat`/`repeat`/`toLowerCase`/`toUpperCase`/
`trim`/`trimStart`/`trimEnd`/`startsWith`/`endsWith`/`includes`), the `Number`
statics/predicates/`toString`(radix 10)/`Number(...)` coercion, the numeric
globals `parseInt`/`parseFloat`/`isNaN`/`isFinite`, the whole `Math` namespace
(every function, **canonical `f64::NAN`** for the consensus-critical
determinism, the pin's exact libm choices, and the `±0`/integer-fold corners),
and `JSON.stringify` over a top-level primitive. Metering is calibrated
raw-exact against the pin: `Math`/`Number`/`parseInt`/`parseFloat` carry a zero
native residual over the `RUN` opcode (their `xs*.c` bodies charge no
`mxMeterSome`); a chunk-only String method (`slice`/`charAt`/…) carries zero,
while an `mxMeterSome`-calling String method (`concat`/`repeat`/`toCase`/`trim`/
`startsWith`/`endsWith`/`includes`) and `Number.prototype.toString`(10) carry a
measured `33280`-raw host residual; `JSON.stringify` of a primitive carries the
`82432`-setup + `16384`-produced residuals over the final result chunk (its
working buffer is an unmetered C-malloc). The four built-ins dual-run sections
agree bit-exactly with **zero divergence**, every skip named:
`built-ins/Math total=275 covered=151 divergent=0 skipped=124`,
`built-ins/Number total=281 covered=59 divergent=0 skipped=222`,
`built-ins/String total=1111 covered=115 divergent=0 skipped=996`,
`built-ins/JSON total=138 covered=2 divergent=0 skipped=136` (before this child
all four were ~0 covered — the constructors bound as values but no methods).
The deferred paths are honest **named skips**, never faked: `indexOf`/
`lastIndexOf`'s inner-loop scan metering (single-character and not-found agree;
a multi-character partial-then-full match over-counts), `Number.prototype.
toString` at a non-decimal radix, non-ASCII case/trim and astral offset math, a
String method result consumed *directly* (without an intervening variable) as a
receiver/argument (an extra temporary-lifetime residual), and — the largest
remaining work — **`JSON.parse`** and **structured `JSON.stringify`** (the
object/array serializer is implemented and its RESULT is correct, but the
per-node allocation metering — the keys instance, per-key strings, and
recursive property frames — is not yet modeled to a clean constant, so it
self-names rather than ship a computron divergence). The stage-3
**keyed-collections** child binds `Map`/`Set`/`WeakMap`/`WeakSet` as intrinsics
(modeled in a per-instance side table like the exotic Array): construction, the
core `set`/`add`/`get`/`has`/`delete`/`size` methods, and — the stage-3b
remainder — the **iteration protocol** (`forEach`, `entries`/`keys`/`values`,
and `for-of` / spread over a Map or Set), all bit-exact (result AND computron)
against the pin. Metering is purely allocation-driven (xsMapSet.c calls no
`mxMeter`): the construct path's four `fxNewSlot`s + initial
`fxNewChunk(mxTableMinLength*8)`; the per-linked-slot residual (`1<<15` per entry
slot beyond the first) an inserting `fxSetEntry`/`fxSetWeakEntry` charges; the
`fxResizeEntries` rehash chunk on XS's exact power-of-two table boundaries;
SameValueZero key equality (`NaN` equals `NaN`, `-0` normalized to `+0`). The
stage-3b iteration adds the Map/Set Iterator (`fxNewMap/SetIteratorInstance`,
reusing the array-iterator dispatch) whose creation cluster is calibrated
computron-exact, an entries-yield's `fxConstructArrayEntry` pair (a `2<<14` frame
residual over the modeled two-element chunk; keys/values yields carry no
residual), and `forEach`'s per-entry call frame (`2<<16` per live entry, over the
callback body the nested dispatch meters; a Map's native frame is 8 raw units
over a Set's — Map walks a key→value slot pair per entry, Set a single slot). The
four collection dual-run sections agree bit-exactly with **zero divergence**,
every skip named:
`built-ins/Map total=144 covered=25 divergent=0 skipped=119`,
`built-ins/Set total=188 covered=37 divergent=0 skipped=151`,
`built-ins/WeakMap total=88 covered=11 divergent=0 skipped=77`,
`built-ins/WeakSet total=75 covered=9 divergent=0 skipped=66`
(and `MapIteratorPrototype`/`SetIteratorPrototype` `divergent=0`, covered=0 —
their tests exercise `Symbol.toStringTag` / direct-prototype corners endor
honestly skips). The Map/Set `for-of` contribution also grows
`language/statements/for-of` to `covered=89` (from the arrays child's 79), still
`divergent=0`. The stage-3b remainder also lands `clear` (`fxClearEntries`: drop every entry
and shrink the address table back toward `mxTableMinLength`), computron-exact.
The deferred collection paths are honest **named skips**: the
copy-constructor iterable argument (`new Map([[k,v]])`), a WeakMap/WeakSet
primitive key (a TypeError in XS), mid-iteration structural mutation, and the
ES2025 Set combinators (`union`/`intersection`/…) — each self-names
`Halt::Unsupported` rather than resolve to a wrong value or a computron
divergence. The stage-3 built-ins reach
endor's intrinsics by name: the oracle's `symbols` atom (decoded by
`endor-vm::symbols`) carries the C-XS compiler's program-local id→name
table, so a `Boolean`/`Object`/… reference relinks to endor's intrinsic
under the id that program assigned it (`endor_vm::run_program_with_symbols`). Per the maintainer directive on PR #600
(2026-07-03), the whole-section parity runs that succeed them draw from the
monorepo's existing `packages/test262-runner` test262 subset — the same
tree and convention that package already uses to prove XS↔Node HardenedJS
parity — rather than a separate pinned test262 submodule. The
`test262-language` runner (module `endor_262::test262`) assembles each
`language/` test the standard test262 way and dual-runs it, reporting an
**honest covered/skipped split**: `covered` is bit-exact (result AND
computron, four-valued completion) only; every skip is named by the opcode
or built-in gap that stopped endor (never folded into a pass rate); a wrong
primitive value is a hard divergence. The covered grammar is expressions,
`var`/scope/loops, objects, functions, closures, and exceptions — it does
not include the built-ins (`eval`/`String`/`Array`/`typeof`/real `Error`
objects) the bulk of `language/` needs, so most tests are honestly skipped
today, the covered count growing as later stages land the built-ins. See
the design's § test262 conformance (requirement 6).
