import test from '@endo/ses-ava/test.js';

import { encodeAscii } from '../src/encode.js';
import { decodeAscii } from '../src/decode.js';

test('encodeAscii / decodeAscii: empty string round-trip', t => {
  const bytes = encodeAscii('');
  t.is(bytes.length, 0);
  t.is(decodeAscii(bytes), '');
});

test('encodeAscii / decodeAscii: ASCII round-trip', t => {
  const original = 'Hello, world!';
  const bytes = encodeAscii(original);
  t.is(bytes.length, original.length);
  t.is(decodeAscii(bytes), original);
});

test('encodeAscii: rejects non-ASCII character', t => {
  t.throws(() => encodeAscii('café'), { instanceOf: RangeError });
});

test('encodeAscii: accepts full printable ASCII range', t => {
  const chars = Array.from({ length: 128 }, (_, i) =>
    String.fromCharCode(i),
  ).join('');
  const bytes = encodeAscii(chars);
  t.is(bytes.length, 128);
  t.is(decodeAscii(bytes), chars);
});

test('decodeAscii: passes through byte > 127 without error', t => {
  const bytes = new Uint8Array([0x80]);
  const result = decodeAscii(bytes);
  t.is(result.charCodeAt(0), 0x80);
});

test('encodeAscii: error names the offending index and code', t => {
  t.throws(() => encodeAscii('abcédef'), {
    instanceOf: RangeError,
    message: /index 3.*code 233/,
  });
});

test('decodeAscii: passes multi-byte UTF-8 sequences through byte-for-byte', t => {
  // A valid UTF-8 sequence must NOT be interpreted; each byte maps to its
  // own code unit.
  const bytes = new Uint8Array([0x61, 0xc3, 0xa9]);
  const result = decodeAscii(bytes);
  t.is(result.length, 3);
  t.is(result.charCodeAt(0), 0x61);
  t.is(result.charCodeAt(1), 0xc3);
  t.is(result.charCodeAt(2), 0xa9);
});

test('encodeAscii returns a plain mutable Uint8Array', t => {
  const bytes = encodeAscii('abc');
  t.true(bytes instanceof Uint8Array);
  // Must be mutable (write succeeds without throwing).
  bytes[0] = 0xff;
  t.is(bytes[0], 0xff);
});
