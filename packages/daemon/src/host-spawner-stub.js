// @ts-check
/**
 * XS stub for @endo/host-spawner. For the XS daemon, host spawning uses
 * Rust process primitives (host functions) rather than Node's child_process.
 */

export const makeHostSpawner = () => ({
  spawn: async () => ({ pid: 0 }),
  kill: async () => {},
});
