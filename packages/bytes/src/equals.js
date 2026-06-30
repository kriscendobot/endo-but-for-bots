import harden from '@endo/harden';

import { assertGenuineUint8Array } from './genuine-uint8-array.js';

/**
 * Compares two `Uint8Array` values byte-for-byte.
 * Returns `true` when the two arrays have equal length and equal contents.
 *
 * Both arguments must be genuine integer-indexed `Uint8Array` values;
 * `bytesEqual` reads each byte by integer index (`array[i]`). An emulated
 * frozen byteArray wrapper answers those reads with `undefined`, so it is
 * rejected with a `TypeError` rather than silently compared. Thaw such a
 * value into a genuine mutable `Uint8Array` first.
 *
 * @param {Uint8Array} a
 * @param {Uint8Array} b
 * @returns {boolean}
 */
export const bytesEqual = (a, b) => {
  assertGenuineUint8Array(a, 'a');
  assertGenuineUint8Array(b, 'b');
  if (a === b) {
    return true;
  }
  if (a.length !== b.length) {
    return false;
  }
  for (let i = 0; i < a.length; i += 1) {
    if (a[i] !== b[i]) {
      return false;
    }
  }
  return true;
};
harden(bytesEqual);
