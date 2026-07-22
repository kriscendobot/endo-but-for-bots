// @ts-check
/**
 * XS-compatible stub for @endo/platform/fs/lite.
 * 
 * For the XS daemon, persistence operates through Rust host functions
 * rather than Node fs. These satisfy the bundler while delegating actual
 * work to the host side. Uses @endo/patterns (XS-compatible) for Shapes.
 */

import { M } from '@endo/patterns';
import harden from '@endo/harden';

// Guard shapes (minimal, matching the platform interface definitions)
const createInterface = name => M.interface(name, {}, { maxArgs: Infinity });

export const readableBlobMethodGuards = {};
export const readableTreeMethodGuards = {};
export const readableNameHubMethodGuards = {};
export const directoryFileMethodGuards = {};
export const pathEntryMethodGuards = {};
export const pathEntryIssuerMethodGuards = {};
export const getInfoMethodGuard = M.any();
export const rangeReadMethodGuards = {};

export const PathEntryInterface = createInterface('PathEntry');
export const PathEntryIssuerInterface = createInterface('PathEntryIssuer');
export const rangeReadConvenienceMethodGuards = {};
export const recursiveListMethodGuards = {};
export const ReadableBlobInterface = createInterface('ReadableBlob');
export const ReadableBlobRangeInterface = createInterface('ReadableBlobRange');
export const ReadableBlobRangeReadInterface = createInterface('ReadableBlobRangeRead');
export const SnapshotBlobInterface = createInterface('SnapshotBlob');
export const ReadableTreeInterface = createInterface('ReadableTree');
export const SnapshotTreeInterface = createInterface('SnapshotTree');
export const TreeWriterInterface = createInterface('TreeWriter');
export const FileInterface = createInterface('File');
export const DirectoryInterface = createInterface('Directory');

// No-op implementations for XS host-side persistence
export const snapshotBlobMethods = { read: async () => new Uint8Array(0) };
export const snapshotTreeMethods = { list: async () => [], stat: async () => ({ type: 'file' }) };

export const makeSnapshotStore = async () => harden({
  create: async () => {},
  exists: async () => false,
  read: async () => undefined,
  write: async () => {},
  delete: async () => {},
});

export const checkinTree = async () => ({});
export const checkoutTree = async () => ({});

export const makeSearch = () => ({ next: async () => ({ value: undefined, done: true }) });
export const provideSearch = () => makeSearch();
export const compileGlobSegment = () => {};
export const parseGlobPattern = () => ({ segments: [] });
export const DEFAULT_BATCH_SIZE = 1024;
export const MAX_BATCH_SIZE = 10240;
export const GLOB_MAX_RESULTS = 10000;
export const GREP_MAX_RESULTS = 10000;

export const makeMaybeRealPath = async () => '/';
export const isPathWithin = () => false;

/**
 * XS-compatible toSafeNumber helper (from @endo/platform/fs/extended/shared/helpers.js).
 * Validates a bigint is within safe Number range.
 */
export const toSafeNumber = (value, name) => {
  if (typeof value === 'bigint') {
    if (value < 0n) {
      throw new Error(`EINVAL: ${name} must be non-negative, got ${value}`);
    }
    if (value > BigInt(Number.MAX_SAFE_INTEGER)) {
      throw new Error(`EINVAL: ${name} exceeds Number.MAX_SAFE_INTEGER`);
    }
    return Number(value);
  }
  return value;
};
