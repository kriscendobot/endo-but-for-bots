import test from '@endo/ses-ava/test.js';

import { bytesEqual } from '../src/equals.js';
import { concatBytes } from '../src/concat.js';
import { compareBytes } from '../src/compare.js';
import { assertGenuineUint8Array } from '../src/genuine-uint8-array.js';

// A stand-in for an emulated frozen byteArray wrapper: an object whose
// `[[Prototype]]` is `Uint8Array.prototype` (so `instanceof Uint8Array` is
// true) but which has no typed-array internal slot, so `wrapper[i]` reads as
// `undefined` rather than a byte. This is the shape the integer-indexed guard
// must reject.
const makeCounterfeitUint8Array = length => {
  const wrapper = Object.create(Uint8Array.prototype);
  Object.defineProperty(wrapper, 'length', { value: length });
  // Deliberately leave the indexed slots absent, mirroring the emulated path.
  return wrapper;
};

test('concatBytes: empty input yields empty Uint8Array', t => {
  const result = concatBytes([]);
  t.true(result instanceof Uint8Array);
  t.is(result.length, 0);
});

test('concatBytes: single chunk preserves bytes', t => {
  const a = new Uint8Array([1, 2, 3]);
  const result = concatBytes([a]);
  t.deepEqual([...result], [1, 2, 3]);
});

test('concatBytes: many small chunks preserves order', t => {
  const chunks = [
    new Uint8Array([1]),
    new Uint8Array([2, 3]),
    new Uint8Array([4, 5, 6]),
    new Uint8Array([7]),
  ];
  const result = concatBytes(chunks);
  t.deepEqual([...result], [1, 2, 3, 4, 5, 6, 7]);
});

test('concatBytes: zero-length chunks interleaved with non-empty', t => {
  const chunks = [
    new Uint8Array([]),
    new Uint8Array([1, 2]),
    new Uint8Array([]),
    new Uint8Array([3]),
    new Uint8Array([]),
  ];
  const result = concatBytes(chunks);
  t.deepEqual([...result], [1, 2, 3]);
});

test('concatBytes: lengths around 64-byte boundaries', t => {
  // Catches any future SIMD optimization that assumes alignment.
  for (const len of [63, 64, 65, 127, 128, 129]) {
    const chunk = new Uint8Array(len);
    for (let i = 0; i < len; i += 1) {
      chunk[i] = i % 256;
    }
    const result = concatBytes([chunk, chunk]);
    t.is(result.length, len * 2);
    for (let i = 0; i < len; i += 1) {
      t.is(result[i], i % 256);
      t.is(result[i + len], i % 256);
    }
  }
});

test('concatBytes: huge chunk plus zero-length chunks', t => {
  const big = new Uint8Array(4096);
  for (let i = 0; i < big.length; i += 1) {
    big[i] = i % 256;
  }
  const result = concatBytes([new Uint8Array([]), big, new Uint8Array([])]);
  t.is(result.length, 4096);
  for (let i = 0; i < 4096; i += 1) {
    t.is(result[i], i % 256);
  }
});

test('bytesEqual: identical reference', t => {
  const a = new Uint8Array([1, 2, 3]);
  t.true(bytesEqual(a, a));
});

test('bytesEqual: identical contents different references', t => {
  const a = new Uint8Array([1, 2, 3]);
  const b = new Uint8Array([1, 2, 3]);
  t.true(bytesEqual(a, b));
});

test('bytesEqual: different lengths', t => {
  const a = new Uint8Array([1, 2, 3]);
  const b = new Uint8Array([1, 2]);
  t.false(bytesEqual(a, b));
});

test('bytesEqual: same prefix different suffix', t => {
  const a = new Uint8Array([1, 2, 3, 4]);
  const b = new Uint8Array([1, 2, 3, 5]);
  t.false(bytesEqual(a, b));
});

test('bytesEqual: empty arrays compare equal', t => {
  t.true(bytesEqual(new Uint8Array(), new Uint8Array()));
});

test('bytesEqual: differs at first byte', t => {
  const a = new Uint8Array([0, 1, 2]);
  const b = new Uint8Array([1, 1, 2]);
  t.false(bytesEqual(a, b));
});

test('compareBytes: equal, less, and greater on genuine Uint8Arrays', t => {
  t.is(compareBytes(new Uint8Array([1, 2, 3]), new Uint8Array([1, 2, 3])), 0);
  t.is(compareBytes(new Uint8Array([1, 2]), new Uint8Array([1, 2, 3])), -1);
  t.is(compareBytes(new Uint8Array([1, 3]), new Uint8Array([1, 2, 3])), 1);
});

test('compareBytes: subrange comparison stays allocation free', t => {
  const buffer = new Uint8Array([0, 1, 2, 1, 2, 9]);
  // Compare bytes [1,2] (indices 1..3) against bytes [1,2] (indices 3..5).
  t.is(compareBytes(buffer, buffer, 1, 3, 3, 5), 0);
});

// The whole point of erights's review on PR #475: a reader that takes a
// counterfeit integer-indexed Uint8Array must reject it loudly, not complete
// successfully with the wrong answer and no diagnostic.
test('assertGenuineUint8Array: accepts a genuine Uint8Array', t => {
  t.notThrows(() => assertGenuineUint8Array(new Uint8Array([1, 2, 3])));
  t.notThrows(() => assertGenuineUint8Array(new Uint8Array()));
});

test('assertGenuineUint8Array: rejects an emulated byteArray wrapper', t => {
  const counterfeit = makeCounterfeitUint8Array(3);
  // The stand-in superficially looks like bytes but reads as undefined.
  t.true(counterfeit instanceof Uint8Array);
  t.is(counterfeit[0], undefined);
  t.throws(() => assertGenuineUint8Array(counterfeit, 'left'), {
    instanceOf: TypeError,
    message: /left must be a genuine integer-indexed Uint8Array/,
  });
});

test('assertGenuineUint8Array: rejects other typed arrays and non-arrays', t => {
  t.throws(() => assertGenuineUint8Array(new Int8Array([1, 2, 3])), {
    instanceOf: TypeError,
    message: /a Int8Array/,
  });
  t.throws(() => assertGenuineUint8Array([1, 2, 3]), {
    instanceOf: TypeError,
  });
  t.throws(() => assertGenuineUint8Array({}), { instanceOf: TypeError });
  t.throws(() => assertGenuineUint8Array(42), { instanceOf: TypeError });
  t.throws(() => assertGenuineUint8Array(null), {
    instanceOf: TypeError,
    message: /got null/,
  });
});

test('compareBytes: rejects a counterfeit argument instead of lying', t => {
  const genuine = new Uint8Array([1, 2, 3]);
  const counterfeit = makeCounterfeitUint8Array(3);
  t.throws(() => compareBytes(counterfeit, genuine, 0, 3, 0, 3), {
    instanceOf: TypeError,
    message: /left must be a genuine integer-indexed Uint8Array/,
  });
  t.throws(() => compareBytes(genuine, counterfeit, 0, 3, 0, 3), {
    instanceOf: TypeError,
    message: /right must be a genuine integer-indexed Uint8Array/,
  });
});

test('bytesEqual: rejects a counterfeit argument instead of lying', t => {
  const genuine = new Uint8Array([1, 2, 3]);
  const counterfeit = makeCounterfeitUint8Array(3);
  t.throws(() => bytesEqual(counterfeit, genuine), {
    instanceOf: TypeError,
    message: /a must be a genuine integer-indexed Uint8Array/,
  });
  t.throws(() => bytesEqual(genuine, counterfeit), {
    instanceOf: TypeError,
    message: /b must be a genuine integer-indexed Uint8Array/,
  });
});

test('concatBytes: rejects a counterfeit chunk instead of copying zeros', t => {
  const genuine = new Uint8Array([1, 2, 3]);
  const counterfeit = makeCounterfeitUint8Array(2);
  t.throws(() => concatBytes([genuine, counterfeit]), {
    instanceOf: TypeError,
    message: /chunk must be a genuine integer-indexed Uint8Array/,
  });
});
