# STATE.md

## Deliverable contract

Address all F1-F10 and F13 findings from code-panel review 4726560054 on PR
#646, push the fixes to `build-ebfb-phase6-conflict-side-selection`, keep the
PR draft, and request the next code-panel run.
The final packaging step must remove `STATE.md` and verify it is absent from
the outgoing PR diff.

## Work completed

- Branch: detached worktree at PR head `1ed75df5973cc68226b81585df97ed5867343598`.
- Base: `origin/llm` at `e8edeb2b232dae9a019112c70809e4b91176b3ca`.
- Commit map: existing PR commits are `654255f86`, `63b42a8da`,
  `35d084900`, followed by fixups `496dc3bcd` and `1ed75df59`.
- Confirmed PR #646 is draft and its head matches the requested SHA.

## Decisions

- Retain the widened default `makeGitTool` catalog, while documenting that
  runtime `allowHistoryRewrite` authority remains required by `makeGit`.
- Remove the redundant `makeGitHistoryTool` surface when the default catalog
  already contains those methods.
- Make `checkoutConflict` preflight every requested path and side stage before
  any checkout mutation, rejecting duplicates and invalid batches clearly.

## Pending work

1. Autosquash the two existing fixup commits into their targets.
2. Implement the authority/docs/schema/test/backend fixes and add a changeset.
3. Run focused tests, lint, type checks, docs/checklist, and pre-push gates.
4. Commit the final removal of `STATE.md`, push, verify draft state, and request
   the next code-panel run.

## Hazards and verification

- Native backend conflict fixtures must cover clean paths, modify/delete
  conflicts, mixed batches, and duplicate paths without partial mutation.
- The final PR diff must not contain `STATE.md`.
- Any failed gate must be recorded in the completion report with exact command,
  exit status, failing stage/probe names, and concrete findings.
