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
