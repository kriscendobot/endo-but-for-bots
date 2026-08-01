import test from 'ava';

import cases from './i-regexp-profile-cases.json' with { type: 'json' };
import {
  IRegexpError,
  contains,
  compile,
  isConservativeRegex,
  matches,
  parseIRegexp,
} from '../src/index.js';

test('profile validity corpus', t => {
  for (const entry of cases.validity) {
    if (entry.accepted) {
      t.notThrows(() => parseIRegexp(entry.source), entry.source);
      t.true(isConservativeRegex(entry.source), entry.source);
    } else {
      const error = t.throws(() => parseIRegexp(entry.source), {
        instanceOf: IRegexpError,
      });
      if (entry.reason === undefined)
        throw Error('rejected corpus case needs a reason');
      t.is(
        error.code,
        /** @type {IRegexpError['code']} */ (entry.reason),
        entry.source,
      );
      t.false(isConservativeRegex(entry.source), entry.source);
    }
  }
});

test('profile match corpus', t => {
  for (const entry of cases.matches) {
    t.is(
      matches(parseIRegexp(entry.source), entry.text),
      entry.matches,
      entry.source,
    );
  }
});

test('contains corpus', t => {
  for (const entry of cases.contains) {
    t.is(
      matches(contains(parseIRegexp(entry.source)), entry.text),
      entry.matches,
      entry.source,
    );
  }
});

test('compile parses once and does not accept forged patterns', t => {
  t.true(compile('a+').test('aaa'));
  t.throws(() => matches({ javascript: '.*' }, 'anything'), {
    instanceOf: TypeError,
  });
});
