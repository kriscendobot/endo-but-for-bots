import harden from '@endo/harden';

const encoder =
  typeof TextEncoder === 'function' ? new TextEncoder() : undefined;

/**
 * Encodes a string as ASCII bytes (one byte per character), one character at
 * a time. Throws a `RangeError` at the first character whose code exceeds
 * 127 (out of range 0-127).
 *
 * This is the reference implementation and the slow path: `encodeAscii`
 * prefers a native `TextEncoder` pass when the platform provides one, and
 * re-enters this loop only to produce the detailed error when the input
 * proves non-ASCII.
 *
 * @param {string} s
 * @returns {Uint8Array}
 */
const encodeAsciiCharByChar = s => {
  const bytes = new Uint8Array(s.length);
  for (let i = 0; i < s.length; i += 1) {
    const code = s.charCodeAt(i);
    if (code > 127) {
      throw RangeError(
        `encodeAscii: character at index ${i} has code ${code}, which exceeds the ASCII range (0-127)`,
      );
    }
    bytes[i] = code;
  }
  return bytes;
};

/**
 * Encodes a string as ASCII bytes (one byte per character).
 * Throws a `RangeError` if any character code exceeds 127 (out of range 0-127).
 *
 * Uses a single native `TextEncoder` UTF-8 pass where available: the UTF-8
 * encoding of a string is one byte per code unit exactly when every code
 * unit is at most 127, so a byte length equal to the string length is itself
 * a complete ASCII range check. On a length mismatch (some code unit exceeds
 * 127), falls back to the character-by-character encoder to throw with the
 * offending index.
 *
 * @param {string} s
 * @returns {Uint8Array}
 */
export const encodeAscii = encoder
  ? s => {
      const bytes = encoder.encode(s);
      if (bytes.length !== s.length) {
        return encodeAsciiCharByChar(s); // throws with the offending index
      }
      return bytes;
    }
  : encodeAsciiCharByChar;
harden(encodeAscii);
