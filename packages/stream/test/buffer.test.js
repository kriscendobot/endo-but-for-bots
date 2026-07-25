// @ts-check
import test from '@endo/ses-ava/test.js';
import { makePromiseKit } from '@endo/promise-kit';
import { makeBuffer } from '@endo/stream/buffer';

/** @import { BufferKit } from '@endo/stream/buffer' */

test('buffer delivers values pushed before and after a pull', async t => {
  await null;
  const { spring, sink } = makeBuffer();
  spring.next(1);
  t.deepEqual(await sink.next(), { value: 1, done: false });

  const result = sink.next();
  spring.next(Promise.resolve(2));
  t.deepEqual(await result, { value: 2, done: false });
});

test('buffer preserves queue positions while values settle independently', async t => {
  await null;
  const { spring, sink } = makeBuffer();
  const first = makePromiseKit();
  spring.next(first.promise);
  spring.next(2);

  const firstResult = sink.next();
  const secondResult = sink.next();
  t.deepEqual(await secondResult, { value: 2, done: false });

  first.resolve(1);
  t.deepEqual(await firstResult, { value: 1, done: false });
});

test('spring return delivers a terminal iterator result', async t => {
  await null;
  /** @type {BufferKit<string, string>} */
  const { spring, sink } = makeBuffer();
  spring.next('value');
  spring.return('finished');
  spring.next('ignored');

  t.deepEqual(await sink.next(), { value: 'value', done: false });
  t.deepEqual(await sink.next(), { value: 'finished', done: true });
  t.deepEqual(await sink.next(), { value: 'finished', done: true });
});

test('spring throw rejects once and then completes', async t => {
  const { spring, sink } = makeBuffer();
  const error = Error('finished with an error');
  spring.throw(error);

  await t.throwsAsync(sink.next(), { is: error });
  t.deepEqual(await sink.next(), { value: undefined, done: true });
});

test('sink return closes the spring and releases a pending pull', async t => {
  /** @type {BufferKit<string, string>} */
  const { spring, sink } = makeBuffer();
  const pending = sink.next();
  const terminal = await sink.return('stopped');
  spring.next('ignored');

  t.deepEqual(terminal, { value: 'stopped', done: true });
  t.deepEqual(await pending, terminal);
  t.deepEqual(await sink.next(), terminal);
});

test('sink throw closes the spring and rejects a pending pull', async t => {
  const { spring, sink } = makeBuffer();
  const pending = sink.next();
  const error = Error('consumer stopped');
  const terminal = sink.throw(error);
  spring.next('ignored');

  await t.throwsAsync(terminal, { is: error });
  await t.throwsAsync(pending, { is: error });
  await t.throwsAsync(sink.next(), { is: error });
});

test('buffer kit and its facets are hardened', t => {
  const { spring, sink } = makeBuffer();
  t.true(Object.isFrozen(spring));
  t.true(Object.isFrozen(sink));
});
