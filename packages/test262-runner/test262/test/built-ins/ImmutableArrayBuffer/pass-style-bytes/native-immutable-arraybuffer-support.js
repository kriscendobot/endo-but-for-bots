/*---
description: >
  Probe whether the host engine provides NATIVE immutable ArrayBuffer support
  (the TC39 Immutable ArrayBuffer proposal: a native sliceToImmutable plus a
  Uint8Array over an immutable buffer that is itself freezable into the native
  integer-indexed exotic byteArray shape), as opposed to the emulated path the
  @endo/immutable-arraybuffer shim installs.

  Requested by @kriskowal on PR #475
  (https://github.com/endojs/endo-but-for-bots/pull/475#pullrequestreview-4574926723):
  verify that the xst version we run supports native frozen Uint8Array and
  immutable ArrayBuffer, and flag the issue if it does not.

  As of the pinned toolchains (xst from Moddable 5.0.0, XS 17.9.1; Node.js 22),
  no engine ships native sliceToImmutable, so the shim's detect-then-skip
  install always installs the EMULATED path: every byteArray is the shim's
  plain-ordinary-object wrapper (no own indexed properties), never the native
  integer-indexed exotic. The native arm of the byteArray brand check
  (packages/pass-style/src/byteArray.js, the `ownIndexCount === length` branch)
  is therefore not exercised by this suite on any current host.

  This test asserts that the EMULATED path is active. It is a tripwire: the day
  an engine (a future XS, or a future Node) ships native immutable ArrayBuffer
  support, the shim detect-skips, the emulated marker below disappears, and this
  assertion fails. That failure is the signal to add genuine native-path
  coverage for the byteArray brand check on the newly-capable host.

  Validates XS+SES / Node.js+SES parity for the immutable-arraybuffer path
  selection.
features: [ses-xs-parity,immutable-arraybuffer,pass-style-bytes]
---*/

// After the prelude runs SES lockdown, sliceToImmutable is present on
// ArrayBuffer.prototype regardless of host: either the native engine provided
// it, or (the current reality) the @endo/immutable-arraybuffer shim installed
// it because the native one was absent.
assert(
  typeof ArrayBuffer.prototype.sliceToImmutable === 'function',
  'sliceToImmutable is a function on ArrayBuffer.prototype after lockdown'
);

// Emulated-marker detection. The shim's emulated immutable buffers carry an own
// [Symbol.toStringTag] === 'ImmutableArrayBuffer' slot, so
// Object.prototype.toString reads them as '[object ImmutableArrayBuffer]'. A
// genuinely native immutable ArrayBuffer carries no such slot and reads as
// '[object ArrayBuffer]'. See @endo/immutable-arraybuffer README,
// "Only emulated immutable buffers carry the 'ImmutableArrayBuffer' slot".
var emulatedMarker = Object.prototype.toString.call(
  new ArrayBuffer(0).sliceToImmutable()
);
var nativeSupported = emulatedMarker !== '[object ImmutableArrayBuffer]';

// A native byteArray would be an integer-indexed exotic: a frozen Uint8Array
// with exactly `length`-many own indexed data properties. The shim's emulated
// wrapper is a plain ordinary object with zero own indexed properties. Counting
// own keys distinguishes the two shapes independently of the marker above.
var sample = frozenBytes(new Uint8Array([1, 2, 3]));
var ownIndexCount = Reflect.ownKeys(sample).length;
var nativeExoticShape = ownIndexCount === sample.length;

// The two signals must agree: native marker iff native exotic shape.
assert.sameValue(
  nativeSupported,
  nativeExoticShape,
  'emulated-marker detection and own-index-count shape detection must agree'
);

// Current reality on every pinned host: the emulated (shim) path is active, so
// the native frozen-Uint8Array / immutable-ArrayBuffer path is NOT supported.
// When this assertion begins to fail, the host has gained native support and
// the native arm of the byteArray brand check needs dedicated coverage here.
assert.sameValue(
  nativeSupported,
  false,
  'native immutable ArrayBuffer support is not provided by the pinned engine; ' +
    'the emulated @endo/immutable-arraybuffer shim path is active. When this ' +
    'fails, add native-path byteArray coverage for the newly-capable host.'
);

// Sanity: regardless of path, the emulated byteArray the prelude produces is a
// frozen Uint8Array backed by an immutable buffer (the observable contract the
// brand check depends on).
assert(Object.isFrozen(sample), 'byteArray wrapper is frozen');
assert.sameValue(sample.buffer.immutable, true, 'backing buffer is immutable');
