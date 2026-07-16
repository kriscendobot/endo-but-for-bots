import harden from '@endo/harden';

const decoder =
  typeof TextDecoder === 'function' ? new TextDecoder() : undefined;

/**
 * Decodes ASCII bytes to a string, one byte at a time. Bytes outside the
 * ASCII range (0-127) are passed through without error as the corresponding
 * code units.
 *
 * This is the reference implementation and the slow path: `decodeAscii`
 * prefers a native `TextDecoder` pass when the platform provides one, and
 * re-enters this loop only when the input proves non-ASCII, to preserve the
 * pass-through behavior (a UTF-8 decoder would instead interpret multi-byte
 * sequences or substitute U+FFFD).
 *
 * @param {Uint8Array} bytes
 * @returns {string}
 */
const decodeAsciiByteByByte = bytes => {
  let s = '';
  for (let i = 0; i < bytes.length; i += 1) {
    s += String.fromCharCode(bytes[i]);
  }
  return s;
};

/**
 * Decodes ASCII bytes to a string.
 * Bytes outside the ASCII range (0-127) are passed through without
 * error; use `encodeAscii` on the source string to ensure only valid ASCII
 * bytes enter the pipeline.
 *
 * Uses a single native `TextDecoder` UTF-8 pass where available. A UTF-8
 * decode of all-ASCII input yields exactly one code unit per byte and can
 * produce no U+FFFD replacement character, so a same-length result free of
 * U+FFFD is itself a complete ASCII range check; any byte above 127 either
 * begins a multi-byte sequence (shortening the result) or is invalid
 * (substituting U+FFFD). On either signal, falls back to the byte-by-byte
 * decoder to preserve the documented pass-through behavior.
 *
 * @param {Uint8Array} bytes
 * @returns {string}
 */
export const decodeAscii = decoder
  ? bytes => {
      const s = decoder.decode(bytes);
      if (s.length !== bytes.length || s.indexOf('\ufffd') !== -1) {
        return decodeAsciiByteByByte(bytes);
      }
      return s;
    }
  : decodeAsciiByteByByte;
harden(decodeAscii);
