import harden from '@endo/harden';

const { getPrototypeOf, getOwnPropertyDescriptor } = Object;
const { apply } = Reflect;
const { toStringTag: toStringTagSymbol } = Symbol;

// Capture the `%TypedArray%.prototype[Symbol.toStringTag]` getter so we can
// brand-check a candidate against the genuine typed-array internal slot.
// Applied to a genuine typed array it returns the constructor name
// (`'Uint8Array'`, `'Int8Array'`, ...); applied to anything lacking the
// `[[TypedArrayName]]` internal slot it returns `undefined`, without throwing,
// even when the candidate's `[[Prototype]]` is `Uint8Array.prototype`. That is
// the case that matters here: a frozen `Uint8Array` wrapper produced by the
// emulated `@endo/immutable-arraybuffer` path is a plain object whose prototype
// is `Uint8Array.prototype` but which has no integer-indexed bytes, so reading
// `wrapper[i]` returns `undefined` rather than the byte. This is the same brand
// check `@endo/pass-style` and SES's hardener use; it is duplicated here to
// keep `@endo/bytes` dependency-light (its only dependency is `@endo/harden`).
const typedArrayPrototype = getPrototypeOf(Uint8Array.prototype);
const toStringTagDescriptor = getOwnPropertyDescriptor(
  typedArrayPrototype,
  toStringTagSymbol,
);
const getTypedArrayToStringTag =
  toStringTagDescriptor && toStringTagDescriptor.get;
if (typeof getTypedArrayToStringTag !== 'function') {
  throw new TypeError(
    'Cannot capture the %TypedArray%.prototype[Symbol.toStringTag] getter',
  );
}

/**
 * Asserts that `candidate` is a genuine integer-indexed `Uint8Array`: a real
 * typed array whose bytes are readable by integer index (`candidate[i]`), not
 * an emulated frozen byteArray wrapper, some other typed array, or a non-array
 * value.
 *
 * Every read-only reader in this package reaches each byte through the
 * integer-indexed protocol (`array[i]`, `array.set(...)`). A counterfeit that
 * merely inherits from `Uint8Array.prototype` (the emulated immutable-buffer
 * byteArray wrapper) answers those reads with `undefined`, so a reader given
 * one would complete successfully with the wrong answer and no diagnostic.
 * This guard converts that silent corruption into a loud `TypeError` at the
 * call boundary, honoring the contract the package README already states:
 * passing a frozen byteArray to any function here throws, and the caller must
 * first thaw it into a genuine mutable `Uint8Array`.
 *
 * @param {unknown} candidate
 * @param {string} [label] role of the value in a diagnostic, such as `'left'`.
 * @returns {asserts candidate is Uint8Array}
 */
export const assertGenuineUint8Array = (candidate, label = 'argument') => {
  const tag = apply(getTypedArrayToStringTag, candidate, []);
  if (tag === 'Uint8Array') {
    return;
  }
  let received;
  if (tag !== undefined) {
    // A different genuine typed array (`Int8Array`, `Float64Array`, ...). Its
    // bytes read fine, but its element semantics are not the unsigned bytes
    // these helpers compare and concatenate, so it is rejected too.
    received = `a ${tag}`;
  } else if (candidate instanceof Uint8Array) {
    // Prototype is `Uint8Array.prototype` but the typed-array internal slot is
    // absent: an emulated immutable-`ArrayBuffer` byteArray wrapper (or a
    // similar counterfeit) whose `candidate[i]` reads would return `undefined`.
    received =
      'a Uint8Array-prototyped wrapper with no integer-indexed bytes (such as a frozen byteArray backed by an emulated immutable ArrayBuffer); thaw it into a genuine mutable Uint8Array first, for example with `thawnBytes` from `@endo/pass-style/from-bytes.js` or `wrapper.slice(0)`';
  } else {
    received = candidate === null ? 'null' : typeof candidate;
  }
  throw new TypeError(
    `${label} must be a genuine integer-indexed Uint8Array; got ${received}`,
  );
};
harden(assertGenuineUint8Array);
