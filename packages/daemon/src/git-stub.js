// @ts-check
/**
 * XS stub for @endo/git. For the XS daemon, git operations use host functions
 * rather than Node child_process. This satisfies the bundler with no-op implementations.
 */

export const makeNativeGitBackend = (opts) => ({
  clone: async () => ({}),
  fetch: async () => {},
  checkout: async () => {},
  init: async () => {},
});
