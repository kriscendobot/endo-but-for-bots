# Re-export policy validation fixtures

These files intentionally exercise the garden's re-export policy automation.
This pull request is a synthetic validation artifact and must remain a draft.

- `compliant.js` is a deprecated compatibility shim that names its canonical
  source module.
- `plain.js` is an intentionally prohibited plain named re-export.
- `index.js` is an intentionally prohibited barrel re-export.
- `type-only.ts` is an exempt type-only re-export.

The prohibited files exist so the deterministic pre-push probe and the
cost-gated `reexport-auditor` review seat can demonstrate their failure paths.
