# STATE.md

## Deliverable contract

Address all F1-F10 and F13 findings from code-panel review 4726560054 on PR
#646, push the fixes to `build-ebfb-phase6-conflict-side-selection`, keep the
PR draft, and request the next code-panel run.
The final packaging step must remove `STATE.md` and verify it is absent from
the outgoing PR diff.

## Work completed

- Branch: detached worktree based on PR head `1ed75df5973cc68226b81585df97ed5867343598`.
- Base: `origin/llm` at `e8edeb2b232dae9a019112c70809e4b91176b3ca`.
- Commit map: existing PR commits are now autosquashed as `654255f86`,
  `5053a07ff`, and `d2bd9957f`; `8db191ba2` reconciles the default history
  tool surface, release notes, README, canonical rebase guard, and authority
  regression test; `1ec373be1` preflights conflict side checkout, reports
  clear tool errors, and covers clean, mixed, duplicate, and modify/delete
  paths, with all test and lint fixups autosquashed.
- Confirmed PR #646 is draft and its head matches the requested SHA.

## Decisions

- Retain the widened default `makeGitTool` catalog, while documenting that
  runtime `allowHistoryRewrite` authority remains required by `makeGit`.
- Remove the redundant `makeGitHistoryTool` surface when the default catalog
  already contains those methods.
- Make `checkoutConflict` preflight every requested path and side stage before
  any checkout mutation, rejecting duplicates and invalid batches clearly.

## Pending work

Step 1: Focused tests, full package tests, lint, types, build, docs, changeset
   status, and pre-push gates are green.
Step 2: Remove `STATE.md`, verify the outgoing diff, push, verify draft state,
   and request the next code-panel run.

## Hazards and verification

- Native backend conflict fixtures must cover clean paths, modify/delete
  conflicts, mixed batches, and duplicate paths without partial mutation.
- The final PR diff must not contain `STATE.md`.
- Any failed gate must be recorded in the completion report with exact command,
  exit status, failing stage/probe names, and concrete findings.
