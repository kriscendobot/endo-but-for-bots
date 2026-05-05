# Genie Integration Survey

| | |
|---|---|
| **Created** | 2026-05-02 |
| **Updated** | 2026-05-02 |
| **Author** | Kris Kowal (prompted) |
| **Status** | Proposed |

## What is the Problem Being Solved?

`@endo/genie` was built as a Claw-like agent harness wrapped around the
external `@mariozechner/pi-agent-core` and `@mariozechner/pi-ai`
libraries.
It runs as an unconfined daemon caplet today (`packages/genie/main.js`)
but carries its own implementations of LLM dispatch, scheduling,
filesystem confinement, full-text search, conversation memory, and a
heartbeat loop.

Its sibling agent harnesses `@endo/lal` and `@endo/fae` solve overlapping
problems with parallel, hand-rolled code:
four bespoke LLM providers in `lal/providers/` (Anthropic, Gemini,
llama.cpp, Ollama), a duplicated tool-call extractor in
`fae/src/extract-tool-calls.js`, hand-coded path-traversal guards in
`fae/src/tool-makers.js`, and no scheduling at all (lal and fae are
purely reactive — they only wake when mail arrives).

Each harness reinvents the same primitives because none of them depend
on the daemon's already-supplied capabilities (`Mount`, `ScratchMount`,
`ReadableTree`, `timer` formula, pet-name namespaces, formula graph,
SQLite store).
This design surveys what genie does, identifies the components that
should move into the daemon as first-class capabilities, identifies the
ones that should be shared with the lal/fae harnesses, and identifies
the rest.

## Survey of Genie Components

This survey treats each subdirectory of `packages/genie/src/` as a
component.
Code paths and external dependencies are listed for each.

### `agent/` — pi-agent-core wrapper

The "engine" that turns a user prompt and a set of tools into a stream
of `ChatEvent`s.
`makePiAgent` resolves a model string into a pi-ai `Model` object,
constructs a system prompt via `buildSystemPrompt`, wraps tool specs
into `AgentTool` shape, and instantiates a pi-agent-core `Agent`.
`runAgentRound` subscribes to that Agent's event stream and yields
adapter-friendly events (`Message`, `Thinking`, `ToolCallStart`,
`ToolCallEnd`, `Error`).
`tool-gate.js` watches for expected tool calls so the observer/reflector
can retry-prompt when the LLM forgets.
External deps: `@mariozechner/pi-agent-core` (Agent loop, tool dispatch,
streaming) and `@mariozechner/pi-ai` (model registry, provider
adapters, API key plumbing).
The Ollama path is handled locally by masquerading as the openai
provider with a custom `baseUrl`.

### `loop/` — adapter-agnostic chat loop

A small coordination layer.
`run.js` (`runGenieLoop`) is parametric over a `Chunk` type and an `IO`
adapter; it pulls inbound prompts, classifies each as `'user' |
'special' | 'heartbeat'`, dispatches, drains output, calls `dismiss`,
and signals idle/busy.
`builtin-specials.js` packages the `/help`, `/tools`, `/observe`,
`/reflect`, `/clear`, `/exit`, `/heartbeat` slash commands as a reusable
handler bundle; `specials.js` is the prefix-aware dispatcher.
`agents.js` (`makeGenieAgents`) bundles the main, heartbeat, observer,
and reflector PiAgent instances.
External deps: only the genie internals plus pi-agent-core types.

### `heartbeat/` — periodic LLM round

`runHeartbeat` runs a single LLM round with a fixed prompt that asks
the agent to read `HEARTBEAT.md`, work pending tasks, and reply with
`HEARTBEAT_OK`.
Records the result to `<workspace>/.heartbeats.log` via direct
`fs.writeFile`.
External deps: `node:fs/promises`, the agent loop.

### `interval/` — first prototype of `endoclaw-timer`

A complete Go-Ticker-style scheduler.
`makeIntervalScheduler` exposes `makeInterval(label, periodMs, opts)`,
returns a host-side `IntervalControl` facet (`pause`, `resume`,
`revoke`, `setMaxActive`, `setMinPeriodMs`) and an agent-side
`IntervalScheduler` facet.
Each tick is delivered via an `onTick` callback with a `tickResponse`
that the agent must `resolve()` or `reschedule()` before the next tick
fires.
Persists entries to a `persistDir` so missed ticks during downtime are
coalesced and replayed.
External deps: `node:timers`, file persistence in `persistence.js`.
The `designs/endoclaw-timer.md` document already declares this as the
prototype that should graduate to the daemon.

### `observer/` — conversation-to-memory compressor

A background PiAgent that runs when the main agent's unobserved
messages exceed a token threshold (default 30 000) or after an idle
delay (default 2 min).
Reads the main agent's `messages` array directly, serialises an
excerpt, and writes prioritised observations to
`memory/observations.md` via the `memorySet` tool.
External deps: pi-agent-core (sub-agent), `node:timers`, the genie
memory tools.

### `reflector/` — observation-to-knowledge consolidator

A background PiAgent that fires from heartbeat or when
`observations.md` exceeds a token threshold (default 40 000).
Reads observations, reflections, and profile files; merges, prunes
stale entries, promotes durable facts, regenerates `profile.md`.
Writes to `memory/observations.md`, `memory/reflections.md`, and
`memory/profile.md`.
External deps: pi-agent-core, the genie memory tools.

### `system/` — system prompt builder

Generator-based assembly of a runtime-info / policy / tools / memory
system prompt.
External deps: only `@endo/harden`.

### `tools/` — tool registry, command/file/memory/web tools, FTS5 backend

Genie's tool surface.
`registry.js` builds a configurable `GenieTools` bundle (`bash`,
`exec`, `git`, file tools, memory tools, web tools).
`memory.js` implements `memoryGet`, `memorySet`, `memorySearch` over a
pluggable `SearchBackend` (substring or FTS5).
`fts5-backend.js` is a `better-sqlite3` FTS5-backed search index.
`vfs.js` defines a `VFS` interface that all file tools route through;
`vfs-node.js` and `vfs-memory.js` are implementations.
`command.js` runs shell commands with allow/deny policies and path
enforcement.
`web-fetch.js` and `web-search.js` are bare network-egress tools.
External deps: `better-sqlite3`, `node:child_process`, `node:fs`,
`@endo/patterns`.

### `dom-parser/` — minimal `DOMParser` shim

A Node-side `DOMParser` implementation with `querySelector` and
`textContent`.
Used by `web-search.js` and `web-fetch.js` to scrape HTML.
External deps: pure JS.

### `workspace/init.js` — template seeding

Copies the contents of `packages/genie/workspace_template/` into the
agent's workspace directory on first spawn so `MEMORY.md`,
`HEARTBEAT.md`, etc. exist.

### `utils/tokens.js` — `chars/4` token estimator

Three-line heuristic, used by observer, reflector, and the agent
module to gate sub-agent triggers.

### Top-level wiring

`main.js` is the daemon-side guest caplet: receives a configuration
form on `@host`, spawns sub-guests per configured agent, wires each
guest's inbox through `runGenieLoop`, registers heartbeat ticks via
`makeIntervalScheduler`, and self-sends `/heartbeat <tickID>` mail
messages to drive the heartbeat loop.
`dev-repl.js` is a parallel stdin/stdout host (not surveyed in detail
here; it shares everything in `loop/` with `main.js`).

## 1. The Pi Engine

### What pi gives genie today

`@mariozechner/pi-ai` ships a model registry covering Anthropic, Google
Gemini (direct and via Vertex / Gemini-CLI), Amazon Bedrock,
Azure OpenAI Responses, Mistral, OpenAI completions, OpenAI Responses,
OpenAI Codex Responses, and (via the openai-completions adapter with a
custom `baseUrl`) Ollama, llama.cpp, and any OpenAI-compatible local
server.
`@mariozechner/pi-agent-core` provides the agentic loop itself: streamed
text and thinking deltas, tool calling with sequential or parallel
execution, tool-update events, retry/abort handling, and a uniform
event API across providers.

`packages/genie/src/agent/index.js` is a thin (~250 lines after
boilerplate) wrapper over both.
The wrapper translates pi events into genie's own `ChatEvent` shape and
adds an Ollama Model construction that should arguably live in pi-ai's
own openai-completions adapter.

### What lal and fae do instead

`packages/lal/providers/` is a hand-written, non-streaming alternative
to pi-ai with four backends: `anthropic.js` (~166 lines),
`gemini.js` (~174), `llamacpp.js` (~98), `ollama.js` (~143), plus
`config.js` (~69) for URL/host detection, totalling **~650 LOC** of
provider plumbing.
The dispatch loop in `lal/agent.js` (~1800 LOC, `runAgenticLoop`) is a
hand-rolled non-streaming agentic loop that calls `provider.chat()` with
the OpenAI-style tools array, parses tool calls (including a
fallback regex extractor for models that emit `<tool_call>` blocks
inline), and feeds results back as `role: 'tool'` messages.
`packages/fae/agent.js` re-imports `lal`'s provider via
`import { createProvider } from '@endo/lal/providers/index.js'` and
duplicates the same loop with a different tool registry and a
conversation-tree memory layer (`@endo/conversation-tree`).
`packages/fae/src/extract-tool-calls.js` (~188 LOC) is a near-duplicate
of the same regex-fallback extractor in `lal/agent.js`.

### Integration proposal

Route lal and fae through the same engine genie uses.
Concretely:

1. **Extract** the genie agent module (`agent/index.js` plus
   `agent/tool-gate.js`) into a new package — call it
   `@endo/llm-engine` — that owns the pi-ai/pi-agent-core dependency.
   Keep the pi packages as-is for now; they are battle-tested and the
   dependency is easy to vend.
   The extracted package's surface is approximately:
   `makeAgent(opts)`, `runAgentRound(agent, prompt)`, the `ChatEvent`
   union, `getMessageTokenCount(agent)`, a tool spec normalizer, and a
   provider/model resolver.
2. **Move** the Ollama-as-openai adapter into the same package
   (or upstream into pi-ai) so callers don't need to know the trick.
3. **Rewrite** lal's `spawnWorkerLoop` and fae's `spawnWorkerLoop`
   against `@endo/llm-engine`, keeping their existing tool registries
   and inbox-mail driver intact.
   The transcript-tree code (`makeConversationTree` in fae,
   `getNode/putNode/assembleTranscript` in lal) is orthogonal to the
   engine and stays in place — those modules just need to accept the
   engine's message format instead of producing OpenAI-style messages
   themselves.
4. **Delete** `packages/lal/providers/` and
   `packages/fae/src/extract-tool-calls.js`.

What lal and fae *gain*:

- **Streaming** — pi-agent-core delivers `text_delta` and
  `thinking_delta` events.
  Today lal and fae block until the entire response arrives.
  Streaming is what makes the daemon-side "Thinking…" status messages
  in genie's `processMessage` possible.
- **Provider breadth** — Bedrock, Vertex, OpenAI Responses, Mistral,
  Codex Responses, and any OpenAI-compatible endpoint (lal already
  routes Ollama through llama.cpp's openai-compatible API; pi-ai does
  this natively).
- **Reasoning support** — pi-agent-core knows about Anthropic extended
  thinking and OpenAI reasoning tokens and exposes them as
  `Thinking` events.
  lal and fae have no concept of thinking blocks today.
- **Tool-call normalization** — the regex fallback for
  `<tool_call>` blocks is owned by pi-agent-core upstream rather than
  duplicated in fae and lal.

What lal and fae *lose*:

- One indirection to a third-party package (pi-ai/pi-agent-core).
  This is the load-bearing risk.
  Mitigation: pin specific minor versions, pre-bundle for the daemon if
  the worker can't reach npm at runtime, and consider eventually
  vendoring the relevant subset if pi-ai's surface stabilizes.
- The bespoke "depth prefix" hack in lal's `reply` tool (which prepends
  `[depth:N]` based on transcript chain length) needs to be expressed
  as middleware around the engine's tool-call path, not inside
  `executeTool`.
  This is mechanical.
- fae's `replyTracker.sent` flag and "auto-reply if the LLM produced
  text but didn't call reply()" fallback need to be expressed as a
  `runAgentRound` consumer that watches for the final assistant
  `Message` event.
  Also mechanical.

### Code-deletion estimate

| Package | Lines deletable | What goes |
|---|---|---|
| `packages/lal/providers/` | ~650 | All four providers + config.js |
| `packages/lal/agent.js` | ~150 | `extractToolCallsFromContent`, the hand-rolled message-shape conversions, the SmallCaps tool-call decoder if pi-agent-core handles parsing natively |
| `packages/fae/src/extract-tool-calls.js` | ~188 | Whole file |
| `packages/fae/agent.js` | ~80 | `processToolCalls` shape conversion, the inline tool-call re-extraction in `runAgenticLoop` |
| `packages/genie/src/agent/index.js` | net 0 | Moves to `@endo/llm-engine`, no deletion |
| **New `@endo/llm-engine`** | **+~500** | Hosts the moved code |
| **Net** | **~−570 LOC** | |

The deletion isn't dramatic in absolute terms but it removes the most
fragile category of code in the repo (provider-specific HTTP wire
formats and ad-hoc tool-call regex extraction) and consolidates LLM
upgrades (new providers, new model families, new reasoning-token APIs)
to a single chokepoint.

## 2. Memory

### How genie persists and recalls memory today

Memory is a **bag of markdown files in the agent's workspace
directory**, plus an **FTS5 SQLite index** in the same directory.
The schema is informal:

- `MEMORY.md` — single-file long-term memory (whatever the agent
  writes).
- `memory/observations.md` — written by the observer sub-agent.
- `memory/reflections.md` — written by the reflector sub-agent.
- `memory/profile.md` — written by the reflector sub-agent.
- `memory/<topic>.md` — agent-authored topic notes.

Access is through three tools — `memoryGet`, `memorySet`,
`memorySearch` — that route through a `VFS` abstraction
(`vfs-node.js` for Node `fs`, `vfs-memory.js` for tests).
`makeMemoryTools` owns an in-memory index queue and worker that pushes
content into a pluggable `SearchBackend`; the daemon plugin uses
`makeFTS5Backend` (a `better-sqlite3` FTS5 virtual table at
`<workspace>/memory-fts.db`).

The path-confinement story is genie's own `safePath()` — basically
`vfs.resolve()` plus a null-byte check.
This is exactly the problem `daemon-mount` was built to solve.

### What it would mean to host this in the daemon

Replace the workspace/VFS/FTS5 stack with three already-existing daemon
primitives: **`Mount`** (or `ScratchMount`) for the file storage,
**pet-name directories** for the namespace, and a new **`memory-index`
formula** (or capability backed by the daemon's existing SQLite store)
for the FTS5 layer.

Concretely:

- The agent guest has a **`memory` pet name** that resolves to a
  daemon-managed `ScratchMount` (no host-side path needed) or to a
  `Mount` over a user-supplied directory if the agent should share
  memory with on-disk files.
  All `memoryGet`/`memorySet`/`memorySearch` calls go through that
  capability instead of through `process.cwd()` and `node:fs`.
  Path confinement, `..` clamping, symlink containment, and read-only
  attenuation come from `Mount` for free — no `safePath()` to write or
  audit.
- **Pet-name structure** replaces the magic-filename layout.
  Instead of "agents look for `memory/observations.md`", the observer
  is granted a pet name `observations` that points to a single
  writable file capability (a `MountFile` or a content-addressed
  `ReadableBlob` written by `memorySet`).
  The reflector gets `observations`, `reflections`, `profile` as
  separate pet names with whatever attenuation we want
  (read-only `observations`, write `reflections`).
  The user (or a parent agent) controls the namespace; the sub-agents
  see only what they were granted.
- **Search** lives as a daemon-side `memory-index` capability
  (proposed new formula type) that the agent can `subscribe` files to
  via the same pattern as the existing `index` queue.
  The daemon already ships `better-sqlite3` and a SQLite store
  (`packages/daemon/src/sqlite-*.js`); the FTS5 virtual table can sit
  alongside the daemon's existing tables instead of in a per-workspace
  `memory-fts.db` file the agent can stomp on.

### Example

Today the genie daemon plugin spawns a guest like this
(`main.js:923`):

```js
agentGuest = await E(hostAgent).provideGuest(agentName, {
  agentName: profileName,
  introducedNames: harden({ 'workspace-mount': 'workspace' }),
});
// ... sub-guest then calls makeMemoryTools({ root: workspaceDir }) directly,
// reaching outside the daemon's capability fence to read/write files.
```

Under the proposal, the same guest would be set up as:

```js
// On first spawn, the host (or a parent agent) prepares the namespace:
await E(hostAgent).provideScratchMount('main-genie-memory');
await E(hostAgent).provideMemoryIndex('main-genie-memory-index', {
  mount: 'main-genie-memory',
});

// The guest is granted just those two pet names:
agentGuest = await E(hostAgent).provideGuest(agentName, {
  agentName: profileName,
  introducedNames: harden({
    'main-genie-memory': 'memory',
    'main-genie-memory-index': 'memory-index',
  }),
});
```

The genie code that today reads:

```js
const result = await memoryGet.execute({ path: 'memory/observations.md' });
```

becomes:

```js
const memory = await E(powers).lookup('memory');
const text = await E(memory).readText(['observations.md']);
```

and `memorySearch` becomes:

```js
const index = await E(powers).lookup('memory-index');
const hits = await E(index).search('user preferences', { limit: 5 });
```

The observer can be granted a *narrowed* `memory` capability that only
permits writes to `observations.md` (a `MountFile` capability) while
the main chat agent gets the read-only `Mount.snapshot()` of memory —
the same enforcement the genie observer's `tool-gate.js` tries to
achieve at the prompt level can now be done at the capability boundary.

### What this respects from the daemon CLAUDE.md

- **Disk before graph** — `provideScratchMount` and the proposed
  `provideMemoryIndex` follow the existing `formulateMount` pattern:
  the formula is written to disk before the in-memory graph entry.
  No new lifecycle to design.
- **Pet-name semantics** — the introduced-name path is the same
  one `daemon-mount.md` Phase 5 already implements.
  `memory` is a pet name; `memory-index` is a pet name; both can be
  renamed, copied, or removed by the host without the agent noticing.
- **Special names off-limits** — the namespace is plain pet names
  (no `@`-prefix), matching the introducedNames rule from the daemon
  CLAUDE.md.
- **Mount semantics** — `Mount.has`, `list`, `lookup`, `readText`,
  `writeText`, `remove`, `move`, `makeDirectory`, `readOnly`,
  `snapshot` cover everything `memoryGet`/`memorySet` need.
  The only gap is that `Mount` works on individual files; a "fetch
  lines N..M" helper would need to be added either to `Mount` or to a
  thin agent-side adapter.

### Open question on storage shape

There is a tension between "memory is markdown files an agent
edits in-place" (the current model) and "memory is a structured store
that the daemon owns and the agent manipulates through capabilities"
(the daemon-native model).
A pure daemon-native version might use `ReadableBlob`s (immutable,
content-addressed) for each observation snapshot, with a directory of
the latest version per topic.
That respects daemon GC and cross-peer sync (`daemon-cross-peer-gc` is
complete and would Just Work) but means the agent sees a different
shape than "edit observations.md".
For the first cut, **stick with `ScratchMount` and let memory be live
markdown files** so the agent's mental model from
`packages/genie/workspace_template/MEMORY.md` etc. doesn't change.
Move to blob-per-snapshot in a later phase if cross-peer memory
mirroring becomes interesting.

## 3. Scheduling

### How genie schedules work today

Genie has *one* scheduling primitive — the interval scheduler in
`packages/genie/src/interval/`.
It is wired into the daemon-side guest exactly once: each agent gets a
single heartbeat interval whose tick callback fires
`E(agentGuest).send('@self', ['/heartbeat <tickID>'])` so the
heartbeat round arrives in the same FIFO inbox as user mail.
The agent-loop's prompt classifier then routes `/heartbeat …` messages
to a heartbeat handler that runs the heartbeat sub-agent.
Lal and fae have **no equivalent** — they only run when mail arrives,
so they cannot self-schedule a daily memory-reflection or a
periodic background task.

The interval scheduler is already half-aware that it should graduate
to the daemon: `designs/endoclaw-timer.md` calls it the prototype and
notes that "Going forward this facility can graduate out to a proper
@endo/xxx package, or maybe just move into @endo/daemon more
generally."
The daemon already has a `timer` formula type
(`packages/daemon/src/types.d.ts:387`) and a `formulateTimer` host
method (`packages/daemon/src/host.js:761`), but it is the simpler
"fire-and-forget every N ms with subscribers" model from the original
endoclaw-timer Phase 0 — no resolve/reschedule, no missed-tick
coalescing, no per-tick deadline, no `IntervalControl` host facet, no
pause/resume.

### Reframing as daemon formulas

The genie interval scheduler should land as a richer `interval-scheduler`
formula type in the daemon, supplanting (or generalising) the existing
`timer` formula:

- **Formula type:** `interval-scheduler`.
  Persisted entry shape mirrors the existing `IntervalEntry` typedef.
  Each individual interval is itself a per-formula entry rather than a
  shared scheduler — or, more naturally, the scheduler is a single
  formula that owns multiple labeled intervals and exposes them through
  pet names.
  The latter matches genie's current `IntervalSchedulerFacet` /
  `IntervalControlFacet` split and is the right shape for a host-managed
  resource.
- **Host method:** `provideIntervalScheduler(petName, opts)` returns a
  scheduler capability.
  `IntervalControl` (pause, resume, revoke, setMaxActive,
  setMinPeriodMs) is retained by the host; the scheduler facet is
  granted via pet name to a guest.
- **Tick delivery:** The genie prototype delivers ticks through an
  `onTick` callback today and then bridges to mail with a side-channel
  `pendingHeartbeatTicks` map.
  The daemon-native version should deliver each tick directly as a
  `type: 'package'` message to a configurable recipient — by default
  the holder of the scheduler capability.
  The `tickResponse.resolve()` / `reschedule()` map onto an
  `E(scheduler).resolveTick(tickId)` /
  `E(scheduler).rescheduleTick(tickId)` round-trip, eliminating the
  side-channel map entirely.
- **Persistence:** lives in the daemon's existing state directory
  (likely a new SQLite table or a per-formula JSON file alongside the
  formula itself), not a `persistDir` argument the agent has to supply.
  Restart recovery (replay missed ticks) and the existing
  `withFormulaGraphLock` pattern are already established.
- **Capability narrowing:** the agent doesn't see "the scheduler"; it
  sees a pet name like `daily-reflect` whose value is an
  `IntervalHandle` for a single pre-configured interval that fires once
  a day with a specific label.
  This is the **principle-of-least-authority** angle: a host can grant
  an agent the right to *be told once a day* without granting the right
  to create or cancel arbitrary intervals.
  Today `setInterval` would let an agent burn the host's CPU; a granted
  `IntervalHandle` cannot.

### What lal and fae get

Both gain the ability to schedule:

- A **heartbeat** equivalent — daily summary, periodic context refresh.
- **Idle observation** — fae's `replyTracker.sent` fallback could be
  paired with a "no message in 5 minutes → run memory consolidation"
  scheduled task.
- **Cross-agent triggers** — a parent agent can schedule a child by
  granting it an `IntervalHandle` whose ticks fire into the child's
  inbox.

### Subordinate use case: `serial-jobs`

The daemon already uses `serial-jobs` (`packages/daemon/src/serial-jobs.js`,
imported in `daemon.js` and `mail.js`) as an internal task queue.
Genie's heartbeat coalescing logic in `runAgentLoop`
(`drainPendingHeartbeats` etc.) is essentially a hand-rolled
single-consumer serial-jobs queue.
A daemon-hosted `interval-scheduler` formula could expose a
"coalescing on" mode that internally uses `serial-jobs` to ensure at
most one tick is in flight per label, eliminating that code from genie
entirely.

### Is this `serve-private-path` territory?

`serve-private-path` is the daemon's UNIX-socket CapTP server, not a
job-scheduling primitive — it does not apply directly to scheduling.
Including it in the prompt was a slight category error.
The relevant primitives are `timer` (existing, simple) and
`serial-jobs` (existing, internal); the proposal here is to upgrade
`timer` into a proper `interval-scheduler` modelled on the genie
prototype.

## 4. Other Components — Integrate / Share / Leave / Retire

| Component | Verdict | Rationale |
|---|---|---|
| `agent/` (pi wrapper) | **Share** | Extract to `@endo/llm-engine`; lal & fae adopt it. See § 1. |
| `agent/tool-gate.js` | **Share** | Generic retry helper; ships with `@endo/llm-engine`. |
| `loop/run.js` (`runGenieLoop`) | **Share** | Already adapter-agnostic. Rename to `@endo/agent-loop` or live under `@endo/llm-engine`; lal and fae's hand-rolled message-iteration loops can be replaced by it. |
| `loop/builtin-specials.js` | **Share** | The `/help`, `/tools`, `/observe`, `/reflect` commands are the same set lal and fae would mount once they have memory. |
| `loop/specials.js`, `loop/io.js` | **Share** | Tiny dispatch & adapter contracts; ship alongside `runGenieLoop`. |
| `loop/agents.js` (`makeGenieAgents`) | **Share** | Once the engine is shared, this is the right place to bundle main + heartbeat + observer + reflector for any harness. |
| `heartbeat/` | **Integrate-into-daemon (partly)** | The *prompt* and the OK-token convention are policy and stay agent-side. The *log file* (`.heartbeats.log`) should move to a daemon-managed log capability — today it's a fragile `fs.writeFile` outside any confinement. Or just send the heartbeat result as a value-message to the host, dropping the file. |
| `interval/` | **Integrate-into-daemon** | See § 3 — graduate to `interval-scheduler` formula. Already the explicit plan in `endoclaw-timer.md`. |
| `observer/` | **Share** | Generic memory-compaction sub-agent; lives wherever `@endo/llm-engine` lives so lal and fae can opt in. The system prompt is generic enough to work for any agent. |
| `reflector/` | **Share** | Same as observer. |
| `system/` (prompt builder) | **Share** | Useful to all harnesses. lal has a hard-coded prompt and fae has a hard-coded prompt; both could be assembled from this builder with their own identity/policy fragments. |
| `tools/registry.js` | **Share** | Tool-bundle assembly is harness-agnostic; gives lal and fae a way to select from an already-vetted tool catalog. |
| `tools/memory.js` | **Integrate-into-daemon** | Replace the `VFS`-based path layer with `Mount`/`ScratchMount`. The `SearchBackend` interface stays; `memorySearch` becomes a thin caller of a daemon-side `memory-index` capability. See § 2. |
| `tools/fts5-backend.js` | **Integrate-into-daemon** | Move into the daemon as the implementation of the new `memory-index` formula type (the daemon already depends on `better-sqlite3`). |
| `tools/vfs.js`, `vfs-node.js`, `vfs-memory.js` | **Retire** | The daemon's `Mount` already provides path confinement, symlink handling, and read/write streaming. The in-memory `vfs-memory.js` is useful for the package's tests; keep it inside the genie test directory if anywhere. |
| `tools/filesystem.js` | **Share / Integrate** | The tool *shape* (readFile/writeFile/editFile/etc. with offset/limit) is what lal and fae need. The *implementation* should call into `Mount` capabilities, not raw `fs`. Then bundle the resulting tools as a shareable `@endo/agent-fs-tools` (or include in the engine package) so all three harnesses use them. |
| `tools/command.js` | **Share** | The bash/exec/git tools and the policy/path-enforcement helpers (`rejectPatterns`, `rejectFlags`, `enforcePath`) are useful to lal and fae. fae has a `run-command.js` tool of its own that overlaps. Note: actually executing shell commands inside the daemon is a separate confinement concern (`daemon-os-sandbox-plugin`); the *tool wiring* can be shared regardless. |
| `tools/web-fetch.js`, `tools/web-search.js` | **Share, with caveat** | Network egress is exactly what `endoclaw-network-fetch` is meant to solve. Until that lands, share the genie implementations as-is; once `HttpClient` exists, swap the implementation under the same tool surface. |
| `dom-parser/` | **Leave-in-place / Retire** | Used only by `web-search.js` to scrape DuckDuckGo. If `web-search` moves to a hosted search API or to a JSON-returning back end, retire `dom-parser` entirely. Otherwise it's a self-contained Node DOMParser shim that nobody else needs. |
| `system/` system prompt suffix/policy | **Share** | The "tool output format / no tags in responses" policy is generic. |
| `workspace/init.js` | **Retire** (after § 2 lands) | When memory is a daemon `ScratchMount` granted by the host, there is no per-agent `workspace_template` to seed. The host (or a setup script) creates the initial `MEMORY.md` and `HEARTBEAT.md` files via `Mount.writeText` once. |
| `utils/tokens.js` | **Share** | One-liner; ship with the engine package. |
| `main.js` (the daemon plugin) | **Leave-in-place** | The plugin is genie-specific (form prompts, agent-name conventions, the `/heartbeat` system self-send). Once components 1–3 are shared, this file shrinks substantially because the heartbeat side-channel and the workspace-mount plumbing disappear, but the form-driven multi-agent provisioning is genie's identity. |
| `dev-repl.js` | **Leave-in-place** | Useful as a non-daemon test bench for the engine. |
| `test/` (FTS5, observer, reflector, tool-gate, dom-parser) | **Move with the code** | Unit tests follow whatever package the code lands in. |
| Heartbeat self-send protocol (`/heartbeat <tickID>` mail) | **Retire** in favor of typed tick messages | Once `interval-scheduler` ticks are first-class daemon messages with their own type (or a typed envelope), the parsing-strings-with-prefixes hack goes away. |

## 5. Rollout Sketch

**Phase 1: Extract the engine.**
Create `@endo/llm-engine` from `packages/genie/src/agent/` plus
`tool-gate.js`, `system/`, and `utils/tokens.js`.
The genie package re-exports the engine for its own consumers.
Lal and fae keep working unchanged.
Estimate: **S**, ~1–2 days, mostly file moves and `package.json`
boilerplate.

**Phase 2: Migrate lal and fae onto the engine.**
Replace `lal/providers/` with the engine's provider resolution.
Replace lal's and fae's hand-rolled `runAgenticLoop` with
`runAgentRound`.
Replace fae's `extract-tool-calls.js` with the engine's tool-call
parsing.
Keep lal's transcript-tree code and fae's conversation-tree code as
adapters that consume `ChatEvent`s.
Pin pi-ai/pi-agent-core at the engine package level.
Estimate: **M**, ~3–5 days; the bulk is verifying behavioral parity
across the four lal providers.
Blocked on Phase 1.

**Phase 3: Graduate `interval-scheduler` into the daemon.**
Per `designs/endoclaw-timer.md`: extend the existing `timer` formula
(or add a new `interval-scheduler` formula) with the resolve/reschedule
semantics, missed-tick coalescing, and host-controlled limits from the
genie prototype.
Add `provideIntervalScheduler` to `HostInterface`.
Switch genie's `runHeartbeatTicker` to use the daemon scheduler;
delete the per-agent `intervalsDir` persistence.
Blocked on the engine extraction only insofar as the heartbeat code
calls into the engine — independent in terms of the daemon work.
Estimate: **M-L**, ~1 week, mostly daemon plumbing and
formula-type integration tests.

**Phase 4: Move memory to `Mount` + `memory-index`.**
Replace `safePath`+`VFS` in `tools/memory.js` with calls to a granted
`Mount` capability.
Add a new `memory-index` formula type that wraps the FTS5 backend in a
daemon exo.
Update the genie plugin to provision a `ScratchMount` and a
`memory-index` per agent at spawn time.
Sub-agents (observer, reflector) receive narrowed memory capabilities
via `introducedNames`.
Blocked on **Phase 4 of `daemon-mount`** (sub-mounts and `snapshot()`)
because narrowing observer to "writes only `observations.md`" is most
naturally a sub-mount.
Estimate: **L**, ~1–1.5 weeks.

**Phase 5: Make scheduling capabilities granular.**
Once Phase 3 ships, change the genie plugin to grant each agent a
*scoped* `IntervalHandle` (a single named interval) rather than an
arbitrary `IntervalScheduler`.
Lal and fae start using the same primitives — first for memory
consolidation, then for whatever proactive behaviour their agent
authors want.
Estimate: **S-M**, ~2–3 days after Phase 3.

**Phase 6: Retire workspace_template, retire VFS, retire `.heartbeats.log`.**
Cleanup pass once Phases 4 and 5 have landed and the workspace
directory is no longer the source of truth for memory.
Estimate: **S**, ~1 day.

## 6. Open Questions and Contentious Calls

- **Vending pi-ai/pi-agent-core into a shared `@endo/llm-engine` is
  the load-bearing decision.**
  pi is a third-party MIT-licensed JS library actively maintained;
  putting all three harnesses on it concentrates supply-chain risk on
  one author.
  Alternatives are (a) vendor a frozen subset, (b) implement the
  same surface in-tree (a multi-month project), or (c) keep the status
  quo (hand-rolled providers in lal, pi in genie) and accept the
  duplication.
  The recommendation is (a) eventually, (b) never, and "extract to a
  shared package now" as the bridging step.
  **I am guessing about pi's stability** — a quick survey of recent
  pi-ai releases would inform this.

- **Should `interval-scheduler` replace the existing `timer` formula
  or live alongside it?**
  The existing timer is used by host code that hasn't been audited as
  part of this design.
  Conservative answer: add `interval-scheduler` as a new formula type;
  deprecate `timer` once nothing depends on it.
  More aggressive: extend `timer` itself.

- **Markdown-bag-in-mount vs. blob-per-snapshot for memory.**
  Phase 4 picks bag-in-mount for continuity with how agents already
  think about memory.
  But bag-in-mount means every memory edit replaces the whole file —
  bad for cross-peer GC, bad for per-snapshot retention, and bad for
  audit.
  Blob-per-snapshot is the daemon-native answer and it would be
  cleaner.
  **Flagging this as a contentious call**: starting with bag-in-mount
  is the pragmatic choice but it forecloses no future move to blobs.

- **The observer/reflector are thin sub-agents that read the main
  agent's `state.messages` directly.**
  This works because everything runs in one process today.
  If the engine package is to be truly portable (so a remote pi-agent
  service could be the LLM provider), the sub-agents need to consume
  a serialised excerpt rather than a live `messages` array reference.
  This is mostly a refactor, but it's worth flagging.

- **lal's "depth prefix" in `reply` and fae's "auto-reply if no tool
  call" fallback are agent-specific quirks.**
  Both can be implemented as middleware around `runAgentRound`, but
  they are not in genie today, which means migrating lal/fae onto the
  engine requires teaching the engine about per-tool middleware (or
  doing the post-processing in the harness loop).
  Either is fine; the choice between them is contentious.

- **Granting an agent a single `IntervalHandle` instead of a full
  scheduler is a real capability narrowing only if the agent cannot
  request more intervals from the daemon.**
  Today the daemon's `formulateTimer` is exposed via `HostInterface`,
  and a guest who reaches the host (e.g. via `@host`) can ask for one.
  The narrowing is meaningful only inside the agent's own pet-store —
  which is the right answer per the daemon's existing capability
  model, but worth being explicit about.

- **`web-fetch` and `web-search` overlap with `endoclaw-network-fetch`
  (M1).**
  If `endoclaw-network-fetch` ships first, the genie tools can be
  retired and replaced with `HttpClient`-backed implementations.
  If genie's tools are extracted first, they become the de facto
  network-egress tools for lal and fae and `endoclaw-network-fetch`
  has to subsume them.
  Recommend coordinating with whoever picks up `endoclaw-network-fetch`.

## Prompt

> You are surveying `packages/genie` in the Endo monorepo for
> opportunities to integrate its components more deeply into the
> daemon and the sibling agent harnesses `packages/lal` and
> `packages/fae`.
> The goal is a design document, not code.
> Top billing for the pi engine, the memory system, and scheduling.
> List every other genie subsystem with an integrate / share / leave /
> retire judgment.
> Don't write code; flag guesses; respect daemon idioms.
