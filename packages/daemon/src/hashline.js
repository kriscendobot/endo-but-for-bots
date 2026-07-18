// @ts-check
/// <reference types="ses"/>
/* eslint-disable no-bitwise -- CRC32 polynomial math */

/**
 * Hashline: parser, validator, renderer, and pure splice for the
 * hash-anchored line-based edit format of `designs/cli-edit-verb.md`.
 *
 * This module is the shared pure core that the daemon-side
 * `EndoMount.edit` / `EndoGuest.edit` capability and the CLI's
 * `endo edit` verb both call into, so the agent's view and the
 * daemon's view of a patch agree byte-for-byte. It performs no I/O
 * and holds no locks; the mount layer owns the read-splice-write
 * critical section, the file-size cap, and the `path-not-found` /
 * `permission-denied` failure reasons.
 *
 * Wire contract (v1, fixed):
 * - Per-line anchor hash: CRC32 (IEEE polynomial) over the
 *   normalized line (trailing whitespace stripped, CRLF normalized
 *   to LF, leading whitespace preserved; blank lines seeded with
 *   their line number). Encoded as 2-char lowercase hex for files
 *   of at most 4096 lines, 4-char above.
 * - Whole-file CAS hash: SHA-256, 64-char lowercase hex, always.
 *   The SHA-256 function is injected (`sha256Hex` power) so this
 *   module stays pure and dependency-free.
 */

import { makeError, q, X } from '@endo/errors';

/**
 * @typedef {object} Anchor
 * @property {number} line 1-indexed line number
 * @property {string} hash 2-to-4-char lowercase hex CRC32 anchor
 */

/**
 * @typedef {'replace' | 'replace-range' | 'delete' | 'insert-after' |
 *   'insert-before' | 'prepend' | 'append'} EditOpKind
 */

/**
 * @typedef {object} EditOp
 * @property {EditOpKind} op
 * @property {Anchor} [anchor] one anchor for non-range ops
 * @property {Anchor} [anchorEnd] second anchor for range ops
 * @property {string[]} [payload] inserted lines, each a bare line
 *   content with no embedded LF
 */

/**
 * @typedef {object} EditPatch
 * @property {string} expectedFileHash SHA-256 of the file the agent
 *   read, 64-char lowercase hex
 * @property {EditOp[]} ops
 */

/**
 * @typedef {object} AnchorMismatch
 * @property {number} line
 * @property {string} hashExpected the patch's anchor hash
 * @property {string} hashActualAtPatchWidth the live line's CRC32 at
 *   the patch anchor's declared hex width ('' if the line does not
 *   exist)
 * @property {string} hashActualAtFileWidth the live line's CRC32 at
 *   the file's currently-native width ('' if the line does not exist)
 */

/**
 * @typedef {object} ReapplyAmbiguity
 * @property {number} line the original anchor line
 * @property {number[]} candidates every line in the reapply window
 *   whose hash matches the anchor
 */

/**
 * @typedef {object} EditFailure
 * @property {'hash-mismatch' | 'file-rev-mismatch' | 'ambiguous-reapply' |
 *   'patch-syntax' | 'path-not-found' | 'permission-denied'} reason
 * @property {string} fileHashActual the live file SHA-256
 * @property {string} [message] human-readable diagnostic
 * @property {AnchorMismatch[]} [mismatches] populated on `hash-mismatch`
 * @property {ReapplyAmbiguity[]} [ambiguities] populated on
 *   `ambiguous-reapply`
 */

/**
 * @typedef {object} EditResult
 * @property {boolean} success
 * @property {string} fileHashAfter SHA-256 of the file after the edit
 *   (of the unchanged file when `success` is false)
 * @property {string} [newText] the spliced file content, present only
 *   on success; the mount layer writes it and must not forward it
 *   across the capability boundary
 * @property {EditFailure} [failure] populated only when success is
 *   false
 */

/**
 * @callback Sha256Hex
 * @param {Uint8Array} bytes
 * @returns {string} 64-char lowercase hex SHA-256 digest
 */

/**
 * @typedef {object} ApplyEditOptions
 * @property {Sha256Hex} sha256Hex
 * @property {boolean} [reapply] enable bounded anchor relocation
 *   (default false, strict)
 * @property {number} [reapplyWindow] half-width of the relocation
 *   search window in lines (default 20, max 200)
 */

const EDIT_OP_KINDS = harden([
  'replace',
  'replace-range',
  'delete',
  'insert-after',
  'insert-before',
  'prepend',
  'append',
]);

/** @type {Set<string>} */
const editOpKindSet = new Set(EDIT_OP_KINDS);

/**
 * SHA-256 of the empty byte string: the canonical `expectedFileHash`
 * of an empty file.
 */
export const EMPTY_FILE_SHA256 =
  'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855';

const REAPPLY_WINDOW_DEFAULT = 20;
const REAPPLY_WINDOW_MAX = 200;

const textEncoder = new TextEncoder();

/** The IEEE CRC32 polynomial table, precomputed at module load. */
const CRC32_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let i = 0; i < 256; i += 1) {
    let c = i;
    for (let k = 0; k < 8; k += 1) {
      c = c & 1 ? 0xedb8_8320 ^ (c >>> 1) : c >>> 1;
    }
    table[i] = c >>> 0;
  }
  return table;
})();

/**
 * CRC32 (IEEE polynomial) over the UTF-8 encoding of `text`, as an
 * unsigned 32-bit integer.
 *
 * @param {string} text
 * @returns {number}
 */
export const crc32 = text => {
  const bytes = textEncoder.encode(text);
  let crc = 0xffff_ffff;
  for (let i = 0; i < bytes.length; i += 1) {
    crc = CRC32_TABLE[(crc ^ bytes[i]) & 0xff] ^ (crc >>> 8);
  }
  return (crc ^ 0xffff_ffff) >>> 0;
};
harden(crc32);

/**
 * Normalize a line for anchor hashing: strip trailing whitespace
 * (which subsumes CRLF-to-LF normalization, since a trailing `\r` is
 * trailing whitespace), preserve leading whitespace.
 *
 * @param {string} line
 * @returns {string}
 */
const normalizeLineForHash = line => line.replace(/[ \t\r]+$/, '');

/**
 * The per-line CRC32 anchor of `line` at 1-indexed `lineNumber`.
 * Empty and whitespace-only lines are seeded with the line number so
 * consecutive blank lines do not share an anchor.
 *
 * @param {string} line
 * @param {number} lineNumber 1-indexed
 * @param {number} [hexWidth] 2 or 4
 * @returns {string} lowercase hex of `hexWidth` characters
 */
export const lineAnchorHash = (line, lineNumber, hexWidth = 2) => {
  const normalized = normalizeLineForHash(line);
  const input = normalized === '' ? `${lineNumber}` : normalized;
  const masked = crc32(input) & (hexWidth === 4 ? 0xffff : 0xff);
  return masked.toString(16).padStart(hexWidth, '0');
};
harden(lineAnchorHash);

/**
 * The file's native anchor width: 2-char hex for files of at most
 * 4096 lines, 4-char above.
 *
 * @param {number} lineCount
 * @returns {2 | 4}
 */
export const anchorHexWidthForLineCount = lineCount =>
  lineCount > 4096 ? 4 : 2;
harden(anchorHexWidthForLineCount);

/**
 * Split file text into lines on LF only. A `\r` preceding an LF stays
 * on the line content so the splice round-trips CRLF byte-for-byte.
 * `trailingNewline` is true when the final byte is LF, and for the
 * empty file.
 *
 * @param {string} text
 * @returns {{ lines: string[], trailingNewline: boolean }}
 */
export const splitLines = text => {
  if (text === '') {
    return harden({ lines: [], trailingNewline: true });
  }
  const trailingNewline = text.endsWith('\n');
  const body = trailingNewline ? text.slice(0, -1) : text;
  return harden({ lines: body.split('\n'), trailingNewline });
};
harden(splitLines);

/**
 * Inverse of `splitLines`.
 *
 * @param {string[]} lines
 * @param {boolean} trailingNewline
 * @returns {string}
 */
export const joinLines = (lines, trailingNewline) => {
  if (lines.length === 0) {
    return '';
  }
  return `${lines.join('\n')}${trailingNewline ? '\n' : ''}`;
};
harden(joinLines);

/**
 * Render the hashline-annotated view of a file: each line prefixed
 * with `LINE#HASH`, line numbers right-aligned to the widest line
 * number, at the file's native anchor width.
 *
 * @param {string} text
 * @returns {string[]} annotated lines
 */
export const renderHashlineLines = text => {
  const { lines } = splitLines(text);
  const hexWidth = anchorHexWidthForLineCount(lines.length);
  const numWidth = `${lines.length}`.length;
  return harden(
    lines.map((line, index) => {
      const lineNumber = index + 1;
      const anchor = `${`${lineNumber}`.padStart(numWidth)}#${lineAnchorHash(
        line,
        lineNumber,
        hexWidth,
      )}`;
      return line === '' ? anchor : `${anchor} ${line}`;
    }),
  );
};
harden(renderHashlineLines);

/**
 * @param {unknown} value
 * @param {string} where
 * @returns {Anchor}
 */
const validateAnchor = (value, where) => {
  if (typeof value !== 'object' || value === null) {
    throw makeError(X`${q(where)}: anchor must be an object, got ${q(value)}`);
  }
  const anchor = /** @type {Record<string, unknown>} */ (value);
  if (
    typeof anchor.line !== 'number' ||
    !Number.isInteger(anchor.line) ||
    anchor.line < 1
  ) {
    throw makeError(
      X`${q(where)}: anchor.line must be a positive integer, got ${q(anchor.line)}`,
    );
  }
  if (typeof anchor.hash !== 'string' || !/^[0-9a-f]{2,4}$/.test(anchor.hash)) {
    throw makeError(
      X`${q(where)}: anchor.hash must be 2 to 4 lowercase hex chars, got ${q(anchor.hash)}`,
    );
  }
  return harden({ line: anchor.line, hash: anchor.hash });
};

/**
 * @param {unknown} value
 * @param {number} index
 * @returns {EditOp}
 */
const validateEditOp = (value, index) => {
  const where = `ops[${index}]`;
  if (typeof value !== 'object' || value === null) {
    throw makeError(X`${q(where)}: must be an object, got ${q(value)}`);
  }
  const raw = /** @type {Record<string, unknown>} */ (value);
  const { op } = raw;
  if (typeof op !== 'string' || !editOpKindSet.has(op)) {
    throw makeError(
      X`${q(where)}: op must be one of ${q([...EDIT_OP_KINDS])}, got ${q(op)}`,
    );
  }
  const opKind = /** @type {EditOpKind} */ (op);

  const needsAnchor = opKind !== 'prepend' && opKind !== 'append';
  const needsAnchorEnd = opKind === 'replace-range';
  const allowsAnchorEnd = needsAnchorEnd || opKind === 'delete';
  const needsPayload = opKind !== 'delete';

  /** @type {Anchor | undefined} */
  let anchor;
  /** @type {Anchor | undefined} */
  let anchorEnd;
  /** @type {string[] | undefined} */
  let payload;

  if (needsAnchor) {
    anchor = validateAnchor(raw.anchor, `${where}.anchor`);
  } else if (raw.anchor !== undefined) {
    throw makeError(X`${q(where)}: ${q(opKind)} must not carry anchor`);
  }
  if (allowsAnchorEnd && raw.anchorEnd !== undefined) {
    anchorEnd = validateAnchor(raw.anchorEnd, `${where}.anchorEnd`);
  } else if (raw.anchorEnd !== undefined) {
    throw makeError(X`${q(where)}: ${q(opKind)} must not carry anchorEnd`);
  }
  if (needsAnchorEnd && anchorEnd === undefined) {
    throw makeError(X`${q(where)}: ${q(opKind)} requires anchorEnd`);
  }
  if (
    anchor !== undefined &&
    anchorEnd !== undefined &&
    anchorEnd.line < anchor.line
  ) {
    throw makeError(
      X`${q(where)}: anchorEnd.line ${q(anchorEnd.line)} must be >= anchor.line ${q(anchor.line)}`,
    );
  }

  if (needsPayload) {
    if (!Array.isArray(raw.payload)) {
      throw makeError(
        X`${q(where)}: ${q(opKind)} requires payload (array of strings)`,
      );
    }
    payload = raw.payload.map((line, lineIndex) => {
      if (typeof line !== 'string') {
        throw makeError(
          X`${q(where)}: payload[${q(lineIndex)}] must be a string`,
        );
      }
      if (line.includes('\n')) {
        throw makeError(
          X`${q(where)}: payload[${q(lineIndex)}] must not contain an embedded newline; express multi-line insertion as multiple payload entries`,
        );
      }
      return line;
    });
  } else if (raw.payload !== undefined) {
    throw makeError(X`${q(where)}: ${q(opKind)} must not carry payload`);
  }

  return harden({
    op: opKind,
    ...(anchor === undefined ? {} : { anchor }),
    ...(anchorEnd === undefined ? {} : { anchorEnd }),
    ...(payload === undefined ? {} : { payload: harden(payload) }),
  });
};

/**
 * Validate an `EditPatch` envelope (the `hashline-json` wire shape).
 * Throws on any malformation; returns a hardened, structurally-valid
 * patch. The mount layer re-runs this on entry regardless of what an
 * intermediate hop claims to have validated.
 *
 * @param {unknown} value
 * @returns {EditPatch}
 */
export const validateEditPatch = value => {
  if (typeof value !== 'object' || value === null) {
    throw makeError(X`EditPatch must be an object, got ${q(value)}`);
  }
  const raw = /** @type {Record<string, unknown>} */ (value);
  if (
    typeof raw.expectedFileHash !== 'string' ||
    !/^[0-9a-f]{64}$/.test(raw.expectedFileHash)
  ) {
    throw makeError(
      X`EditPatch.expectedFileHash must be 64-char lowercase hex SHA-256, got ${q(raw.expectedFileHash)}`,
    );
  }
  if (!Array.isArray(raw.ops)) {
    throw makeError(X`EditPatch.ops must be an array`);
  }
  return harden({
    expectedFileHash: raw.expectedFileHash,
    ops: harden(raw.ops.map((op, index) => validateEditOp(op, index))),
  });
};
harden(validateEditPatch);

/**
 * Parse the textual `hashline` patch format into an `EditPatch`.
 * Throws on any syntax error.
 *
 * Grammar per the design: an `@expected-file-hash <hex>` header
 * (required), operation headers `@op anchor[..anchor]`, payload lines
 * prefixed `| ` (a bare `|` is an empty payload line), `#` comments,
 * and blank lines ending an operation.
 *
 * @param {string} text
 * @returns {EditPatch}
 */
export const parseHashlineText = text => {
  /** @type {string | undefined} */
  let expectedFileHash;
  /** @type {EditOp[]} */
  const ops = [];

  /** @type {string | undefined} */
  let currentOp;
  /** @type {Anchor | undefined} */
  let currentAnchor;
  /** @type {Anchor | undefined} */
  let currentAnchorEnd;
  /** @type {string[]} */
  let currentPayload = [];
  let currentHasPayload = false;

  const flush = () => {
    if (currentOp === undefined) {
      return;
    }
    /** @type {Record<string, unknown>} */
    const raw = { op: currentOp };
    if (currentAnchor !== undefined) {
      raw.anchor = currentAnchor;
    }
    if (currentAnchorEnd !== undefined) {
      raw.anchorEnd = currentAnchorEnd;
    }
    if (currentHasPayload) {
      raw.payload = currentPayload;
    }
    ops.push(validateEditOp(raw, ops.length));
    currentOp = undefined;
    currentAnchor = undefined;
    currentAnchorEnd = undefined;
    currentPayload = [];
    currentHasPayload = false;
  };

  /**
   * @param {string} token
   * @param {string} where
   * @returns {Anchor}
   */
  const parseAnchorToken = (token, where) => {
    const match = /^(\d+)#([0-9a-f]{2,4})$/.exec(token);
    if (match === null) {
      throw makeError(
        X`${q(where)}: malformed anchor ${q(token)}, expected LINE#HASH`,
      );
    }
    return harden({ line: Number.parseInt(match[1], 10), hash: match[2] });
  };

  const patchLines = text.split('\n');
  for (let i = 0; i < patchLines.length; i += 1) {
    const line = patchLines[i];
    const where = `patch line ${i + 1}`;

    if (line === '') {
      flush();
    } else if (line.startsWith('#')) {
      // Comment; ignored.
    } else if (line.startsWith('@expected-file-hash ')) {
      flush();
      const value = line.slice('@expected-file-hash '.length).trim();
      if (!/^[0-9a-f]{64}$/.test(value)) {
        throw makeError(
          X`${q(where)}: @expected-file-hash must be 64-char lowercase hex SHA-256, got ${q(value)}`,
        );
      }
      expectedFileHash = value;
    } else if (line.startsWith('@')) {
      flush();
      const headerBody = line.slice(1).trim();
      const spaceIndex = headerBody.indexOf(' ');
      const opToken =
        spaceIndex === -1 ? headerBody : headerBody.slice(0, spaceIndex);
      const rest =
        spaceIndex === -1 ? '' : headerBody.slice(spaceIndex + 1).trim();
      if (!editOpKindSet.has(opToken)) {
        throw makeError(
          X`${q(where)}: unknown op ${q(opToken)}; expected one of ${q([...EDIT_OP_KINDS])}`,
        );
      }
      currentOp = opToken;
      if (opToken === 'prepend' || opToken === 'append') {
        if (rest !== '') {
          throw makeError(
            X`${q(where)}: ${q(opToken)} takes no anchor, got ${q(rest)}`,
          );
        }
      } else {
        if (rest === '') {
          throw makeError(X`${q(where)}: ${q(opToken)} requires an anchor`);
        }
        const rangeIndex = rest.indexOf('..');
        if (rangeIndex === -1) {
          currentAnchor = parseAnchorToken(rest, where);
        } else {
          currentAnchor = parseAnchorToken(rest.slice(0, rangeIndex), where);
          currentAnchorEnd = parseAnchorToken(
            rest.slice(rangeIndex + 2),
            where,
          );
          if (currentOp === 'replace') {
            // The textual grammar spells a ranged replace as
            // `@replace A..B`; the envelope discriminant is
            // `replace-range`.
            currentOp = 'replace-range';
          }
        }
      }
    } else if (line.startsWith('| ') || line === '|') {
      if (currentOp === undefined) {
        throw makeError(X`${q(where)}: payload line outside any @op operation`);
      }
      currentPayload.push(line === '|' ? '' : line.slice(2));
      currentHasPayload = true;
    } else {
      throw makeError(
        X`${q(where)}: unexpected line ${q(line)}; expected @op header, | payload, # comment, or blank line`,
      );
    }
  }
  flush();

  if (expectedFileHash === undefined) {
    throw makeError(
      X`hashline patch: missing required @expected-file-hash header`,
    );
  }

  return harden({ expectedFileHash, ops: harden(ops) });
};
harden(parseHashlineText);

/**
 * The inclusive line span an op consumes, or undefined for pure
 * inserts.
 *
 * @param {EditOp} op
 * @returns {{ start: number, end: number } | undefined}
 */
const consumedSpan = op => {
  if (op.op === 'replace' || op.op === 'delete' || op.op === 'replace-range') {
    const anchor = /** @type {Anchor} */ (op.anchor);
    const end = op.anchorEnd === undefined ? anchor.line : op.anchorEnd.line;
    return { start: anchor.line, end };
  }
  return undefined;
};

/**
 * @param {string} fileHashActual
 * @param {EditFailure['reason']} reason
 * @param {string} message
 * @param {Partial<EditFailure>} [extra]
 * @returns {EditResult}
 */
const failureResult = (fileHashActual, reason, message, extra = {}) =>
  harden({
    success: false,
    fileHashAfter: fileHashActual,
    failure: { reason, fileHashActual, message, ...extra },
  });

/**
 * Apply a hashline `EditPatch` to file text, enforcing the SHA-256
 * file-rev CAS and per-line anchor validation, and return either the
 * spliced text or a structured failure. Pure: the caller reads the
 * file before and writes `newText` after, under whatever lock the
 * backing store requires.
 *
 * Validation order: envelope shape (`patch-syntax`), file-rev CAS
 * (`file-rev-mismatch`), per-line anchors with optional reapply
 * relocation (`hash-mismatch` / `ambiguous-reapply`), then splice
 * composition conflicts (`patch-syntax`).
 *
 * Same-line composition: operations anchored on the same line
 * compose as insert-before payloads, then the consumed line's
 * replacement (or the original line, or nothing for a delete), then
 * insert-after payloads — realizing the design's fixed
 * insert-after / insert-before / replace-or-delete priority as one
 * deterministic splice per line. Two ops that both consume a line
 * (two replaces, overlapping ranges) and inserts anchored strictly
 * inside a consumed range are `patch-syntax` failures.
 *
 * @param {string} fileText
 * @param {unknown} patchValue an `EditPatch` envelope; re-validated
 *   here regardless of provenance
 * @param {ApplyEditOptions} options
 * @returns {EditResult}
 */
export const applyEditPatch = (fileText, patchValue, options) => {
  const {
    sha256Hex,
    reapply = false,
    reapplyWindow = REAPPLY_WINDOW_DEFAULT,
  } = options;
  if (typeof sha256Hex !== 'function') {
    throw makeError(X`applyEditPatch: options.sha256Hex is required`);
  }
  if (
    !Number.isInteger(reapplyWindow) ||
    reapplyWindow < 1 ||
    reapplyWindow > REAPPLY_WINDOW_MAX
  ) {
    throw makeError(
      X`applyEditPatch: options.reapplyWindow must be an integer in [1, ${q(REAPPLY_WINDOW_MAX)}], got ${q(reapplyWindow)}`,
    );
  }

  const fileHashActual = sha256Hex(textEncoder.encode(fileText));

  /** @type {EditPatch} */
  let patch;
  try {
    patch = validateEditPatch(patchValue);
  } catch (error) {
    return failureResult(
      fileHashActual,
      'patch-syntax',
      /** @type {Error} */ (error).message,
    );
  }

  if (patch.expectedFileHash !== fileHashActual) {
    return failureResult(
      fileHashActual,
      'file-rev-mismatch',
      'the file changed since it was read; re-read at the current revision',
    );
  }

  const { lines, trailingNewline } = splitLines(fileText);
  const fileWidth = anchorHexWidthForLineCount(lines.length);

  /**
   * @param {Anchor} anchor
   * @returns {boolean} whether the anchor matches the live line
   */
  const anchorMatches = anchor =>
    anchor.line <= lines.length &&
    lineAnchorHash(lines[anchor.line - 1], anchor.line, anchor.hash.length) ===
      anchor.hash;

  /** @type {AnchorMismatch[]} */
  const mismatches = [];
  /** @type {ReapplyAmbiguity[]} */
  const ambiguities = [];
  /** @type {Map<Anchor, number>} relocated anchor -> new line */
  const relocations = new Map();

  /** @param {Anchor} anchor */
  const recordMismatch = anchor => {
    const exists = anchor.line <= lines.length;
    mismatches.push(
      harden({
        line: anchor.line,
        hashExpected: anchor.hash,
        hashActualAtPatchWidth: exists
          ? lineAnchorHash(
              lines[anchor.line - 1],
              anchor.line,
              anchor.hash.length,
            )
          : '',
        hashActualAtFileWidth: exists
          ? lineAnchorHash(lines[anchor.line - 1], anchor.line, fileWidth)
          : '',
      }),
    );
  };

  /**
   * Bounded relocation search per the design: visit the window in
   * nearest-by-distance order, lower line number first on ties,
   * collect every candidate whose hash matches at the anchor's
   * declared width.
   *
   * @param {Anchor} anchor
   */
  const searchReapply = anchor => {
    /** @type {number[]} */
    const candidates = [];
    for (let distance = 0; distance <= reapplyWindow; distance += 1) {
      for (const candidateLine of distance === 0
        ? [anchor.line]
        : [anchor.line - distance, anchor.line + distance]) {
        if (candidateLine >= 1 && candidateLine <= lines.length) {
          const actual = lineAnchorHash(
            lines[candidateLine - 1],
            candidateLine,
            anchor.hash.length,
          );
          if (actual === anchor.hash) {
            candidates.push(candidateLine);
          }
        }
      }
    }
    if (candidates.length === 1) {
      relocations.set(anchor, candidates[0]);
    } else if (candidates.length > 1) {
      ambiguities.push(
        harden({ line: anchor.line, candidates: harden(candidates) }),
      );
    } else {
      recordMismatch(anchor);
    }
  };

  for (const op of patch.ops) {
    for (const anchor of [op.anchor, op.anchorEnd]) {
      if (anchor !== undefined && !anchorMatches(anchor)) {
        if (reapply) {
          searchReapply(anchor);
        } else {
          recordMismatch(anchor);
        }
      }
    }
  }

  if (ambiguities.length > 0) {
    return failureResult(
      fileHashActual,
      'ambiguous-reapply',
      'multiple candidate lines match a relocated anchor',
      { ambiguities: harden(ambiguities) },
    );
  }
  if (mismatches.length > 0) {
    return failureResult(
      fileHashActual,
      'hash-mismatch',
      'one or more line anchors do not match the live file',
      { mismatches: harden(mismatches) },
    );
  }

  /**
   * Ops with relocated anchor lines applied.
   * @type {EditOp[]}
   */
  const ops = patch.ops.map(op => {
    /** @param {Anchor | undefined} anchor */
    const relocated = anchor =>
      anchor === undefined || !relocations.has(anchor)
        ? anchor
        : harden({
            line: /** @type {number} */ (relocations.get(anchor)),
            hash: anchor.hash,
          });
    const anchor = relocated(op.anchor);
    const anchorEnd = relocated(op.anchorEnd);
    if (anchor === op.anchor && anchorEnd === op.anchorEnd) {
      return op;
    }
    return harden({
      op: op.op,
      ...(anchor === undefined ? {} : { anchor }),
      ...(anchorEnd === undefined ? {} : { anchorEnd }),
      ...(op.payload === undefined ? {} : { payload: op.payload }),
    });
  });

  // Relocation may have inverted a range.
  for (const op of ops) {
    if (
      op.anchor !== undefined &&
      op.anchorEnd !== undefined &&
      op.anchorEnd.line < op.anchor.line
    ) {
      return failureResult(
        fileHashActual,
        'patch-syntax',
        `range op anchors inverted after relocation: ${op.anchor.line}..${op.anchorEnd.line}`,
      );
    }
    const span = consumedSpan(op);
    if (span !== undefined && span.end > lines.length) {
      return failureResult(
        fileHashActual,
        'patch-syntax',
        `op consumes line ${span.end} beyond the end of the ${lines.length}-line file`,
      );
    }
  }

  // Composition conflict checks: no two ops may consume the same
  // line, and no anchored op may target a line strictly inside
  // another op's consumed range (anchoring on a consumed single line
  // composes; anchoring inside a multi-line range is ambiguous).
  /** @type {Map<number, EditOp>} line -> consuming op */
  const consumedBy = new Map();
  for (const op of ops) {
    const span = consumedSpan(op);
    if (span !== undefined) {
      for (let line = span.start; line <= span.end; line += 1) {
        if (consumedBy.has(line)) {
          return failureResult(
            fileHashActual,
            'patch-syntax',
            `two operations both consume line ${line}`,
          );
        }
        consumedBy.set(line, op);
      }
    }
  }
  for (const op of ops) {
    if (op.op === 'insert-after' || op.op === 'insert-before') {
      const { line } = /** @type {Anchor} */ (op.anchor);
      const consumer = consumedBy.get(line);
      if (consumer !== undefined) {
        const span = /** @type {{ start: number, end: number }} */ (
          consumedSpan(consumer)
        );
        if (span.start !== span.end) {
          return failureResult(
            fileHashActual,
            'patch-syntax',
            `insert anchored on line ${line}, which another operation consumes as part of the range ${span.start}..${span.end}`,
          );
        }
      }
    }
  }

  // Compose per-line splice actions. Each action covers an inclusive
  // original-line span and yields replacement lines.
  /**
   * @typedef {object} SpliceAction
   * @property {number} start 1-indexed first consumed line, or the
   *   insertion point line for pure inserts (0 span)
   * @property {number} end 1-indexed last consumed line; start - 1
   *   for a pure insert before `start`
   * @property {string[]} content
   */

  /** @type {Map<number, { before: string[], after: string[], consume: EditOp | undefined }>} */
  const lineGroups = new Map();
  /** @type {{ span: { start: number, end: number }, op: EditOp }[]} */
  const rangeOps = [];
  /** @type {string[]} */
  const prependPayload = [];
  /** @type {string[]} */
  const appendPayload = [];

  const groupAt = (/** @type {number} */ line) => {
    let group = lineGroups.get(line);
    if (group === undefined) {
      group = { before: [], after: [], consume: undefined };
      lineGroups.set(line, group);
    }
    return group;
  };

  for (const op of ops) {
    if (op.op === 'prepend') {
      prependPayload.push(.../** @type {string[]} */ (op.payload));
    } else if (op.op === 'append') {
      appendPayload.push(.../** @type {string[]} */ (op.payload));
    } else if (op.op === 'insert-before') {
      groupAt(/** @type {Anchor} */ (op.anchor).line).before.push(
        .../** @type {string[]} */ (op.payload),
      );
    } else if (op.op === 'insert-after') {
      groupAt(/** @type {Anchor} */ (op.anchor).line).after.push(
        .../** @type {string[]} */ (op.payload),
      );
    } else {
      const span = /** @type {{ start: number, end: number }} */ (
        consumedSpan(op)
      );
      if (span.start === span.end) {
        groupAt(span.start).consume = op;
      } else {
        rangeOps.push({ span, op });
      }
    }
  }

  /** @type {SpliceAction[]} */
  const actions = [];
  for (const [line, group] of lineGroups) {
    /** @type {string[]} */
    const content = [...group.before];
    if (group.consume === undefined) {
      content.push(lines[line - 1]);
    } else if (group.consume.op !== 'delete') {
      content.push(.../** @type {string[]} */ (group.consume.payload));
    }
    content.push(...group.after);
    actions.push({ start: line, end: line, content });
  }
  for (const { span, op } of rangeOps) {
    actions.push({
      start: span.start,
      end: span.end,
      content:
        op.op === 'delete' ? [] : [.../** @type {string[]} */ (op.payload)],
    });
  }

  // Bottom-up: apply the highest-positioned action first so earlier
  // actions' original-coordinate indices stay valid.
  actions.sort((a, b) => b.start - a.start);

  /** @type {string[]} */
  const result = [...lines];
  result.push(...appendPayload);
  for (const action of actions) {
    result.splice(
      action.start - 1,
      action.end - action.start + 1,
      ...action.content,
    );
  }
  result.unshift(...prependPayload);

  const newText = joinLines(result, trailingNewline);
  const fileHashAfter = sha256Hex(textEncoder.encode(newText));
  return harden({ success: true, fileHashAfter, newText });
};
harden(applyEditPatch);
