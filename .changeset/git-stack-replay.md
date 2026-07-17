---
'@endo/git': minor
'@endo/exo-git': major
'@endo/agent-tools': minor
'@endo/agentry': major
---

Add cherry-pick and structured autosquash rebase operations to the Git APIs.
`@endo/agent-tools` includes these narrow history-rewrite operations in the
default `makeGitTool` inventory.
The live Git capability still requires history-rewrite authority, and the JSON
rebase tool is start-only: it does not expose continue, abort, or skip.
