// @ts-nocheck
import test from '@endo/ses-ava/prepare-endo.js';

import { makeXsFilePowers } from '../src/bus-manager-rust-xs-powers.js';

/**
 * The XS FilePowers reference the Rust supervisor's host callbacks as free
 * globals.  These tests install deterministic stubs on `globalThis` so the JS
 * adapter — the async-iterator contract, the host error round-trip, and the
 * watch-handle lifecycle — is exercised in-process, without a live XS worker.
 * (The live path is covered by the Rust `fs_watch_dir` test; this pins the JS
 * glue that no supervisor test reaches.)
 *
 * @param {import('ava').ExecutionContext} t
 * @param {{ hostWatchDirectory: Function, hostWatchNext: Function, hostWatchClose: Function }} stubs
 */
const withHostWatchStubs = (t, stubs) => {
  const g = /** @type {any} */ (globalThis);
  const saved = {
    hostWatchDirectory: g.hostWatchDirectory,
    hostWatchNext: g.hostWatchNext,
    hostWatchClose: g.hostWatchClose,
  };
  g.hostWatchDirectory = stubs.hostWatchDirectory;
  g.hostWatchNext = stubs.hostWatchNext;
  g.hostWatchClose = stubs.hostWatchClose;
  t.teardown(() => {
    g.hostWatchDirectory = saved.hostWatchDirectory;
    g.hostWatchNext = saved.hostWatchNext;
    g.hostWatchClose = saved.hostWatchClose;
  });
};

test('XS watchDirectory scopes to the root token and streams add/replace/remove', async t => {
  const responses = [
    JSON.stringify([{ kind: 'add', name: 'entry.txt' }]),
    JSON.stringify([
      { kind: 'replace', name: 'entry.txt' },
      { kind: 'remove', name: 'gone.txt' },
    ]),
  ];
  let dirArgs;
  let closedHandle;
  withHostWatchStubs(t, {
    hostWatchDirectory: (...args) => {
      dirArgs = args;
      return 7;
    },
    // A regression here (unexpected extra poll) surfaces as a thrown error
    // rather than an infinite loop that would time the test out.
    hostWatchNext: () =>
      responses.length ? responses.shift() : 'Error: unexpected extra poll',
    hostWatchClose: handle => {
      closedHandle = handle;
    },
  });

  const powers = makeXsFilePowers();
  const { events, cancel } = powers.watchDirectory('/root/watched');
  t.deepEqual(
    dirArgs,
    ['root', 'root/watched'],
    'the watch is capability-scoped to the root token with a root-relative path',
  );

  const iterator = events[Symbol.asyncIterator]();
  const first = await iterator.next();
  t.false(first.done);
  t.deepEqual(first.value, { kind: 'add', name: 'entry.txt' });

  // The second poll returns two changes; the iterator drains the buffer across
  // successive next() calls without re-polling the host for the buffered one.
  const second = await iterator.next();
  t.deepEqual(second.value, { kind: 'replace', name: 'entry.txt' });
  const third = await iterator.next();
  t.deepEqual(third.value, { kind: 'remove', name: 'gone.txt' });

  cancel();
  t.is(closedHandle, 7, 'cancel closes the host watch handle');
});

test('XS watchDirectory throws synchronously when the host cannot start the watch', async t => {
  withHostWatchStubs(t, {
    hostWatchDirectory: () => 'Error: not a directory',
    hostWatchNext: () => '[]',
    hostWatchClose: () => {},
  });
  const powers = makeXsFilePowers();
  t.throws(() => powers.watchDirectory('/root/missing'), {
    message: /not a directory/,
  });
});

test('XS watchDirectory closes the handle and throws on a mid-stream host error', async t => {
  let closeCalls = 0;
  withHostWatchStubs(t, {
    hostWatchDirectory: () => 3,
    hostWatchNext: () => 'Error: watch failed',
    hostWatchClose: () => {
      closeCalls += 1;
    },
  });
  const powers = makeXsFilePowers();
  const { events } = powers.watchDirectory('/root/watched');
  const iterator = events[Symbol.asyncIterator]();
  await t.throwsAsync(() => iterator.next(), { message: /watch failed/ });
  t.is(closeCalls, 1, 'the handle is closed exactly once on a host error');
});

test('XS watchDirectory closes the handle when the host returns malformed JSON', async t => {
  let closeCalls = 0;
  withHostWatchStubs(t, {
    hostWatchDirectory: () => 9,
    hostWatchNext: () => 'not json',
    hostWatchClose: () => {
      closeCalls += 1;
    },
  });
  const powers = makeXsFilePowers();
  const { events } = powers.watchDirectory('/root/watched');
  const iterator = events[Symbol.asyncIterator]();
  await t.throwsAsync(() => iterator.next());
  t.is(closeCalls, 1, 'a malformed payload does not leak the watch handle');
});

test('XS watchDirectory cancel and return() are idempotent', async t => {
  let closeCalls = 0;
  withHostWatchStubs(t, {
    hostWatchDirectory: () => 5,
    hostWatchNext: () => '[]',
    hostWatchClose: () => {
      closeCalls += 1;
    },
  });
  const powers = makeXsFilePowers();
  const { events, cancel } = powers.watchDirectory('/root/watched');
  const iterator = events[Symbol.asyncIterator]();
  const ended = await iterator.return();
  t.true(ended.done, 'return() ends the stream');
  cancel();
  cancel();
  t.is(closeCalls, 1, 'the host watch handle is closed exactly once');
});
