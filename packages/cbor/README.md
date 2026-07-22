# `@endo/cbor`

Canonical [CBOR][cbor] (Concise Binary Object Representation) primitives for
reading and writing one item at a time.

## Overview

This package provides low-level, allocation-conscious primitives for encoding
and decoding the CBOR data items that Endo needs, in the deterministic
"canonical" encoding: the shortest argument width for every integer and length,
canonical `NaN`, and canonical `float64`.

The reader and writer operate over a caller-supplied cursor so that a single
buffer can be composed from, or decomposed into, individual items without
intermediate allocation. The reader is tolerant by default and can be made
strict on demand.

## Usage

```js
import {
  makeCborWriter,
  cborWriterBytes,
  writeInt,
  makeCborReader,
  readInt,
} from '@endo/cbor';

const writer = makeCborWriter();
writeInt(writer, 42);
const bytes = cborWriterBytes(writer);

const reader = makeCborReader(bytes);
const value = readInt(reader); // 42
```

[cbor]: https://www.rfc-editor.org/rfc/rfc8949.html
