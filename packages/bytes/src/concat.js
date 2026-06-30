import harden from '@endo/harden';

import { assertGenuineUint8Array } from './genuine-uint8-array.js';

/**
 * Concatenates a list of mutable `Uint8Array` chunks into a single contiguous
 * `Uint8Array`.
 *
 * Empty input yields an empty `Uint8Array`.
 *
 * Each chunk must be a genuine integer-indexed `Uint8Array`; the copy
 * (`result.set(chunk, offset)`) reads each chunk through the integer-indexed
 * protocol. An emulated frozen byteArray wrapper would copy as silent zeros,
 * so it is rejected with a `TypeError` instead. Thaw such a value into a
 * genuine mutable `Uint8Array` first.
 *
 * @param {ReadonlyArray<Uint8Array>} chunks
 * @returns {Uint8Array}
 */
export const concatBytes = chunks => {
  let totalLength = 0;
  for (const chunk of chunks) {
    assertGenuineUint8Array(chunk, 'chunk');
    totalLength += chunk.length;
  }
  const result = new Uint8Array(totalLength);
  let offset = 0;
  for (const chunk of chunks) {
    result.set(chunk, offset);
    offset += chunk.length;
  }
  return result;
};
harden(concatBytes);
