// Initialization-order test for the genuine-Uint8Array brand check.
//
// erights asked on PR #475 (review comment r3496732452, replying to
// r3496724676): "If this module initializes after the immutable ArrayBuffer
// shim initializes, won't it get the getter that the shim installed, which
// also admits emulated Uint8Arrays?"
//
// The concern: `genuine-uint8-array.js` captures the
// `%TypedArray%.prototype[Symbol.toStringTag]` getter at module-load time. If
// the `@endo/immutable-arraybuffer` shim ran first and replaced that getter
// with one that answers `'Uint8Array'` for its emulated frozen wrappers, then
// the captured getter would admit a counterfeit and `assertGenuineUint8Array`
// would wave it through.
//
// This test reproduces exactly that ordering. The `import` of the shim is
// written ABOVE the `import` of the module under test, so the shim's module
// body runs first and `genuine-uint8-array.js` captures its getter strictly
// afterward. (Under the `lockdown` and `unsafe` ava configs the same ordering
// already holds for a second reason: `ses/lockdown.js` itself imports
// `@endo/immutable-arraybuffer/shim.js`, so the shim is installed before any
// test module loads. The explicit import here makes the test exercise a real
// emulated wrapper under the shims-only config too, and documents the ordering
// the test is about.)
//
// The conclusion the test pins down: the shim installs its property record onto
// `%TypedArrayPrototype%` but that record contains no `Symbol.toStringTag`
// entry, so the genuine getter survives. Applied to a real shim-produced
// emulated frozen Uint8Array it still returns `undefined` (the wrapper has no
// `[[TypedArrayName]]` internal slot), so the brand check rejects it. erights's
// worry does not materialize, and this test stands as the regression guard if a
// future shim change ever does shadow that getter.

import '@endo/immutable-arraybuffer/shim.js';
import test from '@endo/ses-ava/test.js';

import { assertGenuineUint8Array } from '../src/genuine-uint8-array.js';

const { getPrototypeOf, getOwnPropertyDescriptor } = Object;
const { apply } = Reflect;
const { toStringTag: toStringTagSymbol } = Symbol;

// Reconstruct the same getter `genuine-uint8-array.js` captures, so the test can
// observe directly what the brand check sees. Typed loosely because the test
// applies it to deliberate counterfeits whose static type is not the getter's
// nominal `this`.
const typedArrayPrototype = getPrototypeOf(Uint8Array.prototype);
const getTypedArrayToStringTag = /** @type {any} */ (
  getOwnPropertyDescriptor(typedArrayPrototype, toStringTagSymbol)
).get;

// Build a real emulated frozen Uint8Array through the shim's own path: an
// immutable ArrayBuffer, then a Uint8Array view over it. With the shim active
// the global `Uint8Array` is the pseudo-constructor, which on an immutable
// buffer produces a plain wrapper object whose `[[Prototype]]` is
// `Uint8Array.prototype` but which has no integer-indexed bytes. This is the
// genuine counterfeit erights's question is about, not a hand-rolled stand-in.
//
// Typed `any`: the value masquerades as a `Uint8Array` but its integer-indexed
// reads answer `undefined`, so the test deliberately treats it untyped.
//
/** @type {(...bytes: number[]) => any} */
const makeEmulatedFrozenUint8Array = (...bytes) => {
  const seed = new Uint8Array(bytes);
  const immutableBuffer = seed.buffer.sliceToImmutable();
  return new Uint8Array(immutableBuffer);
};

test('the immutable-arraybuffer shim is actually installed', t => {
  // Guards the premise of every assertion below: if the shim did not install,
  // `sliceToImmutable` would be absent and the test would silently exercise a
  // genuine Uint8Array instead of an emulated wrapper.
  t.is(
    typeof ArrayBuffer.prototype.sliceToImmutable,
    'function',
    'shim installs ArrayBuffer.prototype.sliceToImmutable',
  );
});

test('a real shim-emulated frozen Uint8Array is a faithful counterfeit', t => {
  const frozen = makeEmulatedFrozenUint8Array(1, 2, 3);
  // It passes the naive prototype test...
  t.true(frozen instanceof Uint8Array);
  // ...its buffer reports immutable, confirming it came from the shim path...
  t.true(frozen.buffer.immutable);
  // ...yet integer-indexed reads return `undefined` rather than the bytes,
  // which is precisely the silent-corruption hazard the brand check exists to
  // catch.
  t.is(frozen[0], undefined);
  t.is(frozen[1], undefined);
  t.is(frozen[2], undefined);
});

test('the captured toStringTag getter still rejects the emulated wrapper', t => {
  const frozen = makeEmulatedFrozenUint8Array(1, 2, 3);
  // The heart of erights's question: even though the shim ran first, the getter
  // it left in place is the genuine one, so applied to the emulated wrapper it
  // returns `undefined`, not `'Uint8Array'`. A genuine array still tags
  // correctly.
  t.is(apply(getTypedArrayToStringTag, frozen, []), undefined);
  t.is(apply(getTypedArrayToStringTag, new Uint8Array([1, 2, 3]), []), 'Uint8Array');
});

test('assertGenuineUint8Array rejects a real shim-emulated frozen Uint8Array', t => {
  const frozen = makeEmulatedFrozenUint8Array(1, 2, 3);
  t.throws(() => assertGenuineUint8Array(frozen, 'frozen'), {
    instanceOf: TypeError,
    message: /frozen must be a genuine integer-indexed Uint8Array/,
  });
  // The diagnostic names the emulated-wrapper case specifically, so a caller
  // who hits this knows to thaw the value first.
  t.throws(() => assertGenuineUint8Array(frozen), {
    message: /no integer-indexed bytes/,
  });
});

test('assertGenuineUint8Array still accepts a genuine Uint8Array under the shim', t => {
  // The shim replaces the global constructor with a pseudo-constructor; a
  // genuine (mutable, non-immutable-backed) Uint8Array built through it must
  // remain acceptable, so the guard does not over-reject.
  const genuine = new Uint8Array([1, 2, 3]);
  t.false(genuine.buffer.immutable);
  t.is(genuine[0], 1);
  t.notThrows(() => assertGenuineUint8Array(genuine));
});
