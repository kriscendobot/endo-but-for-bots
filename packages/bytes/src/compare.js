// @ts-check

import harden from '@endo/harden';

/**
 * Compare two `Uint8Array` values lexicographically, with optional
 * start/end slicing.
 *
 * Returns a negative number when `left` sorts before `right`, `0` when
 * the two sequences are byte-for-byte equal, and a positive number when
 * `left` sorts after `right`.
 *
 * When `leftStart`, `leftEnd`, `rightStart`, or `rightEnd` are provided,
 * the comparison is restricted to those subranges — no extra allocations
 * are needed for in-place subrange comparisons.
 *
 * Both arguments must be genuine integer-indexable `Uint8Array` values:
 * `compareBytes` reads each byte by integer index (`array[i]`). A frozen
 * `Uint8Array` backed by an immutable `ArrayBuffer` on the emulated
 * `@endo/immutable-arraybuffer` path is a plain object with no
 * integer-indexed own properties, so `array[i]` returns `undefined`
 * rather than the byte; such emulated wrappers do not have integer-index
 * behavior. A caller holding a byteArray pass-style value (or any
 * possibly-emulated wrapper) must first copy it into a genuine mutable
 * `Uint8Array` (for example with `wrapper.slice(0)`, which the shim
 * amplifies) before calling this function. Keeping the conversion at the
 * call boundary lets the common, already-mutable case stay allocation
 * free.
 *
 * @param {Uint8Array} left
 * @param {Uint8Array} right
 * @param {number} [leftStart]
 * @param {number} [leftEnd]
 * @param {number} [rightStart]
 * @param {number} [rightEnd]
 * @returns {number}
 */
export const compareBytes = (
  left,
  right,
  leftStart = 0,
  leftEnd = left.length,
  rightStart = 0,
  rightEnd = right.length,
) => {
  const leftLength = leftEnd - leftStart;
  const rightLength = rightEnd - rightStart;
  let leftIndex = leftStart;
  let rightIndex = rightStart;
  for (;;) {
    if (leftIndex >= leftEnd) {
      // Left exhausted; equal if right is also exhausted, otherwise left < right.
      return leftLength - rightLength;
    }
    if (rightIndex >= rightEnd) {
      // Right exhausted but left is not; left > right.
      return 1;
    }
    if (left[leftIndex] < right[rightIndex]) {
      return -1;
    }
    if (left[leftIndex] > right[rightIndex]) {
      return 1;
    }
    leftIndex += 1;
    rightIndex += 1;
  }
};
harden(compareBytes);
