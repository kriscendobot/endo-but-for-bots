// @ts-check

// A bounded buffer deliberately has no export yet. Its synchronous,
// pre-allocated ring-buffer implementation belongs in this module rather than
// in unbounded-buffer.js: it can refuse reads or writes and can flush in bulk,
// so it has different mechanics and semantics from the promise-queue buffer.

export {};
