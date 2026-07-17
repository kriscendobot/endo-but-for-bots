---
'@endo/exo-git': minor
'@endo/git': minor
'@endo/agent-tools': major
---

Expose `checkoutConflict` on the writable Git APIs and mount-bridged agent
tools, selecting Git index stage 2 (`ours`) or stage 3 (`theirs`) for existing
unmerged paths and staging the selected side.
The default `makeGitTool` catalog now includes the narrow history-rewrite
operations; the live Git capability still requires its explicit
`allowHistoryRewrite` authority before those operations can mutate history.
