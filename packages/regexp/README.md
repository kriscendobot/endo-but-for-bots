# @endo/regexp

`@endo/regexp` parses Endo's resource-safe, Unicode-independent profile of
[RFC 9485 I-Regexp](https://www.rfc-editor.org/rfc/rfc9485). It accepts Boolean
whole-string patterns only: captures, flags, anchors, multi-character escapes,
and Unicode properties are outside profile v1.

`parseIRegexp(source)` returns a hardened parsed pattern or throws an
`IRegexpError` with a stable diagnostic code. `matches(parsed, text)` uses the
validated pattern through the JavaScript ponyfill. `contains(parsed)` safely
constructs the substring mode required by grep callers. `compile(source)` is a
convenience layer exposing `test(text)`. `isConservativeRegex(source)` is the
non-throwing classifier.

The fixed cross-language contract is
[`test/i-regexp-profile-cases.json`](test/i-regexp-profile-cases.json). It is
also consumed by `rust/mount_parity` while the native `endor` backend is proved
out. The later layer-R `hostGrepFiles` implementation remains downstream of
this package.
