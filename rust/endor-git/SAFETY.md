# Custom backend safety invariants

All custom libgit2 pointer use is isolated in `src/ffi.rs`.
The safe API and consumer-provided `BackendStorage` implementations never
receive a libgit2 pointer.

- `ObjectBackend`, `ReferenceBackend`, and `ReferenceIterator` are `repr(C)`
  structures whose libgit2 parent is the first field, so a parent pointer can
  be recovered only while libgit2 owns the allocation.
- Successful ODB/refdb installation transfers each boxed backend to libgit2.
  Each matching `free` callback reconstructs and drops the box exactly once.
  Failed installation reconstructs the box before returning.
- Callback state is shared by `Arc`; repository shutdown cannot drop it before
  both backend free callbacks run.
- Every callback that calls consumer code uses `catch_unwind`.
  A panic becomes a fixed `panic in custom backend` error and never unwinds
  into C.
- C strings are checked for null and UTF-8 before conversion.
  Object byte slices are formed only after checking the pointer/length pair.
- Object read buffers are allocated with `git_odb_backend_data_alloc`, then
  returned to libgit2 for release with its matching allocator.
- References returned from lookup and iteration use libgit2's
  `git_reference__alloc`; libgit2 owns and releases the result.
- Iterator name pointers refer to `CString` values retained in the iterator
  allocation until its free callback.
- The repository mutex serializes all callbacks for one repository.
  Separate repositories retain independent concurrency.

The conformance test drops filesystem and custom SHA-1/SHA-256 repositories,
converts a deliberate backend panic, and races reference updates.
`scripts/sanitizer-check.sh` builds vendored libgit2 with AddressSanitizer and
runs that same suite directly under its runtime.
