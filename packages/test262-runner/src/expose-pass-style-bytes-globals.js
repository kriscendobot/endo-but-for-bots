// Expose pass-style bytes helpers as globals for the pass-style-bytes parity
// tests. This interlude is shared verbatim by both preludes (node-prelude.js
// and xs-prelude.js): the SES lockdown each prelude runs before importing this
// module differs between the two hosts, but the globals it installs do not.
//
// The immutable-arraybuffer shim is already installed by the SES lockdown the
// importing prelude ran first (ses/src/lockdown.js eagerly imports
// @endo/immutable-arraybuffer/shim.js); the detect-then-skip guard inside the
// shim makes any re-installation a no-op when native support is present, so the
// second install attempt reached through to-bytes.js below is harmless.
/* global globalThis */
import { frozenBytes } from '@endo/pass-style/to-bytes.js';
import { thawnBytes } from '@endo/pass-style/from-bytes.js';
import { concatBytes } from '@endo/pass-style/concat-bytes.js';
import { encodeUtf8 } from '@endo/pass-style/encode-utf8.js';
import { decodeUtf8 } from '@endo/pass-style/decode-utf8.js';
import { strictDecodeUtf8 } from '@endo/pass-style/strict-decode-utf8.js';

globalThis.frozenBytes = frozenBytes;
globalThis.thawnBytes = thawnBytes;
globalThis.concatBytes = concatBytes;
globalThis.encodeUtf8 = encodeUtf8;
globalThis.decodeUtf8 = decodeUtf8;
globalThis.strictDecodeUtf8 = strictDecodeUtf8;
