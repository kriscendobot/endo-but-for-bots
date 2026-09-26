---
'@endo/eventual-send': minor
'@endo/ocapn': minor
---

Add `E.untag(value, tag)` and `HandledPromise.untag(value, tag)` to obtain the
payload of a matching tagged value. OCapN clients pipeline this operation as
`op:untag` for remote values.
