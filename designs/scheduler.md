# Daemon Scheduler Capability

|                |                                        |
| -------------- | -------------------------------------- |
| **Created**    | 2026-06-08                             |
| **Updated**    | 2026-07-06                             |
| **Author**     | Kris Kowal (prompted)                  |
| **Status**     | Proposed (in flight — see § Realization Status) |
| **Parent**     | [daemon-capability-bank](daemon-capability-bank.md) |
| **Supersedes** | [endoclaw-timer](endoclaw-timer.md)    |

## Realization Status (2026-07-06)

The daemon-side graduation this design describes is **in flight but not
yet merged.** As of the `llm` tip on 2026-07-06 the daemon still carries
only the simpler `timer` formula (`TimerFormula` / `formulateTimer` /
`makeTimer`) unchanged; none of the `Scheduler` / `Interval` /
`SchedulerControl` / `TickResponse` exos, the `interval-tick` mail
variant, `makeScheduler` on `HostInterface`, or the CLI verbs exist on
trunk yet, and genie's `packages/genie/src/interval/` prototype
(`makeIntervalScheduler`) remains the only working scheduler in the repo.
lal and fae still have no scheduling.

The build lives on
[PR #609](https://github.com/endojs/endo-but-for-bots/pull/609)
(`build/endoclaw-timer-daemon-formula-integration`, open, based on `llm`):

- Adds `packages/daemon/src/interval-scheduler.js` (~737 lines) and an
  `IntervalSchedulerFormula` (`type: 'interval-scheduler'`) to the
  daemon's formula union and `types.d.ts`.
- Adds an `interval-tick` message type — the typed-envelope tick delivery
  this design specifies.
- Carries the prototype's start-to-start timing, resolve/reschedule,
  missed-tick coalescing, and persistence forward, and has been through a
  panel-review fixup pass (reschedule-drift and GC-orphan fixes,
  validation hardening, added coverage and a changeset).

**Two divergences from this design worth reconciling before merge:**

1. **Name.** This design settled on **`scheduler`** per the maintainer's
   2026-06-08 review ("Let's simply call it 'scheduler'"). PR #609 ships
   the formula and message under the prototype's original
   **`interval-scheduler`** name and frames the work as *endoclaw-timer
   Phase 1 remainder* rather than as this `scheduler` design. The name
   should be settled one way or the other — either rename the formula to
   `scheduler`, or update this design (and its § Capability Shape
   `Scheduler`/`SchedulerControl` rename note) to accept
   `interval-scheduler`.
2. **Lineage framing.** PR #609 is authored against the `endoclaw-timer`
   lineage; this design is `endoclaw-timer`'s declared successor. Whoever
   merges #609 should decide whether it lands *as* the realization of
   this `scheduler` design (updating the Status here to *Implemented*) or
   whether this design is retired in favor of continuing under the
   `endoclaw-timer` Phase-1 banner.

**What remains after #609 merges:** the per-`Interval` pet-name granting
(§ Capability Narrowing), the `serial-jobs`-backed coalescing that
retires genie's hand-rolled draining, switching genie's heartbeat onto
the granted `Interval` and deleting the `interval/` prototype, and giving
lal and fae scheduling (§ 5 of [`genie-integration.md`](genie-integration.md)).

## What is the Problem Being Solved?

SES lockdown removes `setTimeout` and `setInterval` from the global
scope.
An agent running inside a locked-down worker has *no* mechanism for
scheduling future execution.
Without a scheduling capability the agent is purely reactive: it can
only respond to messages it already received.
This is the same gap [`endoclaw-timer.md`](endoclaw-timer.md)
identified, and the same one
[`daemon-capability-bank.md`](daemon-capability-bank.md) lists as the
**Timer / scheduling** capability slot
(`daemon-capability-timer.md`, *Planned*).

The existing daemon `timer` formula type
(`packages/daemon/src/types.d.ts`, `formulateTimer` in
`packages/daemon/src/host.js`) is the simpler "fire-and-forget every
N ms with subscribers" model from `endoclaw-timer.md` Phase 0: no
resolve / reschedule, no missed-tick coalescing, no per-tick deadline,
no host-side control facet, no pause / resume.
The Phase 1 prototype shipped in `packages/genie/src/interval/`
(`makeIntervalScheduler`, `IntervalControl`, `tickResponse`) has the
richer semantics but lives inside one agent harness, persists to its
own per-workspace directory, and is invisible to lal and fae.

The
[`genie-integration.md`](genie-integration.md) survey identified the
graduation: extract the genie prototype into a daemon-side capability
that **any** agent harness (genie, lal, fae) can hold.
The maintainer's framing on the genie-integration review on
2026-06-08: *"Let's simply call it 'scheduler'"* — the capability is
named **scheduler**, not *interval-scheduler*, to keep the user-facing
name short and consistent with the rest of the daemon-capability-bank
family.

This design is the focused version of `endoclaw-timer.md`'s § Capability
Shape: the daemon-side capability the prototype always anticipated, plus
the integration points (pet-store-granting, mail-as-tick-delivery,
serial-jobs-backed coalescing) that the daemon framework gained in the
interim.
The genie prototype's mechanics (start-to-start timing, resolve /
reschedule semantics, missed-tick coalescing, host-controlled limits,
permanent revocation) carry forward; this design carries them into the
daemon and names the integration shape.

## Design

### Capability Shape

The scheduler follows the daemon's caretaker pattern (per
[`daemon-capability-bank.md`](daemon-capability-bank.md) § Design
Principle 3): the agent holds an attenuated facet, the host holds the
control facet.

Three exported interfaces, all hardened-JS exos with `M.interface()`
guards:

- **`Scheduler`** — the agent-facing facet.
  Creates `Interval` capabilities.
  Lists the agent's own intervals.
  Carries `help()`.

- **`Interval`** — a single named, scheduled interval.
  An agent that holds only an `Interval` (granted by pet name; see
  *Capability Narrowing* below) can be ticked but cannot create
  further intervals.
  Carries `setPeriod`, `cancel`, `info`, `help`.

- **`SchedulerControl`** — the host-facing facet.
  Pauses, resumes, revokes the scheduler; sets per-scheduler
  `maxActive` / `minPeriodMs` limits; lists all intervals across
  every guest the scheduler serves.

Each tick delivers a one-shot `TickResponse` capability that the agent
must `resolve()` or `reschedule()` before the next tick fires.

```ts
interface Scheduler {
  makeInterval(
    label: string,
    periodMs: number,
    opts?: {
      firstDelayMs?: number;   // default 0 (immediate first tick)
      tickTimeoutMs?: number;  // default periodMs / 2
    },
  ): Promise<Interval>;
  list(): Promise<IntervalEntry[]>;
  help(): string;
}

interface Interval {
  label(): string;
  period(): number;            // current periodMs
  setPeriod(periodMs: number): Promise<void>;
  cancel(): Promise<void>;
  info(): IntervalEntry;
  help(): string;
}

interface SchedulerControl {
  setMaxActive(n: number): void;
  setMinPeriodMs(ms: number): void;
  pause(): void;
  resume(): void;
  revoke(): void;
  listAll(): Promise<IntervalEntry[]>;
  help(): string;
}

interface TickResponse {
  resolve(): void;
  reschedule(): void;
}

type IntervalEntry = {
  id: string;
  label: string;
  periodMs: number;
  firstDelayMs: number;
  tickTimeoutMs: number;
  nextTickAt: number;        // epoch ms of next scheduled tick
  createdAt: number;         // epoch ms when created
  tickCount: number;         // total ticks fired
  status: 'active' | 'paused' | 'cancelled';
};
```

The `IntervalEntry` shape carries forward verbatim from
`endoclaw-timer.md` § Capability Shape; the rename from
`IntervalScheduler` / `IntervalControl` to `Scheduler` /
`SchedulerControl` is the only API-surface delta.

### Tick Delivery as Daemon Mail

Tick events are delivered through the existing daemon mail system
rather than a new delivery mechanism.

The genie prototype delivers ticks through an in-process `onTick`
callback today and then bridges to mail with a side-channel
`pendingHeartbeatTicks` map.
The daemon-native version delivers each tick directly as a
`type: 'package'` message into the recipient's inbox, eliminating the
side-channel map entirely.

```ts
type IntervalTickMessage = {
  type: 'interval-tick';
  messageId: FormulaIdentifier;
  from: FormulaIdentifier;            // the scheduler's handle
  to: FormulaIdentifier;              // the agent's handle
  intervalId: string;
  label: string;
  periodMs: number;
  tickNumber: number;                 // 1-indexed count for this interval
  scheduledAt: number;                // intended fire time (epoch ms)
  actualAt: number;                   // actual fire time (epoch ms)
  missedTicks: number;                // ticks missed during downtime (0 normally)
  tickResponseId: FormulaIdentifier;  // ref to TickResponse capability
};
```

The `tickResponse.resolve()` / `tickResponse.reschedule()` calls map
onto an `E(tickResponse).resolve()` / `E(tickResponse).reschedule()`
round-trip on the one-shot exo whose id is carried in the mail
envelope.
This is the same shape as
[`daemon-value-message.md`](daemon-value-message.md)'s `valueId` reply
primitive: a typed envelope plus a one-shot capability the recipient
resolves.

Tick events interleave naturally with other messages in arrival order;
inbox-replay recovery includes them; agents handle them identically
to user messages.

### Start-to-Start Timing

Each tick is scheduled relative to the previous tick's *scheduled*
time, not its actual completion time.
A 60-second interval fires 60 times per hour regardless of processing
time.
Detail and edge cases (processing-takes-longer-than-period, retry
backoff cap, deadline overrun) carry forward verbatim from
[`endoclaw-timer.md`](endoclaw-timer.md) § Start-to-Start Timing and
§ Resolve/Reschedule Semantics.

### Persistence

Scheduler entries live in the daemon's existing state directory under
a per-formula `intervals/` subdirectory; entry files use the same
atomic write-then-rename pattern as
`packages/daemon/src/synced-pet-store.js` and the content store, with
absolute `nextTickAt` so restart recovery is a simple
`compare-to-now-and-arm` walk.
Detail (directory layout, JSON shape, atomic write helper) carries
forward verbatim from [`endoclaw-timer.md`](endoclaw-timer.md) §
Persistence.

The graduation delta: the prototype takes a `persistDir` argument the
agent supplies; the daemon-native scheduler uses the daemon's own
state directory and the established `withFormulaGraphLock` pattern, so
the agent does not have to know where its persistence lives.

### Capability Narrowing via Pet-Name Granting

The principle-of-least-authority shape: an agent should not hold the
whole `Scheduler` if all it needs is to be told once a day.
The daemon's pet-name discipline already supplies the mechanism.

```js
// Host creates the scheduler / scheduler-control pair on first spawn.
const { scheduler, schedulerControl } = await E(host).makeScheduler(
  'main-scheduler',
  { maxActive: 5, minPeriodMs: 60_000 },
);

// Host pre-creates a single 'daily-reflect' interval and grants the
// agent *only* that Interval handle, not the scheduler.
const dailyReflect = await E(scheduler).makeInterval(
  'daily-reflect',
  24 * 60 * 60 * 1000, // 24h
);
await E(host).storeIdentifier(
  'daily-reflect',
  await E(dailyReflect).identifier(),
);
agentGuest = await E(host).provideGuest('genie-main', {
  agentName: 'genie',
  introducedNames: harden({ 'daily-reflect': 'daily-reflect' }),
});
```

The agent's pet store contains `daily-reflect`; the agent can resolve
it, observe tick messages arriving in its inbox, and respond.
The agent **cannot** create additional intervals, cannot raise the
period beyond the host's policy, and cannot reach the scheduler that
would let it do either.
Today a guest with the equivalent of `setInterval` could burn the
host's CPU; a granted `Interval` cannot.

This is the same pet-name-granting shape that
[`daemon-agent-tools.md`](daemon-agent-tools.md) uses for `endo grant
fae fs /home/user/project`: capability narrowing happens at grant time,
not in the receiving agent's prompt.

### Maker on `HostInterface`

```ts
// Added to HostInterface
makeScheduler: M.callWhen(
  M.string(),                    // petName for the scheduler
  M.opt(M.splitRecord({}, {
    maxActive: M.number(),
    minPeriodMs: M.number(),
  })),
).returns(M.record({
  scheduler: M.remotable('Scheduler'),
  schedulerControl: M.remotable('SchedulerControl'),
})),
```

The maker:

1. Resolves `petName` for namespace placement.
2. Creates a new handle for the scheduler so its tick messages have a
   valid `from` identity.
3. Formulates a `scheduler` formula with `{ handle, maxActive,
   minPeriodMs, paused: false }`.
4. Creates the `Scheduler` and `SchedulerControl` exo facets.
5. Writes the scheduler capability into the host's namespace under
   `petName`.
6. Returns `{ scheduler, schedulerControl }`.

### `serial-jobs`-backed Coalescing

The daemon already uses `serial-jobs`
(`packages/daemon/src/serial-jobs.js`, imported in `daemon.js` and
`mail.js`) as an internal task queue.
Genie's heartbeat coalescing logic in `runAgentLoop`
(`drainPendingHeartbeats` and friends) is essentially a hand-rolled
single-consumer serial-jobs queue.

The daemon-side scheduler exposes a per-`Interval` *coalescing on*
flag (default `true`) that internally routes tick delivery through
`serial-jobs` keyed by `intervalId`, so at most one tick is in flight
per interval regardless of processing time.
This retires `drainPendingHeartbeats` and the associated
genie-internal coalescing code; the daemon's task queue is the only
queue.

### Revocation, Pause / Resume, Startup Recovery

Carry forward verbatim from [`endoclaw-timer.md`](endoclaw-timer.md) §
Revocation, § Pause and Resume, § Startup Recovery.
The cancellation-context integration
(`context.onCancel` clearing all timeouts on scheduler-formula
cancellation, `context.thisDiesIfThatDies(agentId)` tying the
scheduler's lifetime to the agent it serves) is the same as the
prototype's design intent and lands during the graduation.

### `extractDeps` Integration

`extractDeps` in `daemon.js` gains:

```js
case 'scheduler':
  return [formula.handle];
```

The handle is a strong dependency (the scheduler's identity in the
mail system). The per-`Interval` capability narrows by `introducedNames`
into the recipient's pet store and shares the agent's strong
dependency on its own pet store, so individual intervals do not need
their own GC edges.

## Conformance with the Capability Bank's Six Design Principles

[`daemon-capability-bank.md`](daemon-capability-bank.md) § Design
Principles names the rubric every capability in the bank satisfies.
This design's mapping:

1. **Capabilities are objects, not configurations.**
   The agent holds an `Interval` (or, at most, a `Scheduler` whose
   policy was set at grant time).
   It does not hold "a scheduler service configured with allowed
   labels and forbidden periods" — there is no descriptor to misread.

2. **Recursive attenuation.**
   The host narrows by granting a single `Interval` rather than the
   whole `Scheduler`; the per-`Interval` `setPeriod` cannot raise the
   period above the policy set at `makeInterval` time; cancellation
   on the `Interval` revokes only that interval, not the others.
   Authority shrinks by handing out sub-capabilities, not by adding
   deny patterns.

3. **Caretaker separation.**
   `Scheduler` and `SchedulerControl` are separate exo facets of one
   underlying scheduler formula.
   The host can pause, resume, revoke, or tighten limits without the
   guest's cooperation; the guest cannot discover or influence the
   control facet.
   This matches the
   [`daemon-mount.md`](daemon-mount.md) host-facet / guest-facet split
   the bank already follows.

4. **Defense-in-depth deny patterns are optional.**
   `maxActive` and `minPeriodMs` are policy knobs on the control
   facet, not hardcoded denylists.
   They are a safety net against grant-time misconfiguration, not the
   primary confinement mechanism.
   The primary mechanism is structural: an agent holds an `Interval`
   only because the host chose to grant it.

5. **LLM discoverability.**
   Every exo carries `help()` written for an LLM encountering it cold.
   `M.interface()` guards name the exact shapes (`'active' | 'paused' |
   'cancelled'` is enumerated, `IntervalEntry` is a named record, all
   remotables are tagged).
   An agent introspecting `__getMethodNames__()` discovers the
   capability surface without duck-typing.

6. **Existing Endo patterns.**
   Tick delivery rides the daemon's existing mail system (per
   [`daemon-value-message.md`](daemon-value-message.md)); coalescing
   rides the daemon's existing `serial-jobs` queue; persistence rides
   the established disk-before-graph lifecycle and atomic write-then-
   rename helpers; capability narrowing rides the pet-name granting
   pattern from [`daemon-agent-tools.md`](daemon-agent-tools.md).
   No parallel abstractions are introduced.

## Dependencies

| Design                                                          | Relationship                                                                              |
| --------------------------------------------------------------- | ----------------------------------------------------------------------------------------- |
| [endoclaw-timer](endoclaw-timer.md)                             | **Superseded by this design.** Phase 1 prototype in `packages/genie/src/interval/` is the predecessor this design graduates. |
| [daemon-capability-bank](daemon-capability-bank.md)             | Parent. The scheduler is the Timer / scheduling slot of the capability family.            |
| [daemon-value-message](daemon-value-message.md)                 | Tick envelope follows the typed-message-plus-one-shot-capability pattern this design established. |
| [daemon-commands-as-messages](daemon-commands-as-messages.md)   | Tick messages share the inbox with command messages; the same prompt classifier routes both. |
| [daemon-agent-tools](daemon-agent-tools.md)                     | Pet-name granting pattern (`endo grant ...`) the scheduler reuses for per-`Interval` narrowing. |
| [genie-integration](genie-integration.md)                       | The integration survey that motivated this design; references this design from its § 3.   |
| [endoclaw-proactive-messages](endoclaw-proactive-messages.md)   | **Depends on this design.** Composes scheduled ticks with data capabilities for briefings. |

## Implementation Phases

### Phase 1: Core scheduler exo (S)

- Define `SchedulerFormula` in `packages/daemon/src/types.d.ts`.
- Add `scheduler` to the `Formula` discriminated union.
- Implement the maker returning `Scheduler` / `SchedulerControl`
  facets with interface guards.
- Add `scheduler` to the `makers` table in `daemon.js`.
- Add `extractDeps` case for `scheduler`.
- Persistence: per-`Interval` JSON files in the scheduler directory;
  limits and pause state on the formula itself.
- `makeInterval()` creates entries, persists, arms `setTimeout`.
- `cancel()` disarms and marks cancelled.
- Limit enforcement: `maxActive`, `minPeriodMs`.
- Cancellation context integration.

### Phase 2: Tick delivery and response (S)

- Add `interval-tick` to the `MessageFormula` type union.
- Implement `deliverIntervalTickMessage()` using the existing
  `post()` pathway in `mail.js`.
- Create a handle for the scheduler so tick messages carry a valid
  `from` identity.
- Implement the `TickResponse` one-shot exo with `resolve()` /
  `reschedule()`.
- Implement tick timeout with auto-resolve and warning logging.
- Implement exponential backoff on `reschedule()`.

### Phase 3: Startup recovery (S)

- In `seedFormulaGraphFromPersistence()`, when a `scheduler` formula
  loads, read its entry files and re-arm active intervals.
- Compute `missedTicks` for intervals that should have ticked during
  downtime.
- Deliver a single coalesced catch-up `interval-tick` message with
  `missedTicks > 0`.

### Phase 4: Host integration and CLI (S)

- Add `makeScheduler()` to `HostInterface` and implement in
  `packages/daemon/src/host.js`.
- Wire `SchedulerControl.pause()` / `resume()` / `revoke()` through
  the host facet.
- CLI: `endo scheduler list <agent>`, `endo scheduler pause <agent>`,
  `endo scheduler resume <agent>`, `endo grant <agent> daily-reflect
  scheduler 86400000`.
- Update genie's daemon plugin to provision a scheduler via
  `makeScheduler()` and grant the agent a single `daily-heartbeat`
  `Interval` rather than introducing the whole scheduler.

### Phase 5: Retire the genie prototype (S)

- Replace `makeIntervalScheduler` consumers in
  `packages/genie/main.js` and `packages/genie/src/heartbeat/` with
  calls into the granted `Interval` capability.
- Delete `packages/genie/src/interval/scheduler.js`,
  `persistence.js`, `types.js`, `index.js`, and the
  `<workspace>/intervals/` per-agent persistence directory.
- Drop the `drainPendingHeartbeats` coalescing code from
  `runAgentLoop` (the scheduler's `serial-jobs`-backed coalescing
  replaces it).
- Update [`endoclaw-timer.md`](endoclaw-timer.md) status to
  *Superseded by [scheduler](scheduler.md)*.

## Design Decisions

1. **Tick events are messages, not iterator values.**
   Delivering through the mail system gives persistence, ordering,
   and replay for free.
   An `AsyncIterator<Tick>` interface would require a new delivery
   mechanism and would not interleave naturally with other agent
   messages.

2. **Start-to-start timing, not end-to-start.**
   Cadence is consistent regardless of processing time.
   End-to-start would drift.

3. **Resolve / reschedule, not fire-and-forget.**
   The scheduler has visibility into agent health; transient failures
   retry with backoff within the current period; timeout auto-resolve
   prevents a stuck agent from stalling the heartbeat.

4. **Immediate first tick by default.**
   `firstDelayMs` defaults to 0 so the agent's first heartbeat
   initialises state immediately rather than after an arbitrary
   delay.

5. **No cron semantics.**
   The scheduler knows about periods, not calendar policy.
   Higher-level scheduling ("run at 08:00 daily", "skip weekends") is
   the agent's concern.

6. **Missed ticks are coalesced, not replayed.**
   An interval that missed four ticks during downtime delivers *one*
   message with `missedTicks: 4`.

7. **Pause suppresses, not defers.**
   Ticks during a pause are lost, not queued; resume does not flush
   suppressed ticks.

8. **Revocation is permanent.**
   To restore interval access, the host creates a new scheduler.

9. **One scheduler per agent, not per interval.**
   Individual intervals are entry files within the scheduler
   directory, not separate formulas. The scheduler is the unit of GC.

10. **No sub-second intervals.**
    `minPeriodMs` floor is 1000ms; default is 60 000 ms.
    Sub-second cadence is not a useful agent workload and is bounded
    out.

Decisions 1–10 carry forward verbatim from
[`endoclaw-timer.md`](endoclaw-timer.md) § Design Decisions; this
section restates them so the design stands alone for a future
implementer.

## Open Questions

- **Replace `timer` or live alongside it.**
  The existing `timer` formula type
  (`packages/daemon/src/types.d.ts`, `formulateTimer` in
  `packages/daemon/src/host.js`) is used by host code that hasn't
  been audited as part of this design.
  Conservative: add `scheduler` as a new formula type and deprecate
  `timer` once nothing depends on it.
  Aggressive: extend `timer` itself into `scheduler` and break the
  existing API.
  Recommended: the conservative path; the migration of the existing
  `timer` consumers is a separate, smaller PR.

- **Per-`Interval` `setPeriod` ceiling.**
  Today an `Interval` can `setPeriod` to any value above the
  scheduler's `minPeriodMs`.
  Should the host be able to set a *per-interval* maximum at
  `makeInterval` time, so a `daily-reflect` interval cannot become a
  100ms interval even if the host's scheduler-wide `minPeriodMs` is
  1000ms?
  The principle-of-least-authority answer is yes; the implementation
  cost is small (a `maxPeriodMs` opt field).

- **Tick message envelope shape.**
  `interval-tick` is proposed as a new `MessageFormula` variant.
  An alternative is to use the existing `type: 'package'` shape with
  a structured body carrying the tick metadata.
  The advantage of a new variant is that the agent's prompt
  classifier can route tick messages distinctly from user mail
  without parsing the body; the cost is one more message type the
  daemon's mail layer must know about.
  Recommend the new variant; the parsing-strings-with-prefixes hack
  the genie prototype uses today (`/heartbeat <tickID>`) is exactly
  the failure mode the typed envelope avoids.

- **Whether `Scheduler` itself is grantable.**
  The principle-of-least-authority shape grants per-`Interval`
  handles, not the whole `Scheduler`.
  Some agent harnesses (genie's main loop on its own host) may want
  to hold a scoped `Scheduler` so they can create intervals
  dynamically.
  Granting `Scheduler` is supported and safe under the existing
  `maxActive` / `minPeriodMs` policy; the design does not foreclose
  it.
  The default in genie / lal / fae documentation should be "grant an
  `Interval`", with "grant a `Scheduler`" reserved for harnesses
  that need dynamic creation.

## Prompt

> Propose a scheduler design that closes the gap between what the
> daemon currently provides and what an integrated agent would need.
> This should amount to a prerequisite refactor for the genie
> integration. Include the design in the genie-integration PR.
> Let's simply call it 'scheduler'.

(Per maintainer review comments
[`pull/89#discussion_r3369682114`](https://github.com/endojs/endo-but-for-bots/pull/89#discussion_r3369682114)
and
[`pull/89#discussion_r3369682742`](https://github.com/endojs/endo-but-for-bots/pull/89#discussion_r3369682742)
on `endojs/endo-but-for-bots#89` 2026-06-08.)
