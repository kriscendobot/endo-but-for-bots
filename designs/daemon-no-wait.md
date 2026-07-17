# Daemon No-Wait: Native Creation vs. Construction Semantics

| | |
|---|---|
| **Created** | 2026-07-17 |
| **Author** | 0xpatrickdev (prompted) |
| **Status** | Proposed |
| **Source** | `packages/daemon/TODO.md` (Kris Kowal, commit `e86cad138`, 2026-01-23); maintainer review on PR [#751](https://github.com/endojs/endo-but-for-bots/pull/751#discussion_r3600377291) |

## What is the Problem Being Solved?

Every formula-producing daemon method today resolves only when the formula's
value has finished **constructing**, even though the daemon internally
completes **creation** — durable formula persistence plus pet-name
association — much earlier.
A caller who only wants the thing created and named (an agent harness, a
script, a `--no-wait` CLI invocation) has no milestone to await other than
full construction, which for `evaluate`, `makeUnconfined`, and `request` can
be slow or unbounded.

PR #751 tried to patch this in `@endo/agent-tools` with a tool-local
`deadlineMs` and a `setTimeout` race.
The maintainer rejected that as misdirection: waiting and timeouts are
harness policy; the daemon needs native creation-versus-construction
semantics, with waiting for everything remaining the default.
This design turns the `packages/daemon/TODO.md` note — separately await
formula creation and formula construction, enabling a `--no-wait` flag and a
later `show` — into an implementable contract.

## Current Architecture (survey)

The split already exists inside the daemon; it is collapsed only at the facet
boundary.

**Daemon core.** `formulate` in `packages/daemon/src/daemon.js` is the eager
choke point.
Its sequence: format the id; `persistencePowers.writeFormula` (disk before
graph, per the documented invariant); insert into `formulaForId` and
`formulaGraph.onFormulaAdded` under `withFormulaGraphLock`; publish on
`formulaChangeTopic`; construct a controller; then **eagerly** kick off
construction via `evaluateFormula` and return
`harden({ id, value: controller.value })` with the construction promise
un-awaited.
`formulateLazy` (used by `formulatePeer`) persists without constructing and
returns just the id.
`provideController`/`provide` lazily (re)incarnate on demand via
`evaluateFormulaForId`; after a restart, `seedFormulaGraphFromPersistence`
reloads formulas and retention edges but constructs nothing until the next
`provide`.
A rejected construction is **not** memoized: `promise.catch(context.cancel)`
evicts the controller, so the immediate caller sees the rejection and the
next `provide` retries construction.

**Naming.** Result names are written through `makeDeferredTasks`
(`packages/daemon/src/deferred-tasks.js`).
Each facet method pushes `E(directory).storeIdentifier(resultNamePath,
identifiers.<x>Id)`; each `formulate*` wrapper (`formulateEval`,
`formulateUnconfined`, `formulateReadableBlob`, …) runs
`deferredTasks.execute(identifiers)` inside the graph lock **before** calling
`formulate`.
So the pet name is durably associated before construction begins, and a
failed name write aborts creation entirely (the formula is never persisted).

**Facets.** Nearly every formula-creating method in
`packages/daemon/src/host.js` and `guest.js` ends with
`const { value } = await formulate*(...); return value;` — discarding `id`
and returning the raw construction promise.
`host.evaluate` additionally pins unnamed evals transiently
(`pinTransient`) and awaits the value before unpinning.
Two methods already return at creation: `storeValue` (destructures `{ id }`
from `formulateMarshalValue`, unpins, returns `undefined`) and `endow`
(destructures `{ id: evalId }` from `formulateEval` and delivers a value
message without awaiting construction).

**Guards.** `packages/daemon/src/interfaces.js` splits into two families:
the `M.call(...).returns(M.promise())` family (`evaluate`, `makeUnconfined`,
`makeArchive`, `makeFromTree`, `storeBlob`, `provideMount`, `provideGuest`,
`request`, …) whose implementations control what the promise resolves to,
and the `M.callWhen(...).returns(M.remotable(...))` family (`provideGit`,
`provideShell`, `provideHttpClient`, `provideGitRemote`,
`provideBearerCredential`, `provideBasicCredential`, `provideGitClone`)
which is guard-committed to awaiting construction and returning the built
remotable.

**Observation surfaces.** `lookup`/`maybeLookup`/`lookupById`/
`lookupByLocator` resolve through `provide`; `identify`/`locate` return
ids/locators without constructing; the host-only `diagnostics().getFormula`
([formula-inspector](formula-inspector.md)) reads the durable formula record
without constructing; `diagnostics().traces()` surfaces construction errors
(`endo trace`); `cancel` reaches `cancelValue` in daemon core.

## The Contract

Five milestones for every formula-producing operation, mapped to existing
symbols:

1. **Validation and authority resolution** — facet prelude: guard checks,
   endowment/worker/powers resolution (`prepareWorkerFormulation`,
   `prepareMakeCaplet`).
2. **Identifier allocation and durable formula persistence** —
   `randomHex256` + `persistencePowers.writeFormula` + graph insert inside
   `formulate`.
3. **Atomic association of the requested pet-name path** —
   `deferredTasks.execute(identifiers)` under `withFormulaGraphLock`.
   (Internally this precedes milestone 2 today; the two settle together
   inside one locked section, and the `formulate*` wrapper's returned
   promise settles only after both.)
4. **Construction start** — `evaluateFormula` in `formulate`'s synchronous
   prelude. Construction remains **eager on first formulation** and lazy on
   demand after restart; this design does not change eagerness.
5. **Construction fulfillment or rejection** — settlement of
   `controller.value`.

```mermaid
sequenceDiagram
    participant C as Caller
    participant H as Host/Guest facet
    participant D as Daemon core
    C->>H: startEvaluate(..., resultName)
    H->>D: formulateEval(..., tasks)
    D->>D: name written (deferred task, in lock)
    D->>D: formula persisted + graph insert
    D-->>D: evaluateFormula (construction starts)
    D-->>H: { id, value } (value un-awaited)
    H-->>C: FormulationReceipt { id, locator }
    Note over C: later: lookup/show awaits value;<br/>inspect reads formula; trace shows errors
```

Normative rules:

- A **default (waiting) call** — every existing method, unchanged — settles
  at milestone 5, returning the constructed value exactly as today.
- A **no-wait call** settles at milestone 3 (with milestone 4 already
  initiated), returning a **formulation receipt**, and MUST NOT settle
  before the formula and its requested name can survive a daemon restart.
- Milestone 1–3 failures (validation, unknown endowment or worker names,
  invalid or unwritable result paths) reject the no-wait call itself; no
  formula is persisted.
- Milestone 5 rejection after a no-wait ack remains observable: `lookup` /
  `show` on the result name rejects with the construction error (and, per
  current controller semantics, the next `provide` retries construction);
  `endo trace` records worker-attributed errors; `endo inspect` /
  `getFormula` always shows the durable formula record.

### The receipt

```js
/** @typedef {{ id: FormulaIdentifier, locator: string }} FormulationReceipt */
```

A hardened **data-only copyRecord**: the formula identifier and its
`endo://` locator (`formatLocator` from `packages/daemon/src/locator.js`).
No promise leaf and no remotable:

- Across CapTP it passes by copy; the caller's single `await` on the method
  ends at creation. Nothing to assimilate, so promise flattening cannot
  collapse creation back into construction.
- A `value` promise inside the receipt was considered and rejected: a
  no-wait caller by definition drops it, converting every construction
  failure into unhandled-rejection noise, and the promise dies with the
  CapTP connection while id and name survive restart.
- A receipt cannot be confused with an evaluated program's completion value
  because only the new `start*` methods return receipts; `evaluate` never
  does.

### No-wait requires a result name

`start*` methods take the result name as a **required** parameter (guarded
`NameOrPathShape`, not `.optional`).
Rationale: retention.
An unnamed formula is reachable only via transient pins, which do not
survive restart; a receipt for an unnamed formula would dangle after
`sweepUnreachable`.
The TODO frames no-wait as "commands that create and name a thing".
An unnamed variant (locator plus transient or `@pins` retention) is an open
question, not part of this design.

### Naming, replacement, and paths

`start*` reuses the existing deferred-task write —
`E(directory).storeIdentifier(resultNamePath, identifiers.<x>Id)` — so:

- Existing-name replacement keeps today's `storeIdentifier` overwrite
  semantics (the `move`-overwrites regression tests remain authoritative).
- Nested paths resolve through directory hubs as today; a missing
  intermediate hub rejects the deferred task, which aborts creation before
  persistence — the caller gets the error, no orphan formula, though
  already-formulated dependencies (a fresh worker) may remain, as today.
- Names land in the calling agent's namespace (host names do not leak into
  guests and vice versa; the `'guest evaluate executes code directly'` test
  is authoritative).

### Retention, collection, cancellation, workers

- Because the name is written at creation, `start*` needs no
  `pinTransient`; the ephemeral-eval pin/unpin path in `host.evaluate` is
  untouched.
- `endo remove <name>` before settlement removes the formula's only edge;
  collection then cancels the in-flight construction ("became unreachable by
  any pet name path and was collected") — this is the intended abandon
  story for a no-wait operation, alongside explicit `endo cancel <name>`
  (→ `cancelValue`, cascading through `thisDiesIfThatDies` contexts).
- Worker lifetime is unchanged: the eval/caplet formula's `formulaDeps`
  edge retains its worker exactly as in the waiting path.

### Restart

The receipt guarantees formula JSON and pet-store edge are durable.
After a restart, construction is **lazy on demand** (existing semantics:
`'persist spawn and evaluation'` re-derives `twenty` by lookup;
`'closure state lost by restart'` documents reconstruction ≠ state
restoration).
A no-wait operation interrupted by restart therefore re-runs on the next
`lookup`/`show`; construction is at-least-once-on-demand, not
guaranteed-background-completion.
Side-effecting programs may re-run — already true today for any named eval
looked up after restart.
Remote (cross-node) holders of a receipt are subject to the existing
gateway restriction ("Gateway can only provide local values") and
node-number change on restart; receipts are not a new remote-durability
promise.

### Construction outcomes

- Fulfills to pass-by-copy data: `lookup`/`show` yield the copy (within a
  session, the cached controller value; after restart, re-derived).
- Fulfills to a remotable: `lookup` yields the live reference.
- Rejects: `lookup`/`show` reject with the construction error; the
  controller is evicted, so subsequent provides retry.
  Durable memoization of construction failure is deliberately out of scope
  and deferred to [daemon-commands-as-messages](daemon-commands-as-messages.md)'s
  reply-message model (open question 2).

## API Shape

Three shapes were compared:

1. **Options record on existing methods** (`evaluate(..., { wait: false })`).
   Rejected: the return type becomes a union discriminated by an option
   value, which method guards cannot express, `@endo/exo` cannot type, and
   which makes a receipt confusable with a program that evaluates to a
   similar record.
2. **Durable operation-handle remotable.** Rejected: requires a new formula
   type and lifecycle for the handle itself; redundant because the formula
   id **is** the durable handle and the pet name already retains it — all
   later observation goes through existing naming and inspection surfaces.
3. **Parallel start methods** (chosen): for each family where no-wait is
   meaningful, a sibling method with a `start` prefix and a receipt return.
   Distinct method ⇒ distinct guard and distinct return type; existing
   methods keep their exact signatures and behavior, so conversion is
   incremental and default-wait compatibility is structural rather than
   conditional.

New facet methods (this design's full set):

| New method | Facets | Signature |
|---|---|---|
| `startEvaluate` | Host, Guest | `(workerName, source, codeNames, petNamePaths, resultName)` — resultName required |
| `startMakeUnconfined` | Host | `(workerName, specifier, options)` — `options.resultName` required |
| `startMakeArchive` | Host | `(workerName, archiveName, options)` — same |
| `startMakeFromTree` | Host | `(workerName, treeName, options)` — same |
| `startMakeUnconfinedFromTree` | Host | `(workerName, treeName, options)` — same |
| `startRequest` | Host, Guest | `(toNameOrPath, description, responseName)` — responseName required |

Implementation pattern (eval shown; all follow it): factor the body of
`evaluate` into an internal `evaluateInternal` returning the daemon-core
`{ id, value }`; `evaluate` keeps its current tail (ephemeral pin/unpin,
`return value`); `startEvaluate` asserts the result name, discards `value`,
and returns `harden({ id, locator: formatLocator(id, 'eval') })`.
Guards: `startEvaluate: M.call(M.or(NameOrPathShape, M.undefined()),
M.string(), M.arrayOf(M.string()), NamesOrPathsShape, NameOrPathShape)
.returns(M.promise())` (and analogously for the others); the resolved
receipt shape is documented in `types.d.ts` as `FormulationReceipt`.

`startRequest` requires one semantic change in `makeMailbox`
(`packages/daemon/src/mail.js`): today the `request` implementation stores
`responseName → resolutionId` only **after** the response resolves.
`startRequest` stores the resolution (promise-formula) id at creation, so a
later `lookup(responseName)` awaits settlement through the existing
`promise`-formula watch machinery (`makePromise` over the status pet
store) — exactly the pending/fulfilled/rejected vocabulary
[formula-inspector](formula-inspector.md) already renders.
The default `request` naming timing is unchanged.

**Ownership boundaries.** Daemon core already exposes the milestone split
(`{ id, value }`); this design adds no core primitive.
Host/Guest facets own the receipt surface.
The CLI owns `--no-wait` mapping.
Harnesses (`@endo/agent-tools`, chat, scripts) own waiting policy: a
harness that wants a bounded wait races `startEvaluate`'s receipt against
its own scheduler and later reads the name — no daemon deadlines, per the
PR #751 review directive.
This design does not modify `@endo/agent-tools`.

## Inventory

Derivation: enumerated from the `HostInterface` and `GuestInterface` guard
objects in `packages/daemon/src/interfaces.js` (every method name in the
guards was classified), the directory facet in
`packages/daemon/src/directory.js`, the `formulate*` inventory in
`packages/daemon/src/daemon.js`, and the complete commander registry in
`packages/cli/src/endo.js` (all 44 command modules read).
Read-only, mail-control, and lifecycle methods (`lookup`, `list`,
`identify`, `locate`, `followNameChanges`, `resolve`, `reject`, `dismiss`,
`cancel`, `remove`, `move`, `copy`, `storeIdentifier`, `storeLocator`, …)
create no formulas and are excluded as no-change by construction; `move`
and `copy` re-point existing ids.

### Group A — no-wait siblings in this design

| Method / CLI | Formulation path | Result name | Waits today | Change |
|---|---|---|---|---|
| `host.evaluate` / `guest.evaluate`; `endo eval` | `formulateEval` | optional 5th param | construction (`return value`; ephemeral: `await value` + unpin) | add `startEvaluate`; CLI `--no-wait` (requires `-n`) |
| `host.makeUnconfined`; `endo make --UNCONFINED` | `formulateUnconfined` via `prepareMakeCaplet` | `options.resultName` | construction | add `startMakeUnconfined`; CLI `--no-wait` |
| `host.makeArchive`; `endo make <archive>` | `formulateArchive` | `options.resultName` | construction | add `startMakeArchive`; CLI `--no-wait` (temp-archive cleanup reworked, see slice 3) |
| `host.makeFromTree` | `formulateFromTree` | `options.resultName` | construction | add `startMakeFromTree` |
| `host.makeUnconfinedFromTree` | `stageTreeInternal` + `makeUnconfined` | `options.resultName` | construction | add `startMakeUnconfinedFromTree` |
| `agent.request`; `endo request` | `formulatePromise` via `makeRequest` | `responseName`, written **after** resolution | resolution (unbounded, human-in-loop) | add `startRequest` with eager response naming; CLI `--no-wait` |

### Group B — already creation-only (no change; pin with tests)

| Method / CLI | Formulation path | Why conforming |
|---|---|---|
| `host.storeValue` / `guest.storeValue`; `endo store` | `formulateMarshalValue` | returns `undefined` after creation + unpin; never awaits `value`. This is the `storeValue` seam [endo-agent-tools](endo-agent-tools.md) / PR #751 consumes |
| `host.endow`; `endo endow` | `formulateEval` + `deliverValueById` | destructures `{ id: evalId }`, never awaits construction |
| `agent.submit`; `endo submit` | `formulateMarshalValue` | posts value message; returns `void` |
| `guest.define`; `endo define` | `formulatePromise` via `makeDefineRequest` | posts definition message; returns `void`. (CLI cannot name the result — a `--name` option is a separate gap, out of scope) |

### Group C — construction is fast, local, and bounded (no change)

| Method / CLI | Formulation path | Classification rationale |
|---|---|---|
| `provideWorker`; `endo spawn` | `formulateWorker` (idempotent) | worker incarnation is local process start; ack ≈ settlement |
| `makeDirectory`; `endo mkdir` | `formulateDirectory` | local pet-store creation |
| `writeText` (directory leaf) | `formulateReadableBlob` | content written during creation |
| `storeBlob`; `endo store`/`endo archive` | `formulateReadableBlob` | `contentStore.store` consumes the client-side reader **during creation**; returning early would abandon the caller's own stream |
| `storeTree`; `endo checkin` | `checkinTree` | same: content-addressing is the creation |
| `provideMount`; `endo mount` | `formulateMount` | local fs handle |
| `provideScratchMount`; `endo mktmp`; `stageTree` | `formulateScratchMount` | local |
| `provideHost` / `provideGuest`; `endo mkhost`/`mkguest` | `formulateHost`/`formulateGuest` | the dependency chain (keypair, stores, hub, worker) **is** creation; construction is quick; both idempotent. Revisit only if agent incarnation becomes slow |
| `makeChannel`, `makeTimer`, `invite` | `formulateChannel`/`formulateTimer`/`formulateInvitation` | local formulation; `invite`'s value is the invitation itself |
| `accept`; `endo accept` | `formulateGuest` + peer wiring | the peer handshake is inherent to the operation's meaning; returns `void` at completion |

### Group D — guard-committed to construction (no change)

`provideGit`, `provideShell`, `provideHttpClient`, `provideGitRemote`,
`provideBearerCredential`, `provideBasicCredential`, `provideGitClone` use
`M.callWhen(...).returns(M.remotable(...))`: the guard awaits and demands
the built remotable.
Their constructions are fast local capability wiring; converting them would
require guard changes for no benefit.
Classified no-change.

### CLI observation surfaces (unchanged, now load-bearing)

`endo show` (`lookup` + `formatValue`) is the deferred read; `endo inspect`
(`diagnostics().getFormula`) reads the durable formula a receipt promises;
`endo trace` surfaces post-ack construction errors; `endo list`/`locate`/
`paths` observe naming and retention.
CLI plumbing note: the process exits when `withInterrupt`'s callback
resolves and `cancel()` closes the CapTP socket (`packages/cli/src/context.js`);
`--no-wait` works by simply not awaiting anything past the receipt — no
`process.exit`, no timers.

CLI `--no-wait` output contract: print the receipt's locator (an
`endo://` URL, visually unmistakable for a program result) to stdout;
errors before creation exit non-zero as today.

## Dependencies

| Design | Relationship |
|---|---|
| [formula-inspector](formula-inspector.md) | Provides the observe-later surface (`getFormula`, `endo inspect`) and the pending/fulfilled/rejected promise-formula vocabulary this design reuses |
| [daemon-guest-eval-simplification](daemon-guest-eval-simplification.md) | Established `formulateEval` as the single host/guest eval path; `startEvaluate` must keep that parity |
| [daemon-commands-as-messages](daemon-commands-as-messages.md) | The durable command/reply result model; a no-wait receipt maps onto "command message now, value reply later". Durable failure memoization defers there |
| [chat-pending-commands](chat-pending-commands.md) | UI-only pending region; the front-end consumer of the same pending/settled states |
| [endo-agent-tools](endo-agent-tools.md) | PR #751 context; the harness consumer. `storeValue(valueOrPromise, nameOrPath)` and code-mode `evaluate` will adopt `startEvaluate` for bounded-wait harness policy |

## Phased Implementation

Slices are individually mergeable; each leaves default behavior bit-for-bit
compatible until the next lands.
No temporary adapters are needed anywhere: every slice is purely additive to
public surfaces, so nothing is later removed.

**Slice 1 — `startEvaluate` vertical (daemon).** Size S-M, risk low.
Files: `packages/daemon/src/host.js` (factor `evaluateInternal`; add
`startEvaluate`), `guest.js` (same, preserving parity), `interfaces.js`
(guard), `types.d.ts` (`FormulationReceipt`, method type),
`help-text-data.js`.
Tests (`packages/daemon/test/endo.test.js`): receipt returned while
construction pending (eval of a never-settling promise; assert `has`/
`identify` succeed and `getFormula` shows the eval record before
settlement); missing result name rejects; restart between receipt and
settlement then `lookup` re-derives; construction rejection surfaces on
`lookup` and in traces; unnamed-eval GC accounting unchanged
(`'unnamed eval results are collected'` stays green); guest namespace
isolation.

**Slice 2 — CLI `endo eval --no-wait`.** Size S, risk low.
Files: `packages/cli/src/endo.js` (flag), `packages/cli/src/commands/eval.js`.
Tests (`packages/cli/test/`): `--no-wait` without `-n` errors; with `-n`
exits promptly printing the locator while a slow eval is pending; follow-up
`endo show` prints the value; failing eval observed via `endo show`
non-zero and `endo trace --recent`.

**Slice 3 — caplet family.** Size M, risk medium (shared
`prepareMakeCaplet` refactor).
Files: `host.js` (`startMakeUnconfined`, `startMakeArchive`,
`startMakeFromTree`, `startMakeUnconfinedFromTree` over a shared internal),
`interfaces.js`, `types.d.ts`, `help-text-data.js`;
`packages/cli/src/commands/make.js` (`--no-wait`; remove the temp archive
by name immediately after the receipt — safe because the `make-archive`
formula's `formulaDeps` edge, not the `tmp-archive-*` pet name, retains the
blob).
Tests: daemon start-variant coverage for unconfined and archive (note:
first-ever `makeArchive`/`makeFromTree` regression coverage rides along);
CLI `endo make --no-wait` including temp-archive removal and later `show`.

**Slice 4 — `startRequest`.** Size S-M, risk medium (naming-timing
change is new observable behavior, though only for the new method).
Files: `packages/daemon/src/mail.js` (factor `makeRequest` to accept
eager response naming), `host.js`/`guest.js` wiring, `interfaces.js`,
`types.d.ts`, `help-text-data.js`;
`packages/cli/src/commands/request.js` (`--no-wait`).
Tests: receipt before resolution; `lookup(responseName)` pends then
fulfills on `resolve`; rejects on `reject`; restart with pending request
then resolve-after-restart (extends
`'rehydrated requests can be resolved after restart'`).

**Slice 5 — conformance pinning and docs.** Size S, risk low.
Tests asserting Group B methods (`storeValue`, `endow`, `submit`) settle
without awaiting construction (regression fence for the classification);
delete the no-wait note from `packages/daemon/TODO.md`; README/help sweeps.
Follow-up consumption of `startEvaluate` by `@endo/agent-tools` is tracked
in [endo-agent-tools](endo-agent-tools.md) (implementation issue to be
filed), not here.

Total estimate: M-L, roughly one week.
The smallest useful vertical is slices 1–2: they prove receipt semantics,
restart durability, later lookup, and default-wait compatibility on the
`evaluate` path before the caplet and request families migrate.

## Design Decisions

1. **Parallel `start*` methods, not an options record or handle** — distinct
   method ⇒ distinct guard, unconditional return type, zero change to
   existing signatures; see § API Shape.
2. **Receipt is data-only `{ id, locator }`** — survives restart, passes by
   copy over CapTP, cannot be collapsed by promise assimilation, no
   unhandled-rejection noise.
3. **No-wait requires a result name** — the pet name is the retention edge
   that makes the receipt durable; unnamed no-wait would dangle.
4. **Construction stays eager at first formulation, lazy after restart** —
   unchanged from today; no background-completion guarantee is added.
5. **Construction failure stays retry-on-provide, not memoized** — matches
   the existing controller-eviction semantics; durable failure records
   belong to daemon-commands-as-messages.
6. **No daemon or tool deadlines** — waiting policy is the harness's;
   the daemon exposes milestones only (PR #751 review directive).
7. Considered and rejected: a `value` promise inside the receipt.
   Reason: unhandled-rejection noise for no-wait callers and
   connection-lifetime instability.
8. Considered and rejected: a generic `start(methodName, args)` dispatcher.
   Reason: stringly-typed dispatch erodes per-method guards.

## Open Questions

1. Should an unnamed no-wait variant exist, returning a locator and pinning
   via `@pins` (or a transient lease) instead of a pet name?
   Deferred; Group A requires names.
2. Should construction rejection be durably memoized (an error record
   observable without re-running construction), rather than retried on each
   `provide`?
   This design keeps current semantics; daemon-commands-as-messages' reply
   messages are the natural home for durable outcomes.
3. Should the pet-name write move after formula persistence inside the
   locked section?
   Today `deferredTasks.execute` precedes `writeFormula`, so a crash inside
   the window can leave a name pointing at a never-persisted formula
   (pre-existing; possibly the `remove` ENOENT symptom in
   `packages/daemon/TODO.md`).
   The receipt is correct either way — it settles only after both — but a
   hardening reorder could ride slice 1 if the maintainer wants it.
4. Should `endo mkhost`/`mkguest`/`accept` gain `--no-wait` later?
   Classified Group C (creation-dominant) for now; revisit if agent
   incarnation grows slow enough to matter.
5. CLI output under `--no-wait`: locator on stdout (as specified), or pet
   name, or silence?
   Locator chosen for scriptability; cheap to change before slice 2 lands.

## Prompt

> For Endo commands that create and name a thing, like `makeUnconfined`, we
> should be able to await the promise for the creation of the formula and
> then separately, conditionally, await the construction of the formula.
> This will require a refactor of many commands and the CLI, and will allow
> the addition of a `--no-wait` flag for many commands, such that they can
> exit and allow the user to follow-up with a show command.
> — `packages/daemon/TODO.md` (Kris Kowal, commit `e86cad138`)

Expanded per the maintainer's PR #751 review ("Waiting/timeouts are a
harness concern, not the tool's. The daemon still needs to work out
`--no-wait`, with waiting for everything as the default.") into a full
inventory, milestone contract, API selection, and incremental landing plan
for `endojs/endo-but-for-bots`.
