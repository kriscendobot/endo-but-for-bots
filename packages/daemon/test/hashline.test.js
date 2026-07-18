// @ts-check

import '@endo/init/debug.js';

import { createHash } from 'node:crypto';
import test from 'ava';

import {
  EMPTY_FILE_SHA256,
  anchorHexWidthForLineCount,
  applyEditPatch,
  crc32,
  joinLines,
  lineAnchorHash,
  parseHashlineText,
  renderHashlineLines,
  splitLines,
  validateEditPatch,
} from '../src/hashline.js';

/** @param {Uint8Array} bytes */
const sha256Hex = bytes => createHash('sha256').update(bytes).digest('hex');

/** @param {string} text */
const hashOfText = text => sha256Hex(new TextEncoder().encode(text));

/**
 * A patch envelope against `text` with anchors computed from the live
 * file, so tests express edits by line number and content only.
 *
 * @param {string} text
 * @param {Array<Record<string, unknown>>} ops with `line` /
 *   `lineEnd` shorthand instead of anchors
 */
const patchFor = (text, ops) => {
  const { lines } = splitLines(text);
  const width = anchorHexWidthForLineCount(lines.length);
  /** @param {number} line */
  const anchorAt = line => ({
    line,
    hash: lineAnchorHash(lines[line - 1], line, width),
  });
  return {
    expectedFileHash: hashOfText(text),
    ops: ops.map(({ line, lineEnd, ...rest }) => ({
      ...rest,
      ...(line === undefined ? {} : { anchor: anchorAt(Number(line)) }),
      ...(lineEnd === undefined
        ? {}
        : { anchorEnd: anchorAt(Number(lineEnd)) }),
    })),
  };
};

const options = { sha256Hex };

/**
 * @param {import('../src/hashline.js').EditResult} result
 * @returns {import('../src/hashline.js').EditFailure}
 */
const failureOf = result =>
  /** @type {import('../src/hashline.js').EditFailure} */ (result.failure);

// --- crc32 and anchors ---

test('crc32 matches the IEEE check value', t => {
  t.is(crc32('123456789'), 0xcbf4_3926);
});

test('lineAnchorHash widths and normalization', t => {
  const h2 = lineAnchorHash('const x = 1;', 7, 2);
  t.regex(h2, /^[0-9a-f]{2}$/);
  const h4 = lineAnchorHash('const x = 1;', 7, 4);
  t.regex(h4, /^[0-9a-f]{4}$/);
  // The 2-char hash is the low byte of the 4-char hash.
  t.is(h4.slice(2), h2);
  // Trailing whitespace and a trailing CR do not change the hash.
  t.is(lineAnchorHash('const x = 1;  \t', 7), h2);
  t.is(lineAnchorHash('const x = 1;\r', 7), h2);
  // Leading whitespace does.
  t.not(lineAnchorHash('  const x = 1;', 7), h2);
});

test('blank lines are seeded with their line number', t => {
  t.not(lineAnchorHash('', 3), lineAnchorHash('', 4));
  t.is(lineAnchorHash('   ', 3), lineAnchorHash('', 3));
});

test('anchorHexWidthForLineCount crosses at 4096 lines', t => {
  t.is(anchorHexWidthForLineCount(0), 2);
  t.is(anchorHexWidthForLineCount(4096), 2);
  t.is(anchorHexWidthForLineCount(4097), 4);
});

// --- splitLines / joinLines ---

test('splitLines and joinLines round-trip', t => {
  for (const text of ['', 'a', 'a\n', 'a\nb', 'a\r\nb\n', '\n', 'a\n\nb\n']) {
    const { lines, trailingNewline } = splitLines(text);
    t.is(joinLines([...lines], trailingNewline), text, JSON.stringify(text));
  }
});

test('splitLines treats the empty file as trailing-newline-true', t => {
  t.deepEqual(splitLines(''), { lines: [], trailingNewline: true });
});

test('splitLines keeps CR on the line content', t => {
  t.deepEqual(splitLines('a\r\nb\n').lines, ['a\r', 'b']);
});

// --- renderHashlineLines ---

test('renderHashlineLines annotates each line', t => {
  const text = '# Today\n\nBuy milk.\n';
  const rendered = renderHashlineLines(text);
  t.deepEqual(rendered, [
    `1#${lineAnchorHash('# Today', 1)} # Today`,
    `2#${lineAnchorHash('', 2)}`,
    `3#${lineAnchorHash('Buy milk.', 3)} Buy milk.`,
  ]);
});

test('renderHashlineLines right-aligns line numbers', t => {
  const text = `${Array.from({ length: 10 }, (_, i) => `line ${i}`).join('\n')}\n`;
  const rendered = renderHashlineLines(text);
  t.true(rendered[0].startsWith(' 1#'));
  t.true(rendered[9].startsWith('10#'));
});

// --- validateEditPatch ---

test('validateEditPatch accepts a well-formed envelope', t => {
  const patch = validateEditPatch({
    expectedFileHash: EMPTY_FILE_SHA256,
    ops: [{ op: 'append', payload: ['hello'] }],
  });
  t.is(patch.ops.length, 1);
});

test('validateEditPatch rejects malformed envelopes', t => {
  const base = { expectedFileHash: EMPTY_FILE_SHA256 };
  const cases = [
    {},
    { expectedFileHash: 'nope', ops: [] },
    { ...base, ops: [{ op: 'frobnicate' }] },
    // replace requires anchor and payload
    { ...base, ops: [{ op: 'replace', payload: ['x'] }] },
    { ...base, ops: [{ op: 'replace', anchor: { line: 1, hash: 'ab' } }] },
    // delete must not carry payload
    {
      ...base,
      ops: [{ op: 'delete', anchor: { line: 1, hash: 'ab' }, payload: ['x'] }],
    },
    // prepend must not carry anchor
    {
      ...base,
      ops: [{ op: 'prepend', anchor: { line: 1, hash: 'ab' }, payload: ['x'] }],
    },
    // replace-range requires anchorEnd
    {
      ...base,
      ops: [
        { op: 'replace-range', anchor: { line: 1, hash: 'ab' }, payload: [] },
      ],
    },
    // replace must not carry anchorEnd
    {
      ...base,
      ops: [
        {
          op: 'replace',
          anchor: { line: 1, hash: 'ab' },
          anchorEnd: { line: 2, hash: 'cd' },
          payload: ['x'],
        },
      ],
    },
    // inverted range
    {
      ...base,
      ops: [
        {
          op: 'delete',
          anchor: { line: 5, hash: 'ab' },
          anchorEnd: { line: 2, hash: 'cd' },
        },
      ],
    },
    // embedded newline in payload
    { ...base, ops: [{ op: 'append', payload: ['a\nb'] }] },
    // bad anchor shapes
    { ...base, ops: [{ op: 'delete', anchor: { line: 0, hash: 'ab' } }] },
    { ...base, ops: [{ op: 'delete', anchor: { line: 1, hash: 'xyz' } }] },
    { ...base, ops: [{ op: 'delete', anchor: { line: 1, hash: 'abcde' } }] },
  ];
  for (const [index, envelope] of cases.entries()) {
    t.throws(() => validateEditPatch(envelope), undefined, `case ${index}`);
  }
});

// --- parseHashlineText ---

test('parseHashlineText parses a full patch', t => {
  const hash = EMPTY_FILE_SHA256;
  const patch = parseHashlineText(
    [
      '# a leading comment',
      `@expected-file-hash ${hash}`,
      '@replace 4#7e',
      '| Buy eggs (the brown ones).',
      '',
      '@insert-after 4#7e',
      '| Buy bread.',
      '|',
      '',
      '@delete 12#aa..14#bc',
      '',
      '@replace 6#0a..7#0b',
      '| merged',
      '',
      '@prepend',
      '| #!/usr/bin/env node',
      '',
      '@append',
      '| EOF',
    ].join('\n'),
  );
  t.is(patch.expectedFileHash, hash);
  t.deepEqual(
    patch.ops.map(op => op.op),
    ['replace', 'insert-after', 'delete', 'replace-range', 'prepend', 'append'],
  );
  // The empty payload line (`|`) survives as an empty string.
  t.deepEqual(patch.ops[1].payload, ['Buy bread.', '']);
  t.deepEqual(patch.ops[2].anchor, { line: 12, hash: 'aa' });
  t.deepEqual(patch.ops[2].anchorEnd, { line: 14, hash: 'bc' });
});

test('parseHashlineText rejects syntax errors', t => {
  const hash = EMPTY_FILE_SHA256;
  const cases = [
    // missing @expected-file-hash header
    '@append\n| x',
    // malformed header hash
    '@expected-file-hash zzz\n@append\n| x',
    // unknown op
    `@expected-file-hash ${hash}\n@frobnicate 1#ab`,
    // malformed anchor
    `@expected-file-hash ${hash}\n@replace 1#zz\n| x`,
    // anchor on prepend
    `@expected-file-hash ${hash}\n@prepend 1#ab\n| x`,
    // missing anchor on replace
    `@expected-file-hash ${hash}\n@replace\n| x`,
    // payload outside any op
    `@expected-file-hash ${hash}\n| stray`,
    // free text
    `@expected-file-hash ${hash}\nnot a patch line`,
  ];
  for (const [index, text] of cases.entries()) {
    t.throws(() => parseHashlineText(text), undefined, `case ${index}`);
  }
});

// --- applyEditPatch: the worked example ---

test('applyEditPatch applies the design worked example', t => {
  const text = '# Today’s notes\n\nBuy milk.\nBuy eggs.\n\n';
  const patch = patchFor(text, [
    { op: 'replace', line: 4, payload: ['Buy eggs (the brown ones).'] },
    { op: 'insert-after', line: 4, payload: ['Buy bread.'] },
  ]);
  const result = applyEditPatch(text, patch, options);
  t.true(result.success);
  t.is(
    result.newText,
    '# Today’s notes\n\nBuy milk.\nBuy eggs (the brown ones).\nBuy bread.\n\n',
  );
  t.is(
    result.fileHashAfter,
    hashOfText(/** @type {string} */ (result.newText)),
  );
});

// --- applyEditPatch: CAS and anchor validation ---

test('applyEditPatch fails file-rev-mismatch on a stale file hash', t => {
  const text = 'a\nb\n';
  const patch = patchFor(text, [{ op: 'delete', line: 1 }]);
  const changed = 'a\nb\nc\n';
  const result = applyEditPatch(changed, patch, options);
  t.false(result.success);
  t.is(result.newText, undefined);
  const failure = failureOf(result);
  t.is(failure.reason, 'file-rev-mismatch');
  t.is(failure.fileHashActual, hashOfText(changed));
  t.is(result.fileHashAfter, hashOfText(changed));
});

test('applyEditPatch fails hash-mismatch with both widths reported', t => {
  const text = 'alpha\nbeta\ngamma\n';
  const patch = {
    expectedFileHash: hashOfText(text),
    ops: [
      {
        op: 'replace',
        anchor: { line: 2, hash: '0000' },
        payload: ['BETA'],
      },
    ],
  };
  const result = applyEditPatch(text, patch, options);
  t.false(result.success);
  const failure = failureOf(result);
  t.is(failure.reason, 'hash-mismatch');
  t.deepEqual(failure.mismatches, [
    {
      line: 2,
      hashExpected: '0000',
      hashActualAtPatchWidth: lineAnchorHash('beta', 2, 4),
      hashActualAtFileWidth: lineAnchorHash('beta', 2, 2),
    },
  ]);
});

test('applyEditPatch reports an out-of-range anchor as a mismatch', t => {
  const text = 'only\n';
  const patch = {
    expectedFileHash: hashOfText(text),
    ops: [{ op: 'delete', anchor: { line: 9, hash: 'ab' } }],
  };
  const result = applyEditPatch(text, patch, options);
  t.false(result.success);
  const failure = failureOf(result);
  t.is(failure.reason, 'hash-mismatch');
  t.deepEqual(failure.mismatches, [
    {
      line: 9,
      hashExpected: 'ab',
      hashActualAtPatchWidth: '',
      hashActualAtFileWidth: '',
    },
  ]);
});

test('applyEditPatch returns patch-syntax on a malformed envelope', t => {
  const text = 'a\n';
  const result = applyEditPatch(text, { nope: true }, options);
  t.false(result.success);
  t.is(failureOf(result).reason, 'patch-syntax');
  t.is(result.fileHashAfter, hashOfText(text));
});

test('a 2-char anchor still validates against a wide file', t => {
  const lineCount = 5000;
  const text = `${Array.from({ length: lineCount }, (_, i) => `line ${i}`).join('\n')}\n`;
  t.is(anchorHexWidthForLineCount(lineCount), 4);
  const patch = {
    expectedFileHash: hashOfText(text),
    ops: [
      {
        op: 'replace',
        // A narrow anchor, as authored against an old 2-char render.
        anchor: { line: 3, hash: lineAnchorHash('line 2', 3, 2) },
        payload: ['LINE 2'],
      },
    ],
  };
  const result = applyEditPatch(text, patch, options);
  t.true(result.success);
  t.true(/** @type {string} */ (result.newText).includes('LINE 2\nline 3'));
});

// --- applyEditPatch: splice semantics ---

test('same-line ops compose as before, replacement, after', t => {
  const text = 'one\ntwo\nthree\n';
  const patch = patchFor(text, [
    { op: 'insert-after', line: 2, payload: ['after'] },
    { op: 'replace', line: 2, payload: ['TWO'] },
    { op: 'insert-before', line: 2, payload: ['before'] },
  ]);
  const result = applyEditPatch(text, patch, options);
  t.true(result.success);
  t.is(result.newText, 'one\nbefore\nTWO\nafter\nthree\n');
});

test('inserts compose around a same-line delete', t => {
  const text = 'one\ntwo\nthree\n';
  const patch = patchFor(text, [
    { op: 'insert-before', line: 2, payload: ['before'] },
    { op: 'delete', line: 2 },
    { op: 'insert-after', line: 2, payload: ['after'] },
  ]);
  const result = applyEditPatch(text, patch, options);
  t.true(result.success);
  t.is(result.newText, 'one\nbefore\nafter\nthree\n');
});

test('multiple ops apply bottom-up against original coordinates', t => {
  const text = 'a\nb\nc\nd\ne\n';
  const patch = patchFor(text, [
    { op: 'delete', line: 1 },
    { op: 'replace', line: 3, payload: ['C', 'C2'] },
    { op: 'insert-after', line: 5, payload: ['f'] },
  ]);
  const result = applyEditPatch(text, patch, options);
  t.true(result.success);
  t.is(result.newText, 'b\nC\nC2\nd\ne\nf\n');
});

test('range replace and range delete', t => {
  const text = 'a\nb\nc\nd\ne\n';
  const replaced = applyEditPatch(
    text,
    patchFor(text, [
      { op: 'replace-range', line: 2, lineEnd: 4, payload: ['BCD'] },
    ]),
    options,
  );
  t.is(replaced.newText, 'a\nBCD\ne\n');
  const deleted = applyEditPatch(
    text,
    patchFor(text, [{ op: 'delete', line: 2, lineEnd: 4 }]),
    options,
  );
  t.is(deleted.newText, 'a\ne\n');
});

test('two ops consuming the same line fail patch-syntax', t => {
  const text = 'a\nb\nc\n';
  const patch = patchFor(text, [
    { op: 'replace', line: 2, payload: ['x'] },
    { op: 'delete', line: 1, lineEnd: 2 },
  ]);
  const result = applyEditPatch(text, patch, options);
  t.false(result.success);
  t.is(failureOf(result).reason, 'patch-syntax');
});

test('an insert anchored inside a consumed range fails patch-syntax', t => {
  const text = 'a\nb\nc\nd\n';
  const patch = patchFor(text, [
    { op: 'delete', line: 2, lineEnd: 3 },
    { op: 'insert-after', line: 3, payload: ['x'] },
  ]);
  const result = applyEditPatch(text, patch, options);
  t.false(result.success);
  t.is(failureOf(result).reason, 'patch-syntax');
});

test('prepend and append land first and last', t => {
  const text = 'middle\n';
  const patch = patchFor(text, [
    { op: 'insert-before', line: 1, payload: ['above middle'] },
    { op: 'insert-after', line: 1, payload: ['below middle'] },
    { op: 'prepend', payload: ['top'] },
    { op: 'append', payload: ['bottom'] },
  ]);
  const result = applyEditPatch(text, patch, options);
  t.true(result.success);
  t.is(result.newText, 'top\nabove middle\nmiddle\nbelow middle\nbottom\n');
});

test('multiple prepends and appends keep patch order', t => {
  const text = 'x\n';
  const patch = patchFor(text, [
    { op: 'prepend', payload: ['p1'] },
    { op: 'prepend', payload: ['p2'] },
    { op: 'append', payload: ['a1'] },
    { op: 'append', payload: ['a2'] },
  ]);
  const result = applyEditPatch(text, patch, options);
  t.is(result.newText, 'p1\np2\nx\na1\na2\n');
});

test('populating the empty file via append', t => {
  const patch = {
    expectedFileHash: EMPTY_FILE_SHA256,
    ops: [{ op: 'append', payload: ['first line'] }],
  };
  const result = applyEditPatch('', patch, options);
  t.true(result.success);
  t.is(result.newText, 'first line\n');
});

test('CRLF content survives a splice on another line', t => {
  const text = 'a\r\nb\r\nc\r\n';
  const patch = patchFor(text, [{ op: 'replace', line: 2, payload: ['B'] }]);
  const result = applyEditPatch(text, patch, options);
  t.true(result.success);
  // Untouched lines keep their CR; the new line is bare LF.
  t.is(result.newText, 'a\r\nB\nc\r\n');
});

test('a missing trailing newline is preserved', t => {
  const text = 'a\nb';
  const patch = patchFor(text, [{ op: 'replace', line: 1, payload: ['A'] }]);
  const result = applyEditPatch(text, patch, options);
  t.is(result.newText, 'A\nb');
});

test('deleting every line yields the empty file', t => {
  const text = 'a\nb\n';
  const patch = patchFor(text, [{ op: 'delete', line: 1, lineEnd: 2 }]);
  const result = applyEditPatch(text, patch, options);
  t.is(result.newText, '');
  t.is(result.fileHashAfter, EMPTY_FILE_SHA256);
});

// --- reapply ---

test('reapply relocates a drifted anchor', t => {
  // The agent authored against 'a\nb\ntarget\nc\n', but two lines
  // were inserted above, and the file hash was refreshed (reapply
  // does not relax the CAS).
  const drifted = 'x\ny\na\nb\ntarget\nc\n';
  const anchor = {
    line: 3,
    hash: lineAnchorHash('target', 3, 2),
  };
  const patch = {
    expectedFileHash: hashOfText(drifted),
    ops: [{ op: 'replace', anchor, payload: ['TARGET'] }],
  };
  const strict = applyEditPatch(drifted, patch, options);
  t.false(strict.success);
  t.is(failureOf(strict).reason, 'hash-mismatch');
  const relocated = applyEditPatch(drifted, patch, {
    ...options,
    reapply: true,
  });
  t.true(relocated.success, JSON.stringify(relocated.failure));
  t.is(relocated.newText, 'x\ny\na\nb\nTARGET\nc\n');
});

test('reapply hash checks at the relocated line strip the seed', t => {
  // A blank-line anchor cannot relocate: its hash is seeded with the
  // original line number, so no other blank line matches it.
  const drifted = 'inserted\na\n\nb\n';
  const patch = {
    expectedFileHash: hashOfText(drifted),
    // Anchor for the blank line when it sat at line 2.
    ops: [{ op: 'delete', anchor: { line: 2, hash: lineAnchorHash('', 2) } }],
  };
  const result = applyEditPatch(drifted, patch, {
    ...options,
    reapply: true,
  });
  // The blank line now sits at line 3, and its live hash is seeded
  // with 3, so the line-2-seeded anchor finds no candidate — unless
  // some other line's content hash collides, which `a` and `b` and
  // `inserted` are chosen not to.
  t.false(result.success);
  t.is(failureOf(result).reason, 'hash-mismatch');
});

test('reapply fails ambiguous-reapply on duplicate candidates', t => {
  const drifted = 'pad\ndup\nmid\ndup\n';
  const patch = {
    expectedFileHash: hashOfText(drifted),
    // An anchor for `dup` authored at a line where it no longer sits.
    ops: [
      {
        op: 'replace',
        anchor: { line: 3, hash: lineAnchorHash('dup', 3, 2) },
        payload: ['DUP'],
      },
    ],
  };
  const result = applyEditPatch(drifted, patch, {
    ...options,
    reapply: true,
  });
  t.false(result.success);
  const failure = failureOf(result);
  t.is(failure.reason, 'ambiguous-reapply');
  // Nearest-first, lower line first on ties: distance 1 (lines 2, 4).
  t.deepEqual(failure.ambiguities, [{ line: 3, candidates: [2, 4] }]);
});

test('reapply does not search beyond the window', t => {
  const filler = Array.from({ length: 60 }, (_, i) => `filler ${i}`);
  const text = `target\n${filler.join('\n')}\n`;
  const patch = {
    expectedFileHash: hashOfText(text),
    // Authored as if `target` sat at line 40; it sits at line 1,
    // outside a ±20 window around 40... except line 40-20=20 > 1.
    ops: [
      {
        op: 'replace',
        anchor: { line: 40, hash: lineAnchorHash('target', 40, 2) },
        payload: ['TARGET'],
      },
    ],
  };
  const strict = applyEditPatch(text, patch, { ...options, reapply: true });
  t.false(strict.success);
  t.is(failureOf(strict).reason, 'hash-mismatch');
  const wide = applyEditPatch(text, patch, {
    ...options,
    reapply: true,
    reapplyWindow: 100,
  });
  t.true(wide.success);
});

test('reapply does not relax the file-level CAS', t => {
  const text = 'a\n';
  const patch = {
    expectedFileHash: EMPTY_FILE_SHA256,
    ops: [{ op: 'append', payload: ['x'] }],
  };
  const result = applyEditPatch(text, patch, { ...options, reapply: true });
  t.false(result.success);
  t.is(failureOf(result).reason, 'file-rev-mismatch');
});

test('applyEditPatch validates its options', t => {
  const text = 'a\n';
  const patch = patchFor(text, [{ op: 'append', payload: ['x'] }]);
  t.throws(() =>
    applyEditPatch(
      text,
      patch,
      /** @type {import('../src/hashline.js').ApplyEditOptions} */ ({}),
    ),
  );
  t.throws(() => applyEditPatch(text, patch, { ...options, reapplyWindow: 0 }));
  t.throws(() =>
    applyEditPatch(text, patch, { ...options, reapplyWindow: 201 }),
  );
});

// --- round-trip: render, author, apply ---

test('a patch authored from the rendered view applies cleanly', t => {
  const text = 'alpha\nbeta\ngamma\n';
  const rendered = renderHashlineLines(text);
  // Recover the anchor for `beta` from the rendered annotation.
  const match = /^\s*(\d+)#([0-9a-f]+) beta$/.exec(rendered[1]);
  t.truthy(match);
  const [, lineText, hash] = /** @type {RegExpExecArray} */ (match);
  const patchText = [
    `@expected-file-hash ${hashOfText(text)}`,
    `@replace ${lineText}#${hash}`,
    '| BETA',
    '',
  ].join('\n');
  const patch = parseHashlineText(patchText);
  const result = applyEditPatch(text, patch, options);
  t.true(result.success);
  t.is(result.newText, 'alpha\nBETA\ngamma\n');
});
