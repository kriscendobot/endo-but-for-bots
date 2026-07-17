---
'@endo/git': minor
'@endo/exo-git': major
'@endo/agent-tools': minor
'@endo/agentry': minor
'@endo/daemon': minor
---

Add commit amendment and commit-message rewording to the Git APIs.
`@endo/agent-tools` includes the narrow history-rewrite operations in the default `makeGitTool` inventory.
The live Git capability still requires its separate `allowHistoryRewrite` authority before any of those operations can reach the backend.
`makeGit` now takes powers separately from `{ readOnly, allowHistoryRewrite }` options; callers must migrate from the former single-object signature in this major `@endo/exo-git` change.
`@endo/daemon` adds the public `provideGit` history-rewrite option and `GitFormula` support.
