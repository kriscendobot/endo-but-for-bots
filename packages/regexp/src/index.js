// @ts-check

import { IRegexpError } from './errors.js';

export { IRegexpError } from './errors.js';

/** The public, corpus-visible limits of the Endo I-Regexp v1 profile. */
export const profileLimits = Object.freeze({
  sourceScalars: 4096,
  astNodes: 1024,
  repetitionNesting: 8,
  rangeQuantifier: 1000,
});

const freeze = Object.freeze;
/** @type {WeakMap<object, { expression: any, javascript: string }>} */
const parsedRecords = new WeakMap();

/** @param {'syntax'|'unicode-property'|'ambiguous-repetition'|'resource-limit'} code */
const reject = code => {
  throw new IRegexpError(code);
};

/**
 * @template T
 * @param {T} value
 * @returns {T}
 */
const deepFreeze = value => {
  if (value && typeof value === 'object' && !Object.isFrozen(value)) {
    for (const child of Object.values(value)) {
      deepFreeze(child);
    }
    freeze(value);
  }
  return value;
};

/** @param {string} source */
const scalars = source => {
  const result = [];
  for (let index = 0; index < source.length; index += 1) {
    const unit = source.charCodeAt(index);
    if (unit >= 0xd800 && unit <= 0xdbff) {
      const next = source.charCodeAt(index + 1);
      if (next < 0xdc00 || next > 0xdfff) reject('syntax');
      result.push(source.slice(index, index + 2));
      index += 1;
    } else if (unit >= 0xdc00 && unit <= 0xdfff) {
      reject('syntax');
    } else {
      result.push(source[index]);
    }
  }
  return result;
};

/** @param {string} character */
const isNormal = character => {
  const point = character.codePointAt(0);
  if (point === undefined) return false;
  return (
    point <= 0x27 ||
    character === ',' ||
    character === '-' ||
    (point >= 0x2f && point <= 0x3e) ||
    (point >= 0x40 && point <= 0x5a) ||
    (point >= 0x5e && point <= 0x7a) ||
    point >= 0x7e
  );
};

/** @param {string} character */
const isClassNormal = character => {
  const point = character.codePointAt(0);
  if (point === undefined) return false;
  return point <= 0x2c || (point >= 0x2e && point <= 0x5a) || point >= 0x5e;
};

/** @param {string} character */
const codePoint = character => character.codePointAt(0) || 0;

class Parser {
  /** @param {string} source */
  constructor(source) {
    this.chars = scalars(source);
    this.index = 0;
    this.nodes = 0;
  }

  peek() {
    return this.chars[this.index];
  }

  take() {
    const character = this.peek();
    if (character === undefined) reject('syntax');
    this.index += 1;
    return character;
  }

  node(value) {
    this.nodes += 1;
    if (this.nodes > profileLimits.astNodes) reject('resource-limit');
    return value;
  }

  parse() {
    const ast = this.alternation();
    if (this.peek() !== undefined) reject('syntax');
    return ast;
  }

  alternation() {
    const branches = [this.branch()];
    while (this.peek() === '|') {
      this.take();
      branches.push(this.branch());
    }
    return this.node({ type: 'alternation', branches });
  }

  branch() {
    const pieces = [];
    while (this.peek() !== undefined && this.peek() !== ')' && this.peek() !== '|') {
      pieces.push(this.piece());
    }
    return this.node({ type: 'branch', pieces });
  }

  piece() {
    const atom = this.atom();
    let quantifier;
    const character = this.peek();
    if (character === '*' || character === '+' || character === '?') {
      this.take();
      quantifier = { min: character === '+' ? 1 : 0, max: character === '?' ? 1 : null };
    } else if (character === '{') {
      quantifier = this.rangeQuantifier();
    }
    return this.node({ type: 'piece', atom, quantifier });
  }

  rangeQuantifier() {
    this.take();
    const minimum = this.number();
    /** @type {number|null} */
    let maximum = minimum;
    if (this.peek() === ',') {
      this.take();
      maximum = this.peek() === '}' ? null : this.number();
    }
    if (this.take() !== '}') reject('syntax');
    if (
      minimum > profileLimits.rangeQuantifier ||
      (maximum !== null && maximum > profileLimits.rangeQuantifier)
    ) {
      reject('resource-limit');
    }
    if (maximum !== null && minimum > maximum) reject('syntax');
    return { min: minimum, max: maximum };
  }

  number() {
    let value = '';
    while (/^[0-9]$/.test(this.peek() || '')) value += this.take();
    if (value === '') reject('syntax');
    const number = Number(value);
    if (!Number.isSafeInteger(number)) reject('resource-limit');
    return number;
  }

  atom() {
    const character = this.peek();
    if (character === '(') {
      this.take();
      const expression = this.alternation();
      if (this.take() !== ')') reject('syntax');
      return this.node({ type: 'group', expression });
    }
    if (character === '[') return this.classExpression();
    if (character === '.') {
      this.take();
      return this.node({ type: 'dot' });
    }
    if (character === '\\') return this.escape(false);
    if (character === undefined || !isNormal(character)) reject('syntax');
    this.take();
    return this.node({ type: 'literal', value: character });
  }

  escape(inClass) {
    this.take();
    const character = this.take();
    if (character === 'p' || character === 'P') {
      if (this.peek() === '{') {
        while (this.peek() !== undefined && this.take() !== '}') {
          // Consume the property only to report its profile-specific diagnostic.
        }
      }
      reject('unicode-property');
    }
    const escaped = ['(', ')', '*', '+', '-', '.', '?', '[', '\\', ']', '^', '{', '}'];
    if (escaped.includes(character)) return this.node({ type: 'literal', value: character });
    if (character === 'n') return this.node({ type: 'literal', value: '\n' });
    if (character === 'r') return this.node({ type: 'literal', value: '\r' });
    if (character === 't') return this.node({ type: 'literal', value: '\t' });
    if (!inClass && character === '|') return this.node({ type: 'literal', value: '|' });
    return reject('syntax');
  }

  classExpression() {
    this.take();
    const negated = this.peek() === '^';
    if (negated) this.take();
    const entries = [];
    if (this.peek() === '-') {
      this.take();
      entries.push(this.node({ type: 'literal', value: '-' }));
    } else {
      entries.push(this.classEntry());
    }
    while (this.peek() !== ']') {
      if (this.peek() === undefined) reject('syntax');
      entries.push(this.classEntry());
    }
    this.take();
    if (entries.length === 1 && entries[0].value === '^' && !negated) reject('syntax');
    return this.node({ type: 'class', negated, entries });
  }

  classEntry() {
    const first = this.classAtom();
    if (this.peek() !== '-') return first;
    this.take();
    if (this.peek() === ']') return this.node({ type: 'literal', value: `${first.value}-` });
    const last = this.classAtom();
    if (codePoint(first.value) > codePoint(last.value)) reject('syntax');
    return this.node({ type: 'range', first: first.value, last: last.value });
  }

  classAtom() {
    const character = this.peek();
    if (character === '\\') return this.escape(true);
    if (character === undefined || character === ']' || character === '-' || !isClassNormal(character)) {
      reject('syntax');
    }
    this.take();
    return this.node({ type: 'literal', value: character });
  }
}

/** @param {any} atom */
const nullable = atom => {
  if (atom.type === 'literal' || atom.type === 'dot' || atom.type === 'class') return false;
  if (atom.type === 'group') return atom.expression.branches.some(nullableBranch);
  return false;
};

/** @param {any} branch */
const nullableBranch = branch =>
  branch.pieces.every(piece => piece.quantifier && piece.quantifier.min === 0);

/** @param {any} atom */
const leadingBlocks = atom => {
  if (atom.type === 'literal') return [atom.value];
  if (atom.type === 'dot' || atom.type === 'class') return null;
  if (atom.type === 'group') {
    const blocks = [];
    for (const branch of atom.expression.branches) {
      const block = leadingBlock(branch);
      if (block === null) return null;
      blocks.push(block);
    }
    return blocks;
  }
  return null;
};

/** @param {any} branch */
const leadingBlock = branch => {
  let block = '';
  for (const piece of branch.pieces) {
    if (piece.quantifier) return block || null;
    if (piece.atom.type !== 'literal') return block || null;
    block += piece.atom.value;
  }
  return block || null;
};

/** @param {any} atom */
const hasQuantifier = atom => {
  if (atom.type !== 'group') return false;
  return atom.expression.branches.some(branch =>
    branch.pieces.some(piece => piece.quantifier || hasQuantifier(piece.atom)),
  );
};

/** @param {string[]} blocks */
const prefixOverlap = blocks =>
  blocks.some((block, index) =>
    blocks.some(
      (other, otherIndex) =>
        index !== otherIndex && (block.startsWith(other) || other.startsWith(block)),
    ),
  );

/**
 * @param {any} expression
 * @param {number} nesting
 */
const validateSafety = (expression, nesting = 0) => {
  for (const branch of expression.branches) {
    for (const piece of branch.pieces) {
      if (piece.atom.type === 'group') validateSafety(piece.atom.expression, nesting);
      if (piece.quantifier) {
        if (nesting >= profileLimits.repetitionNesting) reject('resource-limit');
        const blocks = leadingBlocks(piece.atom);
        if (nullable(piece.atom) || hasQuantifier(piece.atom)) reject('ambiguous-repetition');
        if (blocks && prefixOverlap(blocks)) reject('ambiguous-repetition');
        if (piece.atom.type === 'group') validateSafety(piece.atom.expression, nesting + 1);
      }
    }
  }
};

/**
 * @param {string} character
 * @param {boolean} inClass
 */
const jsCharacter = (character, inClass = false) => {
  const point = codePoint(character);
  if (point < 0x20 || point === 0x7f || point > 0x7e) return `\\u{${point.toString(16)}}`;
  if (character === '\\' || (inClass && (character === ']' || character === '^' || character === '-'))) {
    return `\\${character}`;
  }
  if (!inClass && '^$.*+?()[]{}|'.includes(character)) return `\\${character}`;
  return character;
};

/** @param {any} atom */
const serializeAtom = atom => {
  if (atom.type === 'literal') return jsCharacter(atom.value);
  if (atom.type === 'dot') return '[^\\n\\r]';
  if (atom.type === 'class') {
    const entries = atom.entries
      .map(entry =>
        entry.type === 'range'
          ? `${jsCharacter(entry.first, true)}-${jsCharacter(entry.last, true)}`
          : [...entry.value].map(character => jsCharacter(character, true)).join(''),
      )
      .join('');
    return `[${atom.negated ? '^' : ''}${entries}]`;
  }
  return `(?:${serializeAlternation(atom.expression)})`;
};

/** @param {{min: number, max: number|null}} quantifier */
const serializeQuantifier = quantifier => {
  if (quantifier.min === 0 && quantifier.max === null) return '*';
  if (quantifier.min === 1 && quantifier.max === null) return '+';
  if (quantifier.min === 0 && quantifier.max === 1) return '?';
  if (quantifier.min === quantifier.max) return `{${quantifier.min}}`;
  return quantifier.max === null
    ? `{${quantifier.min},}`
    : `{${quantifier.min},${quantifier.max}}`;
};

/** @param {any} branch */
const serializeBranch = branch =>
  branch.pieces
    .map(piece => `${serializeAtom(piece.atom)}${piece.quantifier ? serializeQuantifier(piece.quantifier) : ''}`)
    .join('');

/** @param {any} expression */
const serializeAlternation = expression => expression.branches.map(serializeBranch).join('|');

/**
 * Parses and validates a source string as Endo I-Regexp v1.
 *
 * @param {string} source
 * @returns {{profile: string, expression: any, javascript: string}}
 */
export const parseIRegexp = source => {
  if (typeof source !== 'string') reject('syntax');
  const characters = scalars(source);
  if (characters.length > profileLimits.sourceScalars) reject('resource-limit');
  const parser = new Parser(source);
  const expression = parser.parse();
  validateSafety(expression);
  const parsed = deepFreeze({
    profile: 'endo-i-regexp-v1',
    expression,
    javascript: serializeAlternation(expression),
  });
  parsedRecords.set(parsed, { expression, javascript: parsed.javascript });
  return parsed;
};

/** @param {string} source */
export const isConservativeRegex = source => {
  try {
    parseIRegexp(source);
    return true;
  } catch (error) {
    if (error instanceof IRegexpError) return false;
    throw error;
  }
};

/**
 * @param {unknown} parsed
 * @param {string} text
 */
export const matches = (parsed, text) => {
  const record = parsedRecords.get(parsed);
  if (!record || typeof text !== 'string') {
    throw TypeError('matches requires a parsed I-Regexp and string text');
  }
  scalars(text);
  return new RegExp(`^(?:${record.javascript})$`, 'u').test(text);
};

/** @param {unknown} parsed */
export const contains = parsed => {
  const record = parsedRecords.get(parsed);
  if (!record) throw TypeError('contains requires a parsed I-Regexp');
  const expression = {
    type: 'alternation',
    branches: [
      {
        type: 'branch',
        pieces: [
          { type: 'piece', atom: { type: 'dot' }, quantifier: { min: 0, max: null } },
          { type: 'piece', atom: { type: 'group', expression: record.expression }, quantifier: undefined },
          { type: 'piece', atom: { type: 'dot' }, quantifier: { min: 0, max: null } },
        ],
      },
    ],
  };
  validateSafety(expression);
  const contained = deepFreeze({
    profile: 'endo-i-regexp-v1',
    expression,
    javascript: serializeAlternation(expression),
  });
  parsedRecords.set(contained, { expression, javascript: contained.javascript });
  return contained;
};

/** @param {string} source */
export const compile = source => {
  const parsed = parseIRegexp(source);
  return freeze({ test: text => matches(parsed, text) });
};

freeze(parseIRegexp);
freeze(isConservativeRegex);
freeze(matches);
freeze(contains);
freeze(compile);
