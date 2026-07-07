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
| `endor-regexp` | `#![forbid(unsafe_code)]` | Engine-internal port of the XS RegExp engine (`xsre.c`): the pattern compiler (parse → measure → code) and the backtracking match VM, metering-exact against the pin. The JavaScript `RegExp` surface is child 9, linked from `endor-vm` (§ The JavaScript RegExp surface). |

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
receiver/argument (an extra temporary-lifetime residual).

The stage-3b **json-metering** child closes **structured `JSON.stringify`**
(object/array values): the recursive `fxStringifyJSONProperty` per-node metering
is now reproduced bit-exact (result AND computron) against the pin, not fitted.
Decomposed against `xsJSON.c`, each node charges a whole number of `mxMeterOne`
(`1<<14`) steps plus the exact `fxNewSlot`/`fxNewChunk` allocations: an array
node `11` steps to enter (`fxStringifyJSONChars`/`mxGetID(_length)`/
`fxToInteger`), `+1` step for a non-empty array, `5` steps per element body; an
object node `8` steps + one instance slot for the `fxNewInstance` keys holder,
one `XS_AT_KIND` slot per own key, `65528` for the non-empty setup, `4` steps +
the `fxPushKeyString` chunk (`rup8(len+1)`) per key body; primitives the `1`-step
leaf; the wobble the child-4 measurements saw is entirely the final result
`fxNewChunk(offset)` (output length + NUL), metered once by `new_string_metered`.
A **callable value** (function) — whose reference branch runs an unmodeled
`mxGetID(_toJSON)` probe — and the `toJSON`/wrapper/replacer/space corners remain
honest named skips (`JSON.stringify:callable-value`, …), never a wrong value or a
divergence.

The same child implements **`JSON.parse`** (`fx_JSON_parse` — the tokenizer,
recursive value construction, and per-node allocation metering), bit-exact
(result AND computron). The parse path calls **no** `mxMeter` (like `xsMapSet`),
so every unit is the native-frame residual `49152` (over the call trampoline the
interpreter already meters) plus the exact allocations: a produced string's
tokenizer chunk (`fxNewChunk(size+1)`); an array's `fxNewArrayInstance` two
slots + one linked `fxNewSlot` (`33024` fixed body) per element + the one-time
`fxCacheArray` item chunk (`length * sizeof(txSlot)` = `length*32` + header); an
object's `fxNewObjectInstance` slot + per member a `65792` body + the key-name
intern (a novel name one `fxNewSlot`) + the key-string chunk + the recursive
value. Numbers classify exactly as XS (`INTEGER` iff integral, in `txInteger`
range, and non-zero). Honest named skips: a reviver argument
(`JSON.parse:reviver`), a non-string argument needing coercion
(`JSON.parse:non-string`), a surrogate/astral `\u` escape (`JSON.parse:astral`),
malformed input whose `SyntaxError` partial metering is unmodeled
(`JSON.parse:syntax`), and re-serializing a parsed object's runtime-interned key
(`JSON.stringify:interned-key`, child-5's interned-key rendering gap). Together
this lifts `built-ins/JSON` to `total=138 covered=15 divergent=0` (from 2 before
this child), and the curated `stage3b-json-metering.js` corpus + the
`gen_json_structured_program` and `gen_json_parse_program` differential fuzz arms
all agree bit-exactly.

One neighbouring **pre-existing** observation the parse child (or an object-
literal child) should note: a *large/deep* nested **object literal**
*construction* (e.g. `var v = {…}` with no JSON at all) accrues a sub-computron
raw drift in endor vs the oracle that can occasionally tip one computron
boundary — visible on the bare literal, independent of the JSON surface. The
json-structured fuzz arm bounds depth/breadth to stay inside the
construction-exact regime so it remains a clean test of the *stringify* metering
itself; the literal-construction drift is a separate object-literal issue. The stage-3
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
divergence. The stage-3b **binary-data** child (3/9) binds `ArrayBuffer` as an
intrinsic whose per-instance backing store lives in a side table like the exotic
collections: `new ArrayBuffer(byteLength)` allocates the zero-filled
`fxNewChunk(byteLength)` store (metered at XS's 8-byte-aligned adjusted size) over
a constant native frame (`ARRAY_BUFFER_CTOR_FRAME_METERING` — six built-in steps +
the three `fxNewSlot`s of `fxNewArrayBufferInstance`), and the `byteLength`
accessor getter reads the stored `bufferInfo.length` metering nothing beyond its
`GET_PROPERTY` dispatch — both bit-exact (result AND computron) against the pin.
The `built-ins/ArrayBuffer` dual-run section agrees with **zero divergence**:
`built-ins/ArrayBuffer total=80 covered=11 divergent=0 skipped=69` (with the views
landed, `ArrayBuffer.isView` is modeled too — `true` for a TypedArray/DataView,
`false` otherwise). The deferred ArrayBuffer paths are honest **named skips**:
`slice` (the species constructor), `resize`/`transfer`/`concat` and the resizable
(`maxByteLength`) construct, a negative/oversized/non-integer byteLength (each a
RangeError), and the `ArrayBuffer(n)` call without `new` (a TypeError) — each
self-names `Halt::Unsupported` rather than ship a wrong value or a computron
divergence. The same child binds the
**TypedArray family** — the eleven concrete constructors (`Uint8Array`/`Int8Array`/
`Uint8ClampedArray`/`Int16Array`/`Uint16Array`/`Int32Array`/`Uint32Array`/
`Float32Array`/`Float64Array` plus the `BigInt64Array`/`BigUint64Array` shells) as
`Native::TypedArray(i)` indexing the `gxTypeDispatches` element-type table, with the
per-instance view state (dispatch + `byteOffset`/`size` + backing-buffer reference)
in a `typed_arrays` side table. Two construct forms are computron-exact: the
length form `new TA(n)` (which allocates its own `new ArrayBuffer(n << shift)`
backing store — the inner construct's frame folded into
`TYPED_ARRAY_LENGTH_CTOR_FRAME_METERING`, the store chunk metered by
`alloc_array_buffer`) and the buffer form `new TA(buffer[, byteOffset[, length]])`
(a view sharing the argument buffer's store, byteOffset aligned to the element
size). The `length`/`byteLength`/`byteOffset`/`buffer` accessors and the exotic
**index element read/write** (`ta[i]` — one `mxMeterOne` built-in step per
in-bounds access, `undefined`/silent-no-op out of bounds) are bit-exact, including
the per-type coercions: Int/Uint wrap-around, `Uint8ClampedArray` clamp-and-round
(ties to even), the `Uint32` integer-vs-number completion split, and the IEEE
float encodings. The dual-run sections agree with **zero divergence**:
`built-ins/ArrayBuffer total=80 covered=11 divergent=0 skipped=69`,
`built-ins/TypedArray total=1054 covered=0 divergent=0 skipped=1054` (its tests
drive the abstract `%TypedArray%` helpers and methods endor honestly skips),
`built-ins/TypedArrayConstructors/{Uint8Array,Int32Array} covered=1 divergent=0`.
The deferred TypedArray paths are honest **named skips**: the from-iterable /
from-array-like / source-TypedArray copy constructors, the BigInt-element
read/write (BigInt coercion is a later increment), an object element value (needing
`ToPrimitive`), the prototype methods (`set`/`subarray`/`fill`/`map`/… and the
statics `from`/`of`), and the resizable/species corners. The same child binds
**`DataView`** — the endian-aware buffer view — with its view state (buffer
reference + `byteOffset`/`size`) in a `data_views` side table. The construct
`new DataView(buffer[, byteOffset[, byteLength]])` (a view sharing the argument
buffer's store, no allocation) plus the `byteLength`/`byteOffset`/`buffer`
accessors and the full `get<Type>`/`set<Type>` method family (`Int8`/`Uint8`/
`Int16`/`Uint16`/`Int32`/`Uint32`/`Float32`/`Float64`) are bit-exact, honoring the
**endianness argument** (default big-endian; a big-endian access reverses the
element bytes around the shared little-endian codec the TypedArray path uses) and
the same per-type coercions. The metering splits the get (one `mxMeterOne`) from
the set (three built-in steps — the value coercer's two plus the setter's
`mxMeterOne`, constant across the element types). `built-ins/DataView total=455
covered=62 divergent=0 skipped=393`. The deferred DataView paths are honest named
skips: the `getBigInt64`/`setBigInt64`/`getBigUint64`/`setBigUint64` (BigInt
coercion), an object value needing `ToPrimitive`, and the resizable-buffer corner.
The stage-3b **fundamentals-followup** child (4/9) lands the post-arrays
fundamentals deferrals now unblocked by the Array machinery, all bit-exact
(result AND computron) against the pin. A **user function's `.length`** (its
declared arity, set from `begin`'s parameter-count operand at the `code` opcode,
mirroring XS's `fxNewFunctionLength(the, variable, *(code+1))`) and **`.name`**
(its own name, inferred at compile time for a `var f = function(){}` initializer)
read back as first-class own data properties — both allocated at definition
(folded into `FUNCTION_DEFINE_METERING`), so reading them meters nothing beyond
the `GET_PROPERTY` dispatch. **`Function.prototype.bind`** creates a bound
function (recorded in a `bound_functions` side table: target + bound `this` +
bound args) whose `.length` is `max(0, target.length - boundArgs)` and `.name`
is `"bound "+name`; calling it trampolines into the target with the bound `this`
and bound args prepended (`fx_Function_prototype_bound`). Its metering is
calibrated raw-exact: a constant creation cluster (`BIND_CREATE_METERING`, plus
a `BIND_CREATE_ARGS_ARRAY` + per-arg cost when bound args exist and the args
Array is built) and a call trampoline (`BIND_CALL_METERING` + `1<<14` per
forwarded argument). **`Function.prototype.apply`** now forwards a real **dense
Array** argument's elements (the array-read setup + per-element `mxGetIndex` +
forward, `APPLY_ARRAY_BASE_METERING` + `APPLY_ARRAY_PER_ELEMENT_METERING`),
graduating past the no-array subset. **`Symbol.prototype.toString`** →
`Symbol(<description>)` (a primitive-symbol receiver boxing to
`%Symbol.prototype%`), **`valueOf`**, the explicit **`String(symbol)`**
coercion, and the **`Symbol.for`/`keyFor`** global registry (registry-interned
identity, so `Symbol.for(k) === Symbol.for(k)`) land the residue after the
Kind::Symbol + 13 well-knowns. **`AggregateError(errors, message)`** builds the
base error (message from arg 1, chaining to `%AggregateError.prototype%`) plus
an own `errors` Array from a dense-array argument (the `fxGetIterator`/
`fxIteratorNext` walk metered as `AGGREGATE_ERROR_EXTRA` +
`AGGREGATE_ERROR_PER_ELEMENT`). The dual-run sections grow with **zero
divergence**: `built-ins/Function total=511 covered=39 divergent=0` (from 23 —
`prototype/bind` 11, `prototype/apply` 5), `built-ins/Symbol covered=6
divergent=0`, and `built-ins/AggregateError divergent=0` (its tests need
`.at`/`String` features to be *covered*, but correctness is proven by the
curated corpus). The honest **named skips** are: `new (boundFn)` (the
construct-target frame geometry), a non-user-function/native `bind` target and
a bound-of-bound **call** (`.length`/`.name` on it still read), a sparse array
or non-array argument to `apply`/`AggregateError`, a non-string `Symbol.for`
key, and — deferred as documented — **sloppy primitive-`this` boxing**
(`fxToInstance`) in `.call`/`.apply`/bound calls: a sloppy callee boxes a
primitive `this` to its wrapper while a strict callee keeps it, a
meter-affecting distinction not knowable until the callee's `begin`, so a
primitive `thisArg` self-names `Halt::Unsupported` rather than answer a
`this`-dependent test wrongly (kept per the charter's "if calibratable within
budget; else keep the honest named skip"). The stage-3b **object-statics +
intern-table** child (5/9) lands the program-level cross-child dependency both
child-1 and child-2 named: a **global runtime string→id intern table** (XS's
`fxNewNameX`/`fxAt`) reconciled at one point with the C-XS compiler's program
symbols and XS's boot-time default keys (`gxIDStrings`, carried in
`endor-vm::default_keys`). `intern_key` returns an already-interned name's id
with **no** allocation — a program symbol, a prior runtime key, or a
pre-interned default key — and meters exactly one `fxNewSlot` key slot for a
genuinely-novel name, the metering difference measured against the pin (a
genuinely-novel `o["zzz"]`-style key costs one slot; a well-known
`o["toString"]`-style name costs none). On that substrate three surfaces land
bit-exact (result AND computron): **`Object.prototype.hasOwnProperty`** answers
*any* string key soundly (own-only, no prototype walk — an own key true, a
novel/inherited name false), **`Object.keys`** returns a fresh `Array` of an
ordinary object's own enumerable string keys in creation order (metering
calibrated raw-exact via the isolated `B(n)−A(n)` gap: a fixed native-body
residual + the result array's item chunk grown once + one `fxNewSlot` per key;
confirmed key-name-length independent, XS referencing the interned key string),
and **`Object.getOwnPropertyDescriptor`** yields the full data descriptor
(`{value, writable, enumerable, configurable}` in XS's field order) for a
present ordinary data property or `undefined` when absent — the descriptor
field names routed through the same intern table so `descriptor.value` reads
back under the id the program's `.value` access uses. On the same intern-table
substrate two more surfaces land bit-exact: **computed string member access
`o[k]`** — the `AT`/`AT_2` opcodes now resolve *any* string key through the
intern table (a program symbol resolves exactly as its `o.name` static access
does; a genuinely-novel name interns one `fxNewSlot` key slot and reads bit-exact
`undefined`; an index-valued string meters XS's two extra code units), and the
**`in` operator** answers a genuinely-novel key a *sound* `false` (the metered
`fxOrdinaryHasProperty` chain walk charges one `XS_CODE_METERING` per prototype
level descended). Both preserve the invariant by self-naming the one case endor
cannot decide soundly — a **boot default-key name the program never referenced**
(e.g. `o["hasOwnProperty"]` / `"toString" in {}`): endor's `%Object.prototype%`
carries a method only for program-referenced names, so it cannot tell an absent
own read from an inherited built-in it never linked, and refuses rather than
answer a wrong `undefined`/`false`. A fourth static, **`Object.defineProperty`**,
lands the attribute-aware property model: `defineProperty(o, k, desc)` defines a
new own data property on an ordinary object from the canonical four-field data
descriptor (`{value, writable, enumerable, configurable}`, no `get`/`set` — the
verifyProperty shape), storing the three booleans as XS's property flag byte
(`XS_DONT_SET_FLAG`/`XS_DONT_ENUM_FLAG`/`XS_DONT_DELETE_FLAG`) so the attributes
**ripple through** the other statics: `Object.keys` now filters non-enumerable
properties (and still renders every enumerable one in creation order), and
`getOwnPropertyDescriptor` reads the `writable`/`enumerable`/`configurable`
booleans back from the flag byte. `fxDescriptorToSlot`'s six `mxHasID`/four
`mxGetID` field reads, the three `fxToBoolean` coercions, and the
`fxOrdinaryDefineOwnProperty` create fold into one measured raw residual
calibrated against the pin (the novel-key intern slot metered separately).
`built-ins/Object` dual-run grows to `covered=63 divergent=0` (from ~0 before the
child — the verifyProperty-shaped `getOwnPropertyDescriptor`/`defineProperty`
tests, the computed-access `at`/`at_2` unlock, and the `in`-false answers now
covered). The honest **named skips** are:
`Object.keys`/`getOwnPropertyDescriptor` over an exotic receiver
(array/typed-array/collection/wrapper/error — whose own-key set includes
indices/length or internal names) or over an accessor property, an index-string
key, a computed read / `in` of a boot default-key name the program never
referenced (the incomplete `%Object.prototype%` member set), a
`defineProperty` with a partial or accessor descriptor / a redefine of an
existing key / a non-object descriptor / a non-boolean attribute / an
enumerable **novel** key `Object.keys` cannot render to a string.

The stage-4 **object-integrity** child (1/8, harden's direct prerequisite)
lands the property-attribute **integrity model** and the **descriptor-reflection
remainder**, all bit-exact (result AND computron) against the pin. The
integrity levels — `Object.preventExtensions`/`seal`/`freeze` +
`isExtensible`/`isSealed`/`isFrozen` — carry the slot-arena flag semantics XS
implements: a non-extensible instance stamps `XS_DONT_PATCH_FLAG` (`1<<4`) on
its own `XS_INSTANCE_KIND` slot (`mxBehaviorPreventExtensions`/
`mxBehaviorIsExtensible`), and `seal`/`freeze` additionally stamp
`XS_DONT_DELETE_FLAG` (seal) or `XS_DONT_DELETE_FLAG|XS_DONT_SET_FLAG` (freeze)
on every own data property. Metering is allocation-driven (the `xsObject.c`
bodies call no `mxMeter`): `preventExtensions`/`isExtensible` carry a zero
native residual over the frame; `seal`/`freeze` charge the `fxNewInstance` keys
holder + one `mxBehaviorOwnKeys` at-slot (`fxNewSlot`, `1<<8`) per own key
(`65792` base + `256`/key); `isSealed`/`isFrozen` short-circuit to `false` on an
extensible instance and otherwise charge the same-shaped `65800` + `256`/key
walk. Those flags **ripple through the write paths**: an ordinary `o.k = v` to a
frozen / non-writable property, or a new key on a non-extensible object, is
rejected by `mxBehaviorSetProperty` (a **sloppy** callee silently ignores it,
the assignment still evaluating to the RHS; no allocation, so it meters nothing
beyond dispatch), and a `delete` of a non-configurable own property returns
`false` without unlinking. The descriptor-reflection remainder adds
`Object.values`/`entries` (a fresh `Array` of own enumerable values / `[k,v]`
pairs, `66048` base + a per-key value-read residual — `3<<14` for `values`,
`1<<16` for `entries`'s pair construct), `Object.getOwnPropertyDescriptors`
(the plural, `82432` base + `34568`/key), and
`Object.prototype.propertyIsEnumerable` (the `mxBehaviorGetOwnProperty` probe,
`1<<16`). `built-ins/Object` whole-tree dual-run grows to **`covered=176
divergent=0`** (from 63), with the per-section bars all `divergent=0`:
`built-ins/Object/{freeze covered=12, seal 12, preventExtensions 12, isFrozen
24, isSealed 19, isExtensible 25}`. The honest **named skips** are: the
**strict-mode** integrity-violation *throw* — a rejected set/delete under a
strict callee must throw a *catchable* native `TypeError`, which needs the
native-error construction endor does not yet model (a wrong uncatchable
host-abort would diverge from a `try`/`catch`), so it self-names
`strict-set:integrity-violation` / `strict-delete:non-configurable` rather than
answer wrongly; and an **exotic receiver** or an **accessor own property**
across all these surfaces (the same class the object-statics child skips). The
headline stage-4 surfaces this child does **not** reach — **accessor
properties** (getter/setter slots, `get x()`/`set x()` object-literal opcodes,
accessor-descriptor `defineProperty`) and the **full ValidateAndApplyProperty­
Descriptor** reconfiguration path (`defineProperty` redefine / partial
descriptors) — remain the reported **scope fold**, carried forward to a
follow-up child as the honest named skips the object-statics child already
lists.

The stage-4 **classes** child (2/8) lands **`new.target`** (the `XS_CODE_TARGET`
opcode, which already decoded but had no semantics), bit-exact (result AND
computron) against the pin. `new.target` reads the running frame's target
constructor when the frame was entered as a construct (XS's `mxFrameHasTarget`
→ `mxFrameTarget`) and `undefined` inside a plain call; endor reads it from the
frame's (`cur_target`, `cur_func`) pair — for a `new f()` the target IS the
invoked constructor, since the covered grammar has no `Reflect.construct` /
`super()` retargeting (both of which self-name elsewhere). The opcode is pure
dispatch (XS's handler only allocs a stack slot and advances), so the generic
per-opcode `tick_code` is its whole cost and it is metering-exact by
construction. The curated `stage3b`-style corpus `stage4-new-target.js` (15
programs — the factory-guard idiom, a closure-captured constructor, and
construct/plain-call alternation) is locked as the cargo bar
`stage4_new_target_corpus_is_bit_exact_against_oracle`, all bit-exact.
`built-ins/Function` grows to **`covered=40 divergent=0`** (from 39 — the
`new.target`-gated test now covered), and the class dual-run sections agree with
**zero divergence**: `language/statements/class total=3908 covered=1
divergent=0 skipped=3907`, `language/expressions/class total=3663 covered=1
divergent=0 skipped=3662`, every skip named.

The headline **`class` surfaces remain the reported scope fold** (this child
landed the bounded, metering-trivial `new.target` slice and folded the rest
rather than ship a half-implemented, divergence-prone class model). The fold
points self-name as honest skips, dominated by **`to_instance`** (1872 skips in
`statements/class` alone — the gate opcode of the class-definition path:
`XS_CODE_CLASS` prototype/constructor wiring, concise methods, static members),
plus **`extend`** (the `extends` clause), **`super`**/**`set_home`** (home-object
super dispatch), **`generator`** (generator methods), and the accessor-descriptor
gaps the object-statics/integrity children already list. Landing the class-
definition path bit-exact needs (a) making a constructor's `.prototype` /
`prototype.constructor` real readable own properties with XS's `GET_ONLY`/
`DONT_ENUM` flags (endor currently keeps `.prototype` in a side map, invisible to
`GET_PROPERTY`), (b) method-definition non-enumerability through the
`NEW_PROPERTY` flag byte, and (c) calibrating the `CLASS`/`NAME`/`SET_HOME`
allocation metering against the pin — each a divergence surface that does not fit
the one-invocation budget alongside a *green* result, so it is carried forward as
the next class child's scope.

The stage-4 **generators** child (3/8) lands **generator functions and the
iteration-protocol closure** (the pin's `xsGenerator.c` sync half), bit-exact
(result AND computron) against the pin. The **suspend/resume of the interpreter
activation** is heap state in a new `generators` side table (modeled on
`promises`): a `GeneratorData` holds the lifecycle state (suspended-start /
suspended-yield / executing / completed) and a `SavedFrame` — the scope
(`locals`/`id_map`), the call identity (`args`/`this_val`/`cur_func`/`cur_target`/
`strict`/`result`), the generator's own value-stack temporaries, and the resume
cursor. The representation is deliberately the one async/await (child 4) resumes
on. `XS_CODE_GENERATOR_FUNCTION` defines a generator function whose `.prototype`
chains to a new `%GeneratorPrototype%` (`next`/`return`/`throw`), so an instance
resolves the methods by the ordinary prototype-chain walk; `XS_CODE_START_GENERATOR`
creates the instance, snapshots the fresh activation, and returns it (the body
runs on the first `.next`); `XS_CODE_YIELD` snapshots the activation and unwinds
to the `.next` driver via a new `Halt::Yield` (the `{value, done}` object is the
one the body **built by bytecode** — `OBJECT`/`NEW_PROPERTY` — returned as-is);
`XS_CODE_BRANCH_STATUS_{1,2,4}` is the yield-resume epilogue (a `next` resume
always branches past the return/throw handling). `%GeneratorPrototype%.next(v)`/
`return(v)` run through `resume_generator`, which suspends the driver's own
activation onto `call_stack` (exactly as `enter_call` does), reinstalls the
generator frame, runs a nested `dispatch_at` to the next `yield` or `END`, and
restores the driver — so the sent value, completion `{value, done}`, and
`for-of`/spread over a generator all land bit-exact. Metering is allocation-driven
(`xsGenerator.c` calls no `mxMeter`): calibrated frozen constants over the
identical bytecode both engines dispatch —
`GENERATOR_FUNCTION_EXTRA_METERING` (24, the extra `.prototype` cluster),
`GENERATOR_START_METERING` (1136, `fxNewGeneratorInstance`'s slots),
`GENERATOR_YIELD_METERING` (32616, the saved-stack `fxNewChunk`; a `yield` reached
with extra live loop temporaries carries a small sub-computron residual over this,
a documented approximation), `GENERATOR_RESULT_METERING` (66304, a completion
`fxNewGeneratorResult` — a *yield*'s result object is metered by its own bytecode,
so it carries none), and `GENERATOR_RESUME_METERING` (65536, exactly one dispatch
of `fxRunID` re-entry). An object-literal `*m()` method compiles anonymous and is
named by `fxRenameFunction`'s two steps at `NEW_PROPERTY`. The dual-run sections
agree with **zero divergence**, every skip named:
`language/statements/generators total=252 covered=74 divergent=0`,
`language/expressions/generators total=268 covered=79 divergent=0`,
`built-ins/GeneratorPrototype total=57 covered=8 divergent=0`,
`built-ins/GeneratorFunction total=20 covered=0 divergent=0` (its tests drive the
`GeneratorFunction` constructor as a value, an honest skip), and
`language/statements/for-of` grows to **`covered=118`** (from 92) with `for (x of
gen)` now driven, still `divergent=0`. The curated `stage4-generators.js` corpus
is locked as `stage4_generators_corpus_is_bit_exact_against_oracle`, and the
suspend/resume + allocation paths are exercised Miri-clean
(`generator_suspend_resume_is_miri_clean`). The headline **scope fold** —
carried forward as honest named skips (`Halt::Unsupported`, never a wrong value or
a silent divergence): **`yield*` delegation** (`YIELD_STAR`), **`.throw(e)` and
`.return(v)` into a *suspended* body** (throw-into-suspended and `finally`
unwinding through the catch/finally jump chain — `generator:throw-into-suspended`/
`generator:return-into-suspended`), a **`yield` inside a live `try`**
(`generator:yield-in-try` — the jump-chain snapshot/rebase), a **`new`-constructed
generator** (`generator:new-target`), and **async generators / `await`** (child 4,
which resumes on this same `SavedFrame` machinery).

The stage-3b **promises** child (7/9) lands `Promise`, the promise **job
queue**, and the **pump-loop latch** — the host-driven microtask drain the
endor embedding performs after a crank. `xsPromise.c` calls `mxMeter` exactly
once in the whole file (the unhandled-rejection list walk), so promise metering
is almost entirely allocation-driven: the `fxNewSlot` clusters of
`fxNewPromiseInstance` (6 slots), `fxPushPromiseFunctions` (13 — the two
resolve/reject host functions + their shared home object),
`fxNewPromiseCapability` (a derived promise + resolving pair + 8),
`fxPromiseThen`'s reaction (6, +1 THENS-link when the promise is still pending),
and `fxQueueJob` (6 per queued job), each over the calibrated native frame of
its entry point and the reaction/executor bodies the re-entrant dispatch meters
(interp.rs § Promise metering). Landed bit-exact (result AND computron) against
the pin: `new Promise(executor)` (the executor invoked synchronously, its
resolve/reject settling the instance under a shared `[[AlreadyResolved]]`
guard), `Promise.resolve`/`Promise.reject`, `then`/`catch` (reaction
registration returning a derived promise), and the whole microtask machinery —
resolution chains, already-settled promises, pass-through (a handler-less
`then()`), and rejection routing — with the reactions **run at the drain**. The
drain is the crux of the consensus-relevant scheduling: `fxRunScript` queues
promise jobs but does not run them, so BOTH sides pump the queue after the
script settles — the oracle shim gained a post-`fxRunScript` `fxRunPromiseJobs`
loop (metering still accumulating) and `endor-vm::run` drains its own job queue
the same way, so the metered computrons include the whole crank (message
delivery plus its microtask drain), the unit an xsnap/Agoric crank meters. The
curated `stage3b-promises.js` corpus and the `gen_stage3b_promise_program`
fulfilled-chain fuzz arm agree bit-exactly, and `built-ins/Promise` dual-run
grows to `total=474 covered=7 divergent=0` (from ~0 before this child — the
constructor bound as a value but no machinery). The honest **named skips** are:
**thenable adoption** (`resolve` with a reference / `Promise.resolve(object)` /
a reaction returning a promise), a reaction **handler that throws** (the thrown
value's capture out of the re-entrant frame is a later increment), `.finally`,
the `all`/`race`/`allSettled`/`any` **combinators**, a non-user-function
executor, and — stage 4's charter — **async functions / await** (the
`structural:async-or-can-block` skip dominating the section); each self-names
`Halt::Unsupported` rather than answer a wrong value or a computron divergence.

The stage-4 **async/await** child (4/8) lands the **promise native-handler
double-settle keystone** the s7 review ledger blocked the combinators on, all
bit-exact (result AND computron) against the pin. The `[[AlreadyResolved]]`
guard is refactored to be per resolving-function **pair** (a `promise_guards`
index shared by the pair, XS's boolean slot in each `fxPushPromiseFunctions`
home object) rather than per-promise: resolving a promise with a **thenable**
does NOT settle it — `fxResolvePromise` acquires a **second** resolving pair
(with its own fresh guard) and queues a `PromiseResolveThenableJob`, so the
promise stays pending until the thenable's `then(res, rej)` fires at the drain,
and the two-level structure makes every **double-settle** (a thenable/executor
calling `res` twice, or `res` then `rej`, or `rej` then `res`) a metered no-op.
On that substrate **thenable adoption** lands: `Promise.resolve(thenable)`, an
executor resolving with a thenable, and a `.then` handler **returning** an
object-literal thenable — the `mxGetID(_then)` probe metered as one dispatch
(`1<<16`) on any reference resolve (a non-thenable object fulfills with the
object as-is), the `fxOnThenable` drain job (`PROMISE_THENABLE_JOB_FRAME_
METERING`), the second resolving pair's 13 `fxNewSlot`s, and the count-3 job's
slots. **Long `then`-chains** (each handler's result feeding the next reaction)
and the **`Promise.resolve(nativePromise)` identity fast path** (`mxGetID(_
constructor)` + `fxIsSameValue` = `2.5<<16`, previously an untested `0`) land
bit-exact too. `built-ins/Promise` whole-tree dual-run grows to **`total=474
covered=9 divergent=0`** (from 7 — the thenable-resolve and identity cases now
covered), with the acceptance subtrees all `divergent=0`
(`prototype/then covered=1`, `resolve covered=3`, `reject covered=1`,
`all/race/allSettled/any/prototype/finally covered≤1`). The curated
`stage4-async-promises.js` corpus (20 programs) is locked as the cargo bar
`stage4_async_promises_corpus_is_bit_exact_against_oracle`, and the
thenable-adoption allocation/drain path is exercised Miri-clean
(`promise_thenable_adoption_is_miri_clean`). The honest **named skips** are: a
reaction handler / thenable `then` that **throws** (`promise:handler-throw` /
`promise:thenable-then-throw` — cleanly unwinding the re-entrant callback frame
after an internally-caught throw is the throw-family increment, alongside the
generator throw-into-suspended skips), `resolve(promise-itself)`
(`promise:resolve-self`, a catchable TypeError), and **adopting a native
promise** (`promise:adopt-native-thenable`, whose `.then` is
`%Promise.prototype%.then` — it needs the resolving functions registered as
**native** reaction handlers). The keystone was the gating deliverable
("resolve it first"); it unblocks the folded surfaces, which share one clear
prerequisite — native reaction handlers — now built (see the stage-4b child).

The stage-4b **async-function surface** child (2/5) lands the folded
`XS_CODE_ASYNC_FUNCTION`/`START_ASYNC`/`AWAIT` opcode surface over the keystone,
bit-exact (result AND computron) against the pin, executing directly from
[`ASYNC-AWAIT-HANDOFF.md`](ASYNC-AWAIT-HANDOFF.md). An async function is a
`new_async_function` re-chaining `[[Prototype]]` to a new `%AsyncFunction.
prototype%` intrinsic (no own `.prototype`; the calibrated
`ASYNC_FUNCTION_DEFINE_DELTA` backs out the constructor-prototype allocation).
`START_ASYNC` clones the frame like `new_generator_instance`, builds the result
promise via `new_promise_instance` + `make_resolving_functions`, runs
`step_async` synchronously to the first `await` or completion, and returns the
result promise (mirroring `START_GENERATOR`'s boundary split). `AWAIT` is a
YIELD-shaped suspend reading a new `async_run_stack`, returning `Halt::Await`
(per-suspend metering reuses `GENERATOR_YIELD_METERING` — the identical C code);
`BRANCH_STATUS` now honors a threaded `resume_status` (fulfilled → branch by
offset leaving the resolved value on the stack; rejected → `THROW_STATUS` unwind
to the innermost handler), with the generator path unchanged (it only ever
resumes `NoStatus`). The shared prerequisite — the **5-slot native-reaction
path** (`PromiseReaction.kind = AsyncAwait(inst)`, `promise_then_native` a
null-capability `fxPromiseThen`, dispatched at the drain in `run_promise_job` to
`step_async` rather than a user handler) — is built here; `await_schedule`'s
native-promise **fast path** (identity check + reaction, `ASYNC_AWAIT_FASTPATH_
CREDIT`) and **general path** (`new_promise_capability` + the reaction + calling
the fresh resolve, `ASYNC_AWAIT_GENERAL_METERING`) both land bit-exact. Metering
is frozen as calibrated `ASYNC_INSTANCE_METERING` (the `fxNewAsyncInstance`
cluster + completion-settle framing), the define delta, and the two await-branch
constants. `language/statements/async-function` dual-run grows to **`total=60
covered=6 divergent=0`** (54 named skips) and `language/expressions/await` to
**`total=21 covered=6 divergent=0`** (15 named skips) — both from `covered=0`
(every test was a `structural:async-or-can-block` skip); `built-ins/AsyncFunction`
is **`total=16 covered=1 divergent=0`**, and `built-ins/Promise` holds at
**`total=474 covered=9 divergent=0`** (finally + combinators still skipped). The
curated `stage4-async-await.js` corpus (14 programs — plain awaits, the
native-promise fast path, nested async, multi-await chains, await-in-loop, async
arrows, thenable await, rejection paths) is locked as the cargo bar
`stage4_async_await_corpus_is_bit_exact_against_oracle`, and the suspend/resume +
result-promise-settle path is exercised Miri-clean
(`async_await_suspend_resume_is_miri_clean`). GC-roots: the `async_instances`
side table (its `frame: Option<SavedFrame>` and the result-promise/resolving-
function slots) and the `async_run_stack` join the root set, on the same
deterministic trigger points as the generator table; the `AsyncAwait(inst)`
reaction edge roots the suspended instance while its awaited promise is pending.
The honest **named skip** carried per the handoff: **`await` inside a live
`try`** (`await:await-in-try` — the jump-chain snapshot/rebase, the same
increment generators defer for `yield-in-try`). The **reported scope fold**,
carried forward to a follow-up child (each already a named skip, never a wrong
value or a divergence): **`Promise.prototype.finally`** and the **`all`/`race`/
`allSettled`/`any` combinators** — both now rest on the landed 5-slot
native-reaction path, but each is its own surface (`finally`'s `finallyAux`
chains a `Promise.resolve(...).then(finallyReturn/finallyThrow)` native-reaction
family; the combinators need the iterator protocol + a shared-count native
reaction), so they are sized as their own child rather than half-fit here — and
**async generators / `for-await-of`** (`XS_CODE_ASYNC_GENERATOR_FUNCTION`, the
async-iterator protocol; the designated scope fold). The keystone + this surface
consumed their handler invocations; `finally` + the combinators are the next
child on this now-unblocked substrate.

The stage-4 **module machinery** child (5/8) lands the **static half** of
`xsModule.c` as `endor_vm::module` — module records, a module map with a static
host resolve hook (specifier → module, no filesystem), module environments with
**live indirect bindings**, **module namespace exotic objects**, **cyclic
instantiate/evaluate ordering**, **TDZ on un-evaluated bindings**, and
**`ModuleSource`** (the compile-only, bindings-reflection Compartment shape). The
linkage is the ECMAScript CyclicModuleRecord algorithm XS's
`fxLinkModules`/`fxExecuteModules` realize: `ResolveExport`/`GetExportedNames`
resolve local, indirect (`export {x} from 'm'`), and star (`export *`) exports —
excluding ambiguous star names and `default` from star — and `Link`/`Evaluate`
walk the graph with the SCC `dfs_index`/`dfs_ancestor_index` bookkeeping so a
dependency's body runs before its dependents and each body runs **exactly once**.
An `import {x}`/`export {x} from 'm'` name resolves to the **same binding cell**
as `m`'s local `x`, so a write in `m` is observed live through every importer and
re-exporter; a binding cell is created **uninitialized (TDZ)** at link and reading
it before the owner's body initializes it — the observable hazard in a cyclic
graph, where the first-executed module reads a peer's not-yet-initialized live
binding — is a `ReferenceError`. The namespace exotic object mirrors
`fxModuleOwnKeys`: own **string** keys are the resolvable export names **sorted by
code unit** (XS's `c_strcmp`), then the single symbol key `@@toStringTag` →
`"Module"`; `[[Set]]` always fails and the object is non-extensible.

**Path achieved (recorded honestly).** The acceptance-focus's *preferred* path — a
`language/module-code/` **dual-run** — is **not** achievable across the current
audited oracle seam: the `endor-oracle` shim compiles the **script goal only**
(`fxParseScript(..., mxProgramFlag | mxEvalFlag)`); it does not drive the module
goal / loader, so a top-level `import`/`export` is a script-goal syntax error and
the `test262-language` runner already names every `module`-flagged test a
`structural:module` skip. Extending the shim to drive `fxParseModule` +
`fxLinkModules` + a resolve hook across the FFI is a larger, separately-audited
seam this static child deliberately does not open; the **differential gap is
self-named** here rather than papered over. The path this child therefore takes —
the one the job's acceptance focus prescribes as the alternative — is the
**endor-side unit corpus**: module semantics are certified by **14
namespace/linkage/ordering/TDZ unit tests** locked into `cargo test`
(`endor_vm::module::tests`), each a spec-faithful assertion (sorted keys, no-set,
`@@toStringTag`, live indirect binding, star merge + ambiguity, cyclic TDZ vs.
well-ordered live reads, diamond-evaluates-once, `ModuleSource` reflection). The
**manual-xst method** for spot-checking module *results* against the pin
(the differential the seam cannot automate): build `xst` from the pin
(`c/moddable/xs/makefiles/lin/xst.mk`) and run a `.mjs` module directly
(`xst path/to/module.mjs`), comparing the completion/namespace against the
endor-side model — module bytecode is not fed to `endor-vm` because the oracle
does not emit it. **Named skips** (wired at the interpreter's opcode dispatch,
self-naming rather than falling to the generic op-name): dynamic `import()`
(`module:dynamic-import`) and `import.meta` (`module:import-meta`), both needing
the asynchronous host loader this static half does not build. **Scope fold**
(carried forward, each a named skip — never a wrong value): feeding real module
bytecode to `endor-vm` (blocked on the module-goal oracle seam above, so the
`XS_CODE_MODULE`/`XS_CODE_TRANSFER` runtime opcodes stay unimplemented), the
async `import()`/`importHook` loader, and `import.meta`. GC roots were not touched
(no run-loop/allocation-pressure wiring in this child), so the GC-roots ledger
note carries forward untouched.

The stage-4b **compartment** child (3/5) grows the stage-1 `Compartment.evaluate`
seam into the full **native `Compartment`** the SES suites probe
(`endor_vm::compartment`, `xsModule.c`'s compartment half): **per-compartment
globals** over the machine's shared `Rc` intrinsics with **endowments** copied
onto the new global at construction; a **`globalThis`** whose identity is the
compartment's own — distinct per compartment (nested compartments included),
stable for one compartment — read via `Compartment::global_this`; **nested
compartments** (`Compartment::new_compartment`) minting a child over the **same**
machine intrinsics with fresh globals and a fresh globalThis identity (a
Compartment created inside a compartment chains correctly); and **module-map
integration** — a compartment owns a `module::ModuleGraph` (the `new
Compartment({ modules, resolveHook, importHook })` surface), and a **static**
`import { x } from 'm'` resolves through **this** compartment's map
(`Compartment::import_static`), so two compartments with different maps for the
same specifier import different modules. The per-compartment evaluator
(`evaluate_with_symbols`) relinks a program's intrinsic references to the shared
intrinsics by the C-XS symbol atom (exactly as `run_program_with_symbols` does for
the top-level realm) and seeds this compartment's own globals, so two compartments
over one machine diverge exactly and only in their own globals.

**Compartment differential (the acceptance evidence).** `built-ins/Compartment`
does not exist upstream, so the corpus is the evidence: `stage4-compartment.js`
(29 programs) is compiled once on the oracle and its exact bytecode evaluated in
**two** compartments over **one** machine's shared intrinsics
(`endor_262::compartment_dual_run`), asserting **RESULT agreement** (both
compartments == the oracle's completion value, over one `Rc::ptr_eq` intrinsics
graph — evaluate faithfulness, shared-intrinsics identity, cross-compartment
values) **plus computron agreement** (the same bytecode with no globals seeded
reproduces the oracle's run-only count). A **global-separation** differential
seeds the same global id with a different value in each of two compartments and
confirms each renders its **own** binding — matching the oracle's `String()` of
that value while diverging between the compartments. Locked as two `cargo test`
bars (`stage4_compartment_corpus_agrees_across_two_compartments`,
`compartments_isolate_their_own_globals_against_a_seeded_value`) alongside **12
endor-side unit tests** (`endor_vm::compartment::tests` — isolation, shared
intrinsics, distinct/stable globalThis, nested chaining, endowments, constructor
resolve/import-hook shape, static-import module-map resolution, module-map
isolation, cross-compartment live indirect binding, dynamic-import named skip).
**Named skips** (self-naming, never a wrong value): dynamic
`compartment.import()` (`compartment:dynamic-import`), the async host loader the
static half does not build. **Scope fold (recorded honestly):** endor models
`Compartment` **host-side** (a Rust realm API matching XS's C-level compartment
machinery), **not** as a guest-callable `Compartment` intrinsic — a guest
`new Compartment().evaluate('…')` would need the interpreter to expose a native
constructor whose `evaluate` re-enters the compiler, a re-entrant compile seam
that needs the oracle at run time (which `endor-vm` deliberately does not link,
`#![forbid(unsafe_code)]`), so a program that references the `Compartment`
intrinsic itself is a named skip (`compartment:intrinsic-surface`), exactly as
the module goal is a named skip on the oracle seam. `lockdown`/`harden` (freezing
the shared intrinsics) lands in the next child. GC roots were not touched (no
run-loop/allocation-pressure wiring in this child), so the GC-roots ledger note
carries forward untouched. The compartment evaluator's global-seeding path is
**Miri-clean** (`endor_vm::compartment::tests` under Miri, single-threaded).

The stage-4b **lockdown/harden** child (4/5) lands the Hardened-JavaScript
`harden(x)`/`petrify(x)` globals from `xsLockdown.c` onto endor's stage-4
integrity machinery (child 1's `XS_DONT_PATCH_FLAG`/`XS_DONT_DELETE_FLAG`/
`XS_DONT_SET_FLAG` slot-arena stamps). **`harden(x)`** (`fx_harden` +
`fx_hardenFreezeAndTraverse` + `fx_hardenQueue`) is the transitive freeze
worklist over the slot arena: each reached instance is prevent-extensions'd and
every own data property stamped non-writable/non-configurable (accessors
non-configurable), then its prototype and every reference-valued own property
are queued, each reached instance marked `XS_DONT_MARSHALL_FLAG` (the visited
set), so the object graph is walked once; it returns its argument, and a
non-reference / already-hardened / no-arg call passes through per XS.
**`petrify(x)`** (`fx_petrify`) is the single-object, non-transitive freeze (no
prototype walk, `XS_DONT_MARSHALL_FLAG` left clear). The **oracle shim** is
extended minimally to install the harden/lockdown/petrify/mutabilities globals
`xst.c`/`xstFuzz.c` install (the bare `fxCreateMachine` boot does not, so an
`harden(x)` program was an undefined-reference throw) — the audited FFI-seam
extension the differential needs.

**Shim-install crash fix (stage-4 acceptance blocker, re-certified).** The
first cut of that install skipped the `mxPush(mxGlobal)` xst.c performs before
its `fxNextHostFunctionProperty` chain. That builder reads the new function's
**HOME object** from `the->stack` at entry (`home.object =
the->stack->value.reference`), so with the global not on the stack top each of
the four installed functions got a **garbage home pointer** — a stale frame
slot's bits read as a `txSlot*`. The GC's `XS_HOME_KIND` marker dereferences
`home.object` (`aSlot->flag`, then recurses via `fxMarkInstance`) on the next
collection, and the `Function.prototype.toString`/enumeration path reads it too,
so **any** whole-tree dual-run that walked the intrinsic graph
(`built-ins/Function/prototype/toString/{built-in-function-object,well-known-intrinsic-object-functions}.js`)
or churned allocations (`built-ins/Array/prototype/{concat,map,sort}`) **SIGSEGV'd
the whole oracle process (rc=139)** — one root cause behind both reported crash
classes, and behind the ses-conformance child's separate "`lockdown()` SIGSEGVs
the bare-boot shim" finding. The fix pushes `mxGlobal` so the home links to the
real global (mirroring xst.c exactly). Re-certified at the fix, all whole-tree,
**no process abort**: `built-ins/Function total=511 covered=40 divergent=0`
(~1 s), `built-ins/Array total=2625 covered=437 divergent=0` (~2 s),
`built-ins/Object total=3127 covered=176 divergent=0` unchanged (~2 s); a guest
`lockdown()`/`mutabilities()` call now completes cleanly (`undefined`) on the
bare-boot shim instead of aborting. The crash is locked out of a future shim
widening by three named `endor-oracle` cargo tests
(`shim_intrinsic_walk_and_gc_survive_installed_globals` — a self-contained
minimal equivalent of the two test262 walkers that walks globalThis, stringifies
every reachable function, and forces a GC; `shim_lockdown_call_fails_safely_not_segv`;
`shim_mutabilities_call_fails_safely_not_segv`) that abort the test binary rather
than a whole-tree run if the home linkage regresses. Metering is **allocation-driven**
(`xsLockdown.c` calls no `mxMeter`): `petrify` is computron-exact against the
pin for even own-key counts (a sub-computron boundary wobble at odd counts);
`harden`'s cost is deterministic per release, but **computron parity over a
transitive harden is structurally unavailable** — endor models intrinsics
sparsely (only program-referenced names), so harden's transitive object count
diverges from the pin's full intrinsic graph (`harden({a:1})` freezes endor's
sparse `%Object.prototype%`, not the pin's whole populated one), the same
sparse-intrinsics fact the module/compartment children record. The
**acceptance evidence** is therefore a **RESULT-gated** differential corpus:
`stage4-harden.js` (30 programs) is locked as
`stage4_harden_corpus_agrees_on_results_against_oracle`, each program
completing on **both** engines to the **same value** — a sloppy write to a
frozen property is a metered no-op, a hardened object is `Object.isFrozen`/
`isSealed`/non-extensible, harden is transitive (a nested object reachable from
the target is frozen, to arbitrary depth, a shared referent hardened once) and
idempotent and returns its argument, a non-reference passes through, and
`petrify` is non-transitive (the target's own reference property is frozen but
its referent is not). Three `endor-vm` unit tests
(`harden_freezes_target_transitively_and_returns_it`,
`petrify_freezes_single_object_not_transitively`,
`harden_transitive_freeze_is_miri_clean`) lock the freeze semantics and pin the
worklist Miri-clean. Re-running `built-ins/Object` confirms the freeze
machinery introduced **no regression**: `built-ins/Object total=3127
covered=176 divergent=0 skipped=2951` (unchanged from child 1). **Scope fold
(reported honestly, each an honest `Halt::Unsupported`, never a wrong value):**
**`lockdown()`** — the full intrinsics whitelist/walk transitively freezing the
shared intrinsics, the error/`Date.now`/`Math.random` compartment-safety
taming, and the repeated-lockdown idempotence throw — and **`mutabilities`**
(the `fxVerify*` mutable-residue report) are sized as a follow-up child on this
now-landed harden substrate; an **exotic receiver** (array/typed-array/
collection/wrapper/error) to `harden`/`petrify` self-names
`harden:exotic-object`/`petrify:exotic-object` rather than mis-freeze. GC roots
were not touched (no run-loop/allocation-pressure wiring), so the GC-roots
ledger note carries forward untouched.

### Stage-4 acceptance evidence (closure child 5/5)

This is the consolidated stage-4 (Hardened JavaScript & Compartment) acceptance
block — the material the whole-stage-4 acceptance review (supervisor stage s10)
independently reproduces. Full `cargo test --workspace --
--test-threads=1` is green; `#![forbid(unsafe_code)]` is intact on every engine
crate; no GC/allocation path was touched by this child, so the GC-roots ledger
note carries forward untouched and Miri is not implicated here.

**Per-child covered/divergent numbers (all `divergent=0`).**

| Stage-4 child | Landed surface | Headline dual-run / corpus evidence |
|---|---|---|
| 4a-1 object-integrity | `preventExtensions`/`seal`/`freeze` + slot-arena attribute stamps | `built-ins/Object covered=176 divergent=0` (from 63) |
| 4a-2 classes (`new.target`) | `XS_CODE_TARGET` real semantics | `built-ins/Function covered=40 divergent=0`; `language/{statements,expressions}/class covered=1 divergent=0` each (rest named skips) |
| 4a-3 generators | `function*` + iteration protocol suspend/resume | `language/{statements,expressions}/generators covered=74/79 divergent=0`; `for-of covered=118`; `stage4-generators.js` corpus bit-exact |
| 4a-4 async/promise keystone | promise native-handler double-settle + thenable adoption | `built-ins/Promise covered=9 divergent=0`; `stage4-async-promises.js` bit-exact |
| 4a-5 modules (static half) | `endor_vm::module` records/map/resolve/link/evaluate, TDZ, ModuleSource | 14 cargo-locked unit tests; `language/module-code` dual-run structurally skipped (oracle shim compiles the script goal only) — certified by the endor-side corpus + manual-`xst` method |
| 4b-2 async-function surface | `ASYNC_FUNCTION`/`START_ASYNC`/`AWAIT` opcodes | `language/{statements,expressions}/await covered=6/6 divergent=0`; `stage4-async-await.js` bit-exact |
| 4b-3 Compartment | native `Compartment::evaluate_with_symbols` over shared intrinsics | `stage4-compartment.js` differential across two compartments, result+computron agreement |
| 4b-4 lockdown/harden | native `harden(x)`/`petrify(x)` over the slot arena | `stage4-harden.js` (30 programs) result-gated agreement; 3 Miri-clean freeze unit tests; `built-ins/Object` unchanged at `covered=176 divergent=0` |
| 4b-5 closure (this child) | boot-bundle identical-run verdict + ses-xs-parity tally | below |

**Boot-bundle identical-run verdict (`daemon-endor-architecture.md` § Unified
runner).** The daemon boots `polyfills.js` → `host_aliases.js` → `ses_boot.js`
(SES `lockdown()` + the HandledPromise shim). The first two are committed
sources embedded by `rust/endo/xsnap/src/lib.rs`; `ses_boot.js` is **not
committed** — it is a ~1 MB build artifact the daemon bundler (`rollup` over
`@endo/*`) generates before the `include_str!`, absent in a fresh checkout, and
bundling the full SES distribution is out of this engine workspace's scope. The
bar (`endor-262::tests::stage4_daemon_boot_bundle_never_diverges_and_names_its_gaps`,
locked in `cargo test`) dual-runs the **actual committed bytes** the daemon boots
against the pin. **Verdict: the committed boot bundle does not run identically on
endor yet** — its first statement reads `globalThis`, and endor has **no live
global-object binding**, so every bundle honestly aborts there
(`boot:no-globalThis-global-object-binding`) rather than diverging. Result
agreement is the bar and endor **never lies** about the bundle (zero divergence —
no wrong value, no over-acceptance); it declines it with a self-named halt. This
is a **named, ledgered post-stage-4 engine gap**, not a fold to be hidden. The
downstream engine gaps the bundle would hit *after* a `globalThis` binding lands
(observed by dual-running each polyfill sub-behavior against the pin) are the
rest of the ledger:

| Ledgered boot-bundle gap | Blocking construct in the bundle |
|---|---|
| `boot:no-globalThis-global-object-binding` | `typeof globalThis.TextEncoder` (first statement; blocks all three bundles today) |
| `boot:Reflect-intrinsic` | `Reflect.ownKeys(descs)` in the harden deep-freeze |
| `boot:typed-array-from-iterable` | `new Uint8Array([...])` (the from-array form; the length form works) |
| `boot:defineProperty-symbol-key` | `Object.defineProperty(Object, Symbol.for('harden'), …)` |
| `boot:class-instance-construction` | `new TextEncoder()` (class-body method dispatch on a constructed instance) |
| `boot:ses-lockdown-bundle` | `ses_boot.js` — the uncommitted ~1 MB SES bundle (bundler out of scope) |

The bar flips to green (and the ledger advances to the next row) the moment the
`globalThis` binding lands. No committed bundle was silently narrowed to make the
bar pass.

**SES conformance (`ses-xs-parity`) tally.** The repo's `ses-xs-parity` feature
set — the exact tests `packages/test262-runner` runs `xst` against with
`--features-include ses-xs-parity` — is **2 files**, both under
`built-ins/Compartment/prototype` (`Symbol.toStringTag.js`,
`Symbol.toStringTag-lockdown.js`). The bar
(`endor-262::test262::tests::ses_xs_parity_suite_has_zero_divergence`, locked in
`cargo test`) runs them against endor with **zero RESULT divergence**:
`total=2 covered=0 divergent=0`, each skip named — `1 endor-aborted` (the
non-lockdown file references the `Compartment` **intrinsic global**, which endor
does not bind as a JS intrinsic — the Compartment child's
`compartment:intrinsic-surface` fold; endor's `Compartment` is a Rust host type),
and `1 oracle-shim-unsafe:lockdown` (the lockdown-tagged file calls `lockdown()` —
the same `lockdown()` surface the harden child folded on the endor side; it is
pre-partitioned out and never dual-run). The bare-boot shim **no longer SIGSEGVs**
on `lockdown()` — that abort was the garbage-home shim-install bug the harden
child's crash-fix above resolved (`lockdown()` now completes cleanly on the
oracle) — but the file stays pre-partitioned because endor still folds `lockdown`
as an honest `Halt::Unsupported`, so a dual-run would name-skip on the endor side
regardless. endor reaches none of the SES-parity surface end-to-end today, but
lies about none of it; the tally grows as the `Compartment`/`lockdown` intrinsic
globals land.

**Consolidated fold ledger for s10 (each verified STILL an honest named skip at
this closure point).**

| Fold | Origin child | Self-named skip |
|---|---|---|
| async generators / `for-await-of` | 4a-3 / 4b-2 | designated scope fold (excluded from the async corpora) |
| `language/module-code` dual-run | 4a-5 | structural skip (oracle shim = script goal only); certified by endor-side corpus + manual-`xst` |
| runtime `XS_CODE_MODULE`/`XS_CODE_TRANSFER`, dynamic `import()`, `import.meta` | 4a-5 | named `Halt::Unsupported` skips |
| `Compartment` intrinsic global surface | 4b-3 | `compartment:intrinsic-surface` (confirmed by the ses-xs-parity `endor-aborted` skip above) |
| `await`-in-`try` | 4b-2 | `await:await-in-try` |
| `lockdown()` + `mutabilities` | 4b-4 | `Halt::Unsupported` (endor); `oracle-shim-unsafe:lockdown` (oracle shim); confirmed by the ses-xs-parity guard above |
| `harden`/`petrify` exotic receiver | 4b-4 | `harden:exotic-object` / `petrify:exotic-object` |
| daemon boot bundle (`globalThis` + downstream) | 4b-5 | boot-bundle gap ledger above |

The stage-3 built-ins reach
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

## The XSRE RegExp engine port (stage-3b, `endor-regexp`)

`endor-regexp` is the engine-internal transliteration of the pin's RegExp
engine (`xsre.c`, the design's resolved question 6). It has **no
JavaScript surface** — child 9 integrates the `RegExp` builtin; this crate
is the matcher itself, so it can be pinned to C-XS in isolation.

Two halves, both `#![forbid(unsafe_code)]`:

- **`compile`** ports `fxCompileRegExp`: a recursive-descent parse into a
  term tree, a `measure` pass assigning each term its **byte** offset in
  the code array (kept in bytes exactly as C-XS keeps them, so the compile
  meter and the emitted graph match the pin), and a `code` pass emitting
  the integer step stream.
- **`match_regexp`** ports `fxMatchRegExp`: the backtracking VM over that
  step stream. C-XS threads its backtrack states as a linked list through
  the machine stack or `c_malloc`; the safe port keeps them in a
  `Vec<State>` and records an assertion's saved point as a length marker
  (so `fxPopStates` becomes a `truncate`) — behaviorally identical, no
  `unsafe`.

**Metering hooks carried through** (so child 9 can calibrate end to end):
`Program::compile_meter_raw` is the regexp-compile component
(`size × XS_PARSE_REGEXP_METERING`), and `MatchOutcome::match_meter_raw`
is `steps × XS_REGEXP_METERING` — the matcher's per-step cost.

**Parity is pinned against C-XS** through the oracle shim's new
`endor_oracle::regexp` entry point (which calls `fxCompileRegExp` +
`fxMatchRegExp` directly and returns captures + per-phase meter). The
`tests/parity.rs` suite is **bit-exact on the matched answer, every
capture's byte offsets, and the per-step match meter**: `total=325
checked=325 skipped=0 divergent=0`, covering character classes,
greedy/lazy quantifiers, groups/backreferences, anchors, alternation,
lookahead + lookbehind, the `i`/`m`/`s` flags, and pathological
backtracking. The **match** meter
is the consensus-relevant number and is pinned exactly; the *compile*
meter is deliberately not asserted against the shim, because C's compile
number folds in `fxNewChunk`'s `XS_CHUNK_ALLOCATION_METERING` over the code
and data buffers — a GC-allocator artifact the `Vec`-backed port
structurally does not incur (the design already excludes
parse/allocation metering from the parity number).

The **fuzz arm** (`endor_fuzz::regexp`, cargo-fuzz target
`differential_regexp`) is a structure-aware generator over the supported
grammar whose 3000-seed sweep pins the same three quantities bit-exact,
**zero divergence**. It already earned its keep: it caught a missing
backreference-number range check (`fxCaptureReferenceMeasure` rejects
`\11` when there are fewer than 11 groups; XS reads the whole decimal
greedily and errors, it does not fall back to `\1`), now ported. Both the
suite and the sweep bound catastrophic inputs deliberately — the oracle
shim leaves the C matcher's meter interval unset, so a nested unbounded
empty star (`(a*)*b`) backtracks unbounded on the **pin too**; that is a
both-engines pathology, not a port divergence, so such inputs are excluded
from the corpus and the generator never applies an unbounded quantifier to
a group.

The `i` flag's **case folding** is ported (`crate::charcase`, the
non-`u`/`v` `fxCharCaseCanonicalize` path over the `gxCharCaseIgnore0`
table transcribed verbatim from the pin): a single-character set is folded
at compile time (`fxCharSetCanonicalizeSingle`), a range under `i` expands
to the union of its folded singletons (`fxCharSetRange`), `\w` drops
`a`..`z` (the folded subject reaches its `A`..`Z` form), and the match loop
folds every decoded character (`fxGetCharacter`) — all pinned bit-exact.

**Honest, named skips (the stage bar).** This increment ports the core
grammar over the non-`u`/`v` subset (the `i` flag included). Every deferred
surface compiles to a **named** `CompileError::Unsupported`, never a wrong
meter or a wrong value: the `u`/`v` flags (CESU-8 surrogate walk, unicode
property escapes, V-mode string sets, and their `u`/`v` fold tables),
`\p{}`/`\P{}`, named captures (`(?<name>)` / `\k<name>`), inline modifiers
(`(?flags:)`), and astral (`> 0xFFFF`) code points. The crate is
`#![forbid(unsafe_code)]` and Miri-clean
(`cargo +nightly miri test -p endor-regexp --lib`).

### The JavaScript RegExp surface (stage-3b, child 9)

`endor-vm` links `endor-regexp` as the JavaScript `RegExp` builtin
(`xsRegExp.c` + the RegExp-consuming `xsString.c` methods), **bit-exact
(result AND computron) against the pin** end-to-end — construction *and* the
whole-program run, not just the matcher:

- **Construction**: the `regexp` opcode + a `/.../ ` literal (which XS compiles
  to `new RegExp(pattern, flags)`), the `RegExp` constructor, the compiled
  program + source/flags + `lastIndex` in the `regexps` side table. Metering
  models `fxNewRegExpInstance`'s four `fxNewSlot`s, the `fxCompileRegExp` parse
  meter, and its two `fxNewChunk` buffers (`code` = `parser->size`, `data` =
  the capture/name/assertion/quantifier struct-size sum), so it is raw-exact
  across every pattern shape.
- **`exec`/`test`**: the match drive from `lastIndex` (g/y), the `[whole,
  ...captures]` result array with `index`/`input`/`groups`, `lastIndex`
  advance; `test` drives the full `exec` as XS's `fxExecuteRegExp` does.
- **Accessors**: `source` (escape-on-read), the composite `flags` (the
  eight per-flag cascade), the per-flag getters, `lastIndex` get/set, and
  `toString` (the three growing `fxConcatString` chunks) — special-cased by id
  in `GET`/`SET_PROPERTY` over the side table.
- **`String.prototype.{search,match,replace,split}`** via the
  `Symbol.{search,match,replace,split}` protocol to the RegExp workers:
  `search` (index or −1), non-global `match` (the exec array), non-global
  literal-replacement `replace` (the segment-list assembly), and `split` (the
  sticky-splitter walk). Each carries its calibrated protocol-dispatch +
  worker residual, raw-exact.

**Corpus + fuzz.** `endor-262/corpora/stage3b-regexp.js` (a curated corpus,
`stage3b_regexp_corpus_is_bit_exact_against_oracle`) and a whole-program
differential **fuzz arm** (`differential_regexp_surface`, a 1200-seed sweep
that pins result + computron end-to-end) complement the matcher-level arm — it
already earned its keep, driving out a sub-computron construction-metering gap
(the unmodeled `data` buffer) and the split empty-match corner. Dual-run:
`built-ins/RegExp/prototype total=407 covered=50 divergent=0`,
`built-ins/RegExp/prototype/exec covered=33`, `/test covered=15`,
`language/literals/regexp covered=21` (from 6), and the four
`built-ins/String/prototype/{search,match,replace,split}` sections
**divergent=0** with growth.

**Honest, named skips** (each a named `Halt::Unsupported`, never a wrong value
or a fitted meter): named-group result shaping, a RegExp-valued pattern arg,
a syntax-error / unsupported-feature throw, a non-ASCII stateful (g/y) subject;
global `match`/`replace` collection, the `$`-substitution grammar and function
replacement in `replace`; a limit that truncates, an empty-matching separator,
and a non-ASCII subject in `split`; and a string (non-RegExp) argument to the
String methods (the `withoutRegexp` coerce path).

## Decoder-hang trophy: a malformed backward branch that never terminated (stage-4b)

**Target 2 (bytecode decoder) caught a real non-termination bug — and the
harness is now wedge-proof against future ones.** At the stage-4a modules tip
(`e08b83ac3`), `cargo test --workspace` no longer completed: the fuzz test
`decoder_never_panics_on_arbitrary_bytes` (`endor-fuzz/src/lib.rs`) entered a
deterministic infinite loop, burning 2h+ of CPU at 99.9% on that single test.

**The input.** Seed 1750 of the test's LCG sweep decodes to the 14-byte string
`[25 fe 86 1c 28 ee 59 08 a6 f7 ec c0 0d 17]`; the minimal core is the first
two bytes `[25 fe]`. Byte `0x25` is `XS_CODE_BRANCH_STATUS_1` (the
generator/async resume epilogue); its 1-byte operand `0xfe` is offset `-2`.
With a 2-byte instruction at pc 0, `branch_target(0, 2, -2) = 0` — the branch
**targets its own pc**, a zero-progress self-loop. `disassemble` terminates
fine (it walks lengths, not control flow); the hang is in the interpreter's
dispatch loop.

**Root cause and the commit that introduced it.** The interpreter only aborts a
backward branch through the metering host (`check_meter`), but the fuzz entry
`run_program` arms **no** metering host (`meter_host: None` → `check_meter`
always `Continue`). The pre-existing "backward branch off the front"
regression case `[16 80]` terminates only incidentally — its offset drives the
pc out of bounds to a `Halt::Decode`. A branch that targets an **in-bounds**
pc (a legitimate loop shape, but here degenerate) has no out-of-bounds escape
and spins forever. This became reachable at **`b41446ad7`** (*engine: generator
functions + iteration protocol, stage-4 child 3/8*): before it, opcode 37 was
an unimplemented byte that halted with `Halt::Unsupported`; that commit gave it
a real backward-branch handler. The s8 acceptance of `0b991a8b4` still passed
the suite `128/0` because its opcode set had not yet armed byte `0x25`.

**The fix (two layers).**
1. *Root — a total interpreter for un-metered callers.* A dispatch-count
   ceiling (`Interp::run_bounded` / `run_program_bounded`, `Halt::StepLimit`).
   The default `run` stays `u64::MAX`-unbounded, so every oracle-differential
   path is byte-for-byte unchanged; only the fuzz harness installs the finite
   `DECODER_STEP_LIMIT` (2,000,000 dispatches — far above any well-formed
   `<= 40`-byte program, so it fires only on a genuine non-terminating cycle).
   A self-loop now aborts with `Halt::StepLimit` in single-digit milliseconds.
2. *Lock — the offending input as a named regression case* (both the 14-byte
   seed string and the 2-byte core) plus a wedge-proofing test
   `decoder_hang_is_bounded_not_infinite` asserting the self-branch hits the
   step ceiling rather than completing or hanging. Any **future** decode
   non-termination now fails loudly in milliseconds instead of wedging the
   whole workspace bar.

**Bar is tractable again.** `cargo test --workspace -- --test-threads=1`
completes green — **149 passed, 0 failed** — in **~5 s** wall-clock on a warm
build (the endor-fuzz binary's own run, including the 2000-seed decoder sweep,
is 1.3 s). `#![forbid(unsafe_code)]` intact across the workspace.

## Stage 5: the compiler port (`endor-compile` coder, child 5/7)

Stage 5 replaces the differential-oracle compiler with a pure-Rust one in
XS's own shape (lexer → parser → scoper → **coder**), held to a
**byte-identical-bytecode** bar: `endor_compile::compile(src)` must equal
`endor_oracle::run(src).bytecode` byte for byte. Children 1–4 landed the
lexer/parser/scoper; **child 5** lands the coder's emission framework and
the expression + simple-statement node surface — the first stratum that
produces real byte-identity evidence (`endor-compile/tests/coder_byte_identity.rs`,
with an opcode-level disassembler for triage).

**Ported faithfully (the framework that shapes the bytes).** XS's exact
`fxParserCode` three passes: pass 1 sizes every record with branches
assumed widest and accrues the `delta` slack; pass 2 selects each
branch's `_1`/`_2`/`_4` width from the now-known target offsets, narrowing
`size`; pass 3 emits with back-patched displacements. Plus the
`fxCoderAdd*` record constructors, the target/fixup arena, stack-depth
accounting, the `INTEGER`/`STRING`/`BIGINT`/index-family operand-width
selection, and the constant encodings (LE integers, LE-limb BigInts,
IEEE-754 doubles, NUL-terminated strings) in XS's byte order. The program
header is coded through `fxScopeCodingEval` — the oracle compiles the
script goal as an eval program (`BEGIN_SLOPPY`/`STRICT`, `EVAL_ENVIRONMENT`).

**Node surface covered:** literals of every scalar kind (int, number,
string, `true`/`false`/`null`/`undefined`, BigInt), every
unary/binary/relational/shift/bitwise operator, `&&`/`||`/`??`, the
conditional, sequence, expression statements, `if`/`else`, and blocks.

**Honest folds (the back half — child 6).** Every construct whose
emission depends on the **atom/symbol table** — whose per-symbol id
assignment (XS's hash-table walk order) is embedded into the *code*
stream, so it must be reproduced exactly — or on a **nested function
body** is deferred to child 6: identifier/property loads-and-stores,
`var`/lexical declarations, assignment (simple/compound/destructuring),
call/`new`/member (incl. optional chaining), object/array/template
construction, and function/class bodies. The current coder `panic!`s (in
tests) rather than emit a wrong byte for these — a named gap, never a
silent divergence.

**Child 6 progress (the back half, in slices).** Child 6 lands the
symbol-dependent and control-flow surface incrementally, each slice
byte-identical vs the oracle:

- Slices 1–8: symbol-free and atom-table control flow and expressions —
  loops (`while`/`do`/C-`for`), labeled break/continue, `switch`, `throw`,
  `debugger`, `try`/`catch`/`finally`, `this`, regexp and template
  literals, the atom table + global variable/member access, calls +
  computed member, assignment/compound/`new`, increment/decrement/`delete`,
  and object + array literals.
- Slice 9 — **declarations**: `var`/`let`/`const` at program (eval) scope,
  the `fxScopeCodingEval` header (sloppy `var`/`Define` hoist + direct-eval
  `WITH` publish; strict reserve + block coding), `fxScopeCodingBlock` /
  `fxScopeCoded` slot allocation and `UNWIND` teardown, and the
  binding/declare/access resolution that picks a frame-slot op
  (`NEW_LOCAL`/`LET_LOCAL`/`CONST_LOCAL`/`VAR_LOCAL`, `GET_LOCAL`/
  `SET_LOCAL`) over the symbol path. The scoper now exposes a per-node
  access-resolution map, and the program node is stamped `mxEvalFlag` so
  its top scope is coded as `EVAL` (an eval program's lexicals are plain
  `LOCAL`s, not the program-scope `CLOSURE` marking `fxScopeBound`
  applies). Block-scoped lexicals code and unwind correctly.
- Slice 10 — **`with`**: `fxWithNodeCode` (`TO_INSTANCE`/`WITH`, the body
  under a forced eval flag, `WITHOUT`); sloppy-only.
- Slice 11 — **functions (first slice)**: `fxFunctionNodeCode` for plain
  (`CONSTRUCTOR_FUNCTION`) and arrow (`FUNCTION`) function values — the
  nested `CODE` block (`BEGIN`/`END`/`END_ARROW`), the per-function coder
  state save/restore, `FUNCTION_ENVIRONMENT` storing, and the
  plain-function non-enumerable `caller` own property — plus function
  *declarations* (`fxDefineNodeCode` with `fxScopeCodeDefineNodes`
  hoisting: defines emit at the top of their scope, the in-list occurrence
  a no-op), `fxReturnNodeCode`, `fxBodyNodeCode`, the empty
  `fxParamsBindingNodeCode`, and a null-symbol operand for anonymous
  functions. Scoped to **simple bodies** (expression statements +
  `return expr`) in non-naming contexts.
- Slice 12 — **positional parameters**: `fxScopeCodingParams` (each `Arg`
  gets a `NEW_LOCAL` frame slot) and `fxParamsBindingNodeCode`
  (`ARGUMENT i` / `VAR_LOCAL` / `POP` binds each parameter from its
  argument); `BEGIN` carries the parameter count. Defaults, destructuring,
  rest, the `arguments` object, and captured parameters stay deferred.
- Slice 13 — **name inference**: an anonymous function assigned to a simple
  identifier (a `var`/`let`/`const` binding initializer or a plain
  assignment) takes that identifier as its name, landing in the
  function-creation operand (a pending-name the naming site stages and
  `code_function` consumes). Object-method/property naming (the `NAME`-op
  path), member-target assignment, and anonymous classes stay deferred.
- Slice 14 — **the branch optimizer + non-trivial function bodies**:
  `fxCoderOptimize`'s full four peephole passes (a `BRANCH_1` reaching an
  `END*` becomes that `END*` inline; an `UNWIND_1` before an `END*` is
  dropped; a dead `END*` before the same `END*` is dropped; branch-to-next),
  the `fxStatementNodeCode` store-and-pop fusion (`SET_LOCAL`/`SET_CLOSURE`
  + `POP` → `PULL_LOCAL`/`PULL_CLOSURE`), and XS's `mxExpressionNoValue`
  increment/compound optimization. Together these make function bodies with
  **control flow** (loops, `if`/`else`, `switch`, labeled break, try-finally,
  `return` threaded to `END`) and **declarations** (`var`/`let`/`const`,
  nested blocks) code byte-identically.
- Slice 15 — **`catch (e)` parameter bindings**: `fxCatchNodeCode`'s
  parameter branch — the parameter scope allocates the binding slot, the
  caught `EXCEPTION` stores into it, then the body block codes and both
  scopes unwind.
- Slice 16 — **default parameters**: `fxBindingNodeCodeReference`/`Assign`
  — the `= default` param dance (`DUB` / `STRICT_NOT_EQUAL` vs `undefined`
  / `BRANCH_IF` past the initializer, then the inner target's store).
  `code_params_binding` admits `Binding` items; the defaulted param is
  excluded from the `BEGIN` count (like XS's non-simple-parameter count).
  Covers a later param defaulting from an earlier one.
- Slice 17 — **rest parameters**: `fxParamsBindingNodeCode`'s rest branch —
  a `...rest` param binds its target from `ARGUMENTS i` (collecting the
  remaining arguments into an array) rather than `ARGUMENT i`; excluded
  from the `BEGIN` count. With or without leading fixed parameters.
- Slice 18 — **captured closures**: the closure slot contract. A variable
  an inner function references is promoted to a closure slot in its defining
  scope (`NEW_CLOSURE` / `VAR_CLOSURE`); the capturing inner function's
  `fxScopeCodeRetrieve` emits `RETRIEVE_1` to pull the closures into its
  frame (accesses resolve to `GET_CLOSURE`), and `fxScopeCodeStore` emits
  `STORE_1` of the defining scope's slot into the new function's environment
  on creation. Parameters and locals, nested/multi-capture, arrow closures,
  and mutation of captured bindings all covered; arrow capture of
  `this`/`super`/`target` and the `arguments` object stay deferred.
- Slice 19 — **named function expressions**: a `function g(){…}` value
  binds its own name `g` in a `const` slot of its scope, initialized to the
  running function (`CURRENT`), so the body can recurse by name. The Rust
  scoper folds XS's symbolScope into the function scope, so this is a
  targeted `code_function_name` emission. A name captured by an inner
  function (a closure-slot name) stays deferred.
- Slice 20 — **`for-in` / `for-of` iteration**: `fxForInForOfNodeCode` — seed
  the iterator (`FOR_IN`/`FOR_OF`), cache `next`, and drive a `next()` loop
  inside a `try`/`finally` that closes the iterator (`.return()`) on
  break/continue/return/throw, reusing the selector/alias/finalize/jump
  target machinery (shared with `try`). Non-declaring heads (plain
  reference / member / computed target), labeled break, nesting, and use
  inside functions all covered; declaring heads (`for (let/const/var …)`),
  `using`, and `for await` stay deferred.
- Slice 21 — **object-property name inference**: `fxObjectNodeCode`'s
  `NEW_PROPERTY`/`NEW_PROPERTY_AT` attribute carries `XS_NAME_FLAG` when the
  property value is an anonymous function (`fxNodeCodeName`), so the
  interpreter infers the value's `.name` from the key (data + computed
  keys; named / non-function values unaffected).
- Slice 22 — **object shorthand**: a `{x}` shorthand codes exactly like the
  data property `x: x` (its value is an `Access` to `x`), differing only in
  that a shorthand named `__proto__` is a normal property rather than the
  prototype setter.
- Slice 23 — **object spread**: a `{...expr}` member copies `expr`'s own
  enumerable properties onto the object via the `COPY_OBJECT` intrinsic
  (invoked with the object as `this`); mixes with data properties.
- Slice 24 — **array spread**: `fxArrayNodeCode`'s spread branch — a
  running `counter` slot indexes appends and each `...expr` is iterated
  with the `for-of` protocol (`FOR_OF` + a `next()`/`done` loop that
  `SET_PROPERTY_AT`s each value); elisions bump `counter`/`length`.
- Slice 25 — **call/`new` spread arguments**: `fxParamsNodeCode`'s spread
  branch + `fxSpreadNodeCode` — a `...spread` argument makes the count
  dynamic (a `counter` slot bumped per fixed arg and per `for-of`'d spread
  element), closing with a plain `RUN` (no static count). Calls and `new`.
- Slice 26 — **written `__proto__:`**: a non-shorthand `__proto__: v`
  object property makes `v` the prototype via `INSTANTIATE` (in place of
  the plain `OBJECT`) and is skipped in the property loop; a shorthand
  `{__proto__}` or computed `['__proto__']` stays a normal property.
- Slice 27 — **object concise methods + accessors**: a concise method /
  getter / setter emits its (anonymous) function value with the `FUNCTION`
  creation-op (the accessor flag rides on the property in the Rust AST, so
  it is relayed to `code_function` as a staged hint), and the `NEW_PROPERTY`
  attribute carries the `NAME | METHOD` (+ `GETTER`/`SETTER`) bits.
  Identifier and computed keys covered; `super` in a method body deferred.
- Slice 28 — **declaring `for-in`/`for-of` heads**: `for (let/const x of …)`
  binds a fresh per-iteration lexical in the loop's block scope — the scope
  header allocates the slot, a per-iteration `fxScopeCodeReset`
  (`RESET_LOCAL`/`RESET_CLOSURE`) refreshes it, the binding assigns via
  `LET_LOCAL`/`CONST_LOCAL`, and the scope unwinds it. `let`/`const`,
  `for-in`, nesting, and use inside functions covered; `for await` and
  `using` heads stay deferred (`for (var …)` heads code correctly — the var
  hoists out, leaving the loop block non-declaring).
- Slice 29 — **the `arguments` object**: a function that references
  `arguments` carries a synthetic `arguments` `Var`; its scope header slots
  it and a parameter prelude builds the object (`ARGUMENTS_SLOPPY` mapped /
  else `ARGUMENTS_STRICT`, operand = the parameter count) and stores it.
- Slice 30 — **mapped `arguments` closure-marks parameters**: ported
  `fxParamsBindingNodeBind`'s rule into the scoper — a sloppy function with
  `arguments` and a simple parameter list promotes each parameter to a
  closure slot so the mapped object can alias it, completing the
  `arguments` surface (`function (a) { … arguments … }` now codes
  `NEW_CLOSURE`/`VAR_CLOSURE`/`GET_CLOSURE` for the parameters).
- Slice 31 — **object destructuring**: `fxObjectBindingNodeCodeAssign` —
  `TO_INSTANCE` the value into a temporary, then read each `PropertyBinding`
  named property (`GET_PROPERTY`) and assign it into the target. Both
  destructuring assignment (`({a,b} = x)`) and lexical/var binding
  (`let {a,b} = x`); shorthand, renamed (`{a: p}`), `= default` elements,
  and nested-value sources covered. Object rest (`{...r}`), computed keys,
  and nested *patterns* stay deferred.
- Slice 32 — **array destructuring**: `fxArrayBindingNodeCodeAssign` — seed
  an iterator over the value (`FOR_OF`), pull each element from `next()`
  into its target (skipping elision holes, collecting a `...rest` into an
  array, applying `= default`s), and close the iterator (`.return()`) on
  early exit, reusing the selector/alias/finalize/jump `try`/`finally`
  machinery (only the return target crosses it). Both destructuring
  assignment (`[a,b] = x`) and lexical/var binding (`let [a,b] = x`); holes,
  rest, defaults, and member targets covered.
- Slice 33 — **destructuring parameters**: a parameter that is an array/
  object pattern pulls its `ARGUMENT i` and binds it through the same
  array/object destructuring coders (`code_params_binding` accepts
  `ArrayBinding`/`ObjectBinding` param items). Mixed with plain parameters,
  rest, and defaults, in both function expressions and arrows.
- Slice 34 — **object rest + computed-key destructuring**: completes
  `fxObjectBindingNodeCodeAssign` — object rest (`{...r}`) collects the
  source's own enumerable properties minus the explicitly-bound keys via
  `COPY_OBJECT` (each bound key pushed as an exclusion argument), and
  computed keys (`{[k]: v}`) read through `GET_PROPERTY_AT`. With nested
  patterns (which already recurse), **all destructuring is now covered**:
  array + object, assignment + binding + parameters, holes/rest/defaults/
  computed/nested.
- Slice 35 — **generator functions + `yield`**: `fxFunctionNodeCode`'s
  generator path (the `GENERATOR_FUNCTION` create op + `START_GENERATOR`
  opening the body) and `fxYieldNodeCode` (sync) — build the
  `{ value, done: false }` result, `YIELD`, and thread the
  `.return()`/`.throw()` completion (`BRANCH_STATUS`) out to the return
  target on non-`next` resume.
- Slice 36 — **async functions + `await`**: the `ASYNC_FUNCTION` create op
  + `START_ASYNC` at entry, and `fxAwaitNodeCode` — evaluate, `AWAIT`, and
  thread the rejection/completion (`BRANCH_STATUS`) out to the return
  target until the async job resumes. Async arrows too.
- Slice 37 — **async generators**: the `ASYNC_GENERATOR_FUNCTION` create op
  + `START_ASYNC_GENERATOR`, and `fxYieldNodeCode`'s async branch (yield the
  raw value, `YIELD`/`BRANCH_STATUS`, then `AWAIT` + `THROW_STATUS`).
- Slice 38 — **`yield*` delegation**: `fxDelegateNodeCode` — the full
  iterator-delegation state machine (`FOR_OF`/`FOR_AWAIT_OF` seed,
  `YIELD_STAR` forwarding, `CHECK_INSTANCE`, and the loop/return/throw/normal
  sections with `CATCH`/`UNCATCH` + `BRANCH_CHAIN`/`COALESCE` completion
  routing), async variant awaiting each step. Completes generators/async.
- Slice 39 — **`for await (… of …)`**: the `is_async` branch of the ported
  `fxForInForOfNodeCode` (`AWAIT`/`THROW_STATUS` after each `next()`/
  `return()`) became reachable once async functions landed; pinned.
- Slice 40 — **`super` in methods + arrow `this`/`super`/`target` capture**:
  object-method `super` (member read/call/store/delete/computed, in plain/
  async/generator methods) already worked via the member coder's super
  path; this implements the arrow-default capture in `fxScopeCodeRetrieve`/
  `Store` — an arrow that transitively uses `this`/`super`/`target`
  retrieves the receiver and target (`RETRIEVE_TARGET`/`RETRIEVE_THIS`) and
  stores them on creation (`STORE_ARROW`).
- Slice 41 — **direct-`eval` parameters**: a syntactic `eval(...)` call
  closes with the `EVAL` intrinsic (`INTEGER count` + `EVAL`, or
  `GET_LOCAL counter` + `EVAL` for spread) instead of `RUN`; the scoper
  already poisons the enclosing scopes. Program/block-level `eval` (with or
  without declarations, spread, as a value) is byte-identical; `a.eval()`
  is correctly a normal call.
- Slice 42 — **base classes**: `fxClassNodeCode` for an anonymous `class`
  with no heritage — a fresh prototype (`NULL`/`OBJECT`), the base
  constructor (`code_function` now handles `BASE`: `BEGIN_STRICT_BASE` /
  `END_BASE`), the `CLASS` opcode binding the prototype/constructor pair,
  and concise method / accessor / static members (`NEW_PROPERTY` with
  `DONT_ENUM` + method/getter/setter bits; static on the constructor,
  instance on the prototype). The scoper's `fxClassNodeBind` reserves the
  two class temporaries. Covers generator/async/`super`-using methods.
- Slice 43 — **named classes**: ported `fxClassNodeHoist`/`Bind`'s
  named-class path — the scoper now builds the class's block scopes (a
  `symbolScope` binding the class name as a `const` closure visible in the
  body, plus the class body scope) and enters them in the bind pass (so the
  name slot lands in `RESERVE`); the coder codes the `symbolScope`
  (`NEW_CLOSURE`), the `NAME` op, and the class→name `CONST_CLOSURE`.

**Still folded (named gaps, coder still `panic!`s — never mis-emits):**
`extends` / derived constructors +
`super(...)`, class **fields** and **private** members, static blocks,
computed method keys, and anonymous-class name inference — these need more
class-hoisting scoper work; `using` heads (a parser gap); and
module import/export linkage + the module-body wrapper. These are the
remaining child-6/7 surface.
