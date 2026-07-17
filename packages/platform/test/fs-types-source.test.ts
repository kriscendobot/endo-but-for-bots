import type { PassableBytesReader } from '@endo/exo-stream';
import type {
  ContentStore as PackageContentStore,
  ContentStoreBlob as PackageContentStoreBlob,
  ReadableBlob as PackageReadableBlob,
  ReadableBlobRange as PackageReadableBlobRange,
  ReadableBlobRangeRead as PackageReadableBlobRangeRead,
} from '@endo/platform/fs/lite/types';
import type {
  ReadableBlob as PackageJsReadableBlob,
  ReadableBlobRange as PackageJsReadableBlobRange,
} from '@endo/platform/fs/lite/types.js';
import type {
  Search as PackageSearch,
  SearchFilePowers as PackageSearchFilePowers,
} from '@endo/platform/fs/search.types';
import type {
  Search as PackageJsSearch,
  SearchFilePowers as PackageJsSearchFilePowers,
} from '@endo/platform/fs/search.types.js';
import type { Reader } from '@endo/stream';

import type {
  DirEntry as PackageBackendDirEntry,
  FsBackend as PackageBackendFsBackend,
  NodeKind as PackageBackendNodeKind,
  WatchEvent as PackageBackendWatchEvent,
} from '@endo/platform/fs/extended/backend-types';
import type {
  DirEntry as PackageJsBackendDirEntry,
  FsBackend as PackageJsBackendFsBackend,
  NodeKind as PackageJsBackendNodeKind,
  WatchEvent as PackageJsBackendWatchEvent,
} from '@endo/platform/fs/extended/backend-types.js';
import type {
  DirEntry as PackageExtendedDirEntry,
  FsBackend as PackageExtendedFsBackend,
  NodeKind as PackageExtendedNodeKind,
  WatchEvent as PackageExtendedWatchEvent,
} from '@endo/platform/fs/extended';
import type {
  DirEntry as PackageJsExtendedDirEntry,
  FsBackend as PackageJsExtendedFsBackend,
  NodeKind as PackageJsExtendedNodeKind,
  WatchEvent as PackageJsExtendedWatchEvent,
} from '@endo/platform/fs/extended/types-index.js';
import type {
  ContentStore as SourceContentStore,
  ContentStoreBlob as SourceContentStoreBlob,
  ReadableBlob as SourceReadableBlob,
  ReadableBlobRange as SourceReadableBlobRange,
  ReadableBlobRangeRead as SourceReadableBlobRangeRead,
} from '../src/fs/types.js';
import type {
  Search as SourceSearch,
  SearchFilePowers as SourceSearchFilePowers,
} from '../src/fs/search.types.js';
import type {
  DirEntry as SourceBackendDirEntry,
  FsBackend as SourceBackendFsBackend,
  NodeKind as SourceBackendNodeKind,
  WatchEvent as SourceBackendWatchEvent,
} from '../src/fs/extended/backend-types.js';

type Equal<Left, Right> =
  (<T>() => T extends Left ? 1 : 2) extends <T>() => T extends Right ? 1 : 2
    ? true
    : false;

type Assert<T extends true> = T;

type ExpectedReadableBlob = {
  streamBase64: (synPromise: unknown) => Promise<unknown>;
  text: () => Promise<string>;
  json: () => Promise<any>;
  help: (method?: string) => string;
};

type ExpectedContentStoreBlob = {
  makeFileReader: () => Reader<Uint8Array>;
  text: () => Promise<string>;
  json: () => Promise<any>;
  size?: () => Promise<bigint>;
  readRange?: (offset: number, length: number) => Promise<Uint8Array>;
};

type _PackageReadableBlobMatchesSource = Assert<
  Equal<PackageReadableBlob, SourceReadableBlob>
>;
type _PackageJsReadableBlobMatchesSource = Assert<
  Equal<PackageJsReadableBlob, SourceReadableBlob>
>;
type _ReadableBlobSurfaceIsExact = Assert<
  Equal<SourceReadableBlob, ExpectedReadableBlob>
>;

type _PackageReadableBlobRangeMatchesSource = Assert<
  Equal<PackageReadableBlobRange, SourceReadableBlobRange>
>;
type _PackageJsReadableBlobRangeMatchesSource = Assert<
  Equal<PackageJsReadableBlobRange, SourceReadableBlobRange>
>;
type _ReadableBlobRangeSurfaceIsExact = Assert<
  Equal<
    keyof SourceReadableBlobRange,
    keyof ExpectedReadableBlob | 'getInfo' | 'fetch'
  >
>;
type _ReadableBlobRangeFetchIsStreaming = Assert<
  Equal<
    SourceReadableBlobRange['fetch'],
    (offset: bigint, length: bigint) => Promise<PassableBytesReader>
  >
>;

type _PackageReadableBlobRangeReadMatchesSource = Assert<
  Equal<PackageReadableBlobRangeRead, SourceReadableBlobRangeRead>
>;
type _ReadableBlobRangeReadSurfaceIsExact = Assert<
  Equal<
    keyof SourceReadableBlobRangeRead,
    keyof SourceReadableBlobRange | 'rangeRead' | 'rangeReadText'
  >
>;

type _PackageContentStoreBlobMatchesSource = Assert<
  Equal<PackageContentStoreBlob, SourceContentStoreBlob>
>;
type _ContentStoreBlobSurfaceIsExact = Assert<
  Equal<SourceContentStoreBlob, ExpectedContentStoreBlob>
>;
type _PackageContentStoreMatchesSource = Assert<
  Equal<PackageContentStore, SourceContentStore>
>;
type _ContentStoreFetchesHostBacking = Assert<
  Equal<ReturnType<SourceContentStore['fetch']>, SourceContentStoreBlob>
>;
type _PublicRangeBlobOmitsHostBackingHelpers = Assert<
  Equal<
    Extract<
      keyof SourceReadableBlobRangeRead,
      'makeFileReader' | 'size' | 'readRange'
    >,
    never
  >
>;

type _PackageSearchMatchesSource = Assert<Equal<PackageSearch, SourceSearch>>;
type _PackageJsSearchMatchesSource = Assert<
  Equal<PackageJsSearch, SourceSearch>
>;
type _PackageSearchPowersMatchSource = Assert<
  Equal<PackageSearchFilePowers, SourceSearchFilePowers>
>;
type _PackageJsSearchPowersMatchSource = Assert<
  Equal<PackageJsSearchFilePowers, SourceSearchFilePowers>
>;

type _PackageBackendDirEntryMatchesSource = Assert<
  Equal<PackageBackendDirEntry, SourceBackendDirEntry>
>;
type _PackageJsBackendDirEntryMatchesSource = Assert<
  Equal<PackageJsBackendDirEntry, SourceBackendDirEntry>
>;
type _PackageBackendFsBackendMatchesSource = Assert<
  Equal<PackageBackendFsBackend, SourceBackendFsBackend>
>;
type _PackageJsBackendFsBackendMatchesSource = Assert<
  Equal<PackageJsBackendFsBackend, SourceBackendFsBackend>
>;
type _PackageBackendNodeKindMatchesSource = Assert<
  Equal<PackageBackendNodeKind, SourceBackendNodeKind>
>;
type _PackageJsBackendNodeKindMatchesSource = Assert<
  Equal<PackageJsBackendNodeKind, SourceBackendNodeKind>
>;
type _PackageBackendWatchEventMatchesSource = Assert<
  Equal<PackageBackendWatchEvent, SourceBackendWatchEvent>
>;
type _PackageJsBackendWatchEventMatchesSource = Assert<
  Equal<PackageJsBackendWatchEvent, SourceBackendWatchEvent>
>;
type _PackageExtendedDirEntryMatchesSource = Assert<
  Equal<PackageExtendedDirEntry, SourceBackendDirEntry>
>;
type _PackageJsExtendedDirEntryMatchesSource = Assert<
  Equal<PackageJsExtendedDirEntry, SourceBackendDirEntry>
>;
type _PackageExtendedFsBackendMatchesSource = Assert<
  Equal<PackageExtendedFsBackend, SourceBackendFsBackend>
>;
type _PackageJsExtendedFsBackendMatchesSource = Assert<
  Equal<PackageJsExtendedFsBackend, SourceBackendFsBackend>
>;
type _PackageExtendedNodeKindMatchesSource = Assert<
  Equal<PackageExtendedNodeKind, SourceBackendNodeKind>
>;
type _PackageJsExtendedNodeKindMatchesSource = Assert<
  Equal<PackageJsExtendedNodeKind, SourceBackendNodeKind>
>;
type _PackageExtendedWatchEventMatchesSource = Assert<
  Equal<PackageExtendedWatchEvent, SourceBackendWatchEvent>
>;
type _PackageJsExtendedWatchEventMatchesSource = Assert<
  Equal<PackageJsExtendedWatchEvent, SourceBackendWatchEvent>
>;
