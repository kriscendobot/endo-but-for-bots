/** A diagnostic that deliberately never includes untrusted regexp source. */
export class IRegexpError extends Error {
  /** @param {'syntax'|'unicode-property'|'ambiguous-repetition'|'resource-limit'} code */
  constructor(code) {
    super(`I-Regexp rejected: ${code}`);
    this.code = code;
  }
}

Object.freeze(IRegexpError);
