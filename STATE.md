# PR #627 follow-up state

## Deliverable

Address all code-panel must-fix findings and the summary type correction for
PR #627, verify `@endo/agentry`, push the fixes, retain draft status, and
request the next code-panel run.
This fixer owns the final packaging step and must remove `STATE.md` before the
upstream push.

## Current branch

`fix-ebfb-627-code-panel-findings` is rebased onto `origin/llm`.
The rebased reviewed commits are `7aa552129`, `17a35188b`, and `c84423703`.

## Plan

1. Repair CLI initialization and boundary/registry handling.
2. Make scenario provisioning contained and failure-safe.
3. Restore environment gating and remove the unsafe host-shell condition.
4. Add the required changeset, run verification, remove this state file, and
   push the follow-up commits.

## Hazards

The reviewed shell condition uses a host spawner, which cannot provide the
claimed filesystem confinement.
It will be removed rather than presenting host-process authority as confined.
