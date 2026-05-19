# Familiar Preliminary Release

| | |
|---|---|
| **Created** | 2026-05-12 |
| **Updated** | 2026-05-19 (review pass: deferrals, scope tightening, gateway alignment, open-question resolutions) |
| **Author** | Kris Kowal (prompted) |
| **Status** | Proposed |
| **Source** | Issue [#229](https://github.com/endojs/endo-but-for-bots/issues/229) |

## What is the Problem Being Solved?

The maintainer has asked, in
[issue #229](https://github.com/endojs/endo-but-for-bots/issues/229):

> Please propose a plan for a preliminary release of the Familiar,
> identifying gaps between the project status and a minimum viable
> release.
> The minimum viable release must at least have the `lal` agent
> integrated.
> It must be stand-alone and not rely on any developer tooling.
> Material like the `lal` "Primer" need to be bundled by whatever
> means.
> It may fall to the `lal` code to carry the Primer and inject it
> into the CAS on initialization instead of relying on the setup
> script.

The unmistakable goal is a downloadable Familiar that a
non-developer can install on their own machine, launch, point at an
LLM provider, and use to converse with the bundled `lal` agent.
A new user must not need a checkout of `endojs/endo`, a Yarn
install, a `corepack` activation, an `electron-forge` invocation,
or any other developer tool on the host that runs the application.

This document audits the present state of `packages/familiar/`,
catalogues the gaps, and proposes a phased plan with a clear
minimum-viable-release (MVR) scope.

## Status quo

The Familiar's build pipeline is `node scripts/build.mjs`, which
runs six steps:

1. `yarn workspace @endo/chat build` (vite renderer build).
2. `scripts/bundle.mjs` (esbuild CJS bundles for the daemon, CLI,
   worker, lal setup, lal agent, plus the Electron main).
3. `scripts/download-node.mjs` (download a Node binary from
   `nodejs.org/dist/<version>/`).
4. `scripts/prepare-package.mjs` (copy the right Node binary and
   chat dist into the package).
5. `scripts/package-app.mjs` (`@electron/packager` produces a
   `.app` / `.exe` / linux directory under `out/Familiar-<plat>-<arch>/`).
6. `scripts/make-distributables.mjs` (DMG on macOS, zip on every
   platform; opt out via `--app-only`).

The relevant supporting designs already landed:

- [`familiar-electron-shell`](familiar-electron-shell.md):
  daemon-manager, window, menu, IPC, `localhttp://`, navigation
  guard, exfiltration defenses.
- [`familiar-daemon-bundling`](familiar-daemon-bundling.md):
  esbuild CJS bundles for daemon, worker, CLI, plus the embedded
  Node binary.
- [`familiar-bundled-agents`](familiar-bundled-agents.md): `lal`
  setup bundle and agent bundle inside `bundles/`.
  The daemon's `ENDO_EXTRA` mechanism (in
  [`packages/daemon/src/daemon-node.js`](../packages/daemon/src/daemon-node.js))
  imports the bundled `endo-lal-setup.cjs` after the host is
  ready and runs its `main(host)`.
- [`lal-fae-form-provisioning`](lal-fae-form-provisioning.md):
  the agent sends a configuration form to the host inbox; the
  user supplies host, model, and auth token through Chat.

The agent already self-bundles its own Primer.
`packages/lal/agent.js` (line 1641 onward) does:

```js
const primerDirPath = new URL('./primer', import.meta.url).pathname;
const localPrimerTree = makeLocalTree(primerDirPath);
await E(agent).storeTree(localPrimerTree, 'lal-primer');
const primerTreeId = await E(agent).identify('lal-primer');
```

`scripts/bundle.mjs` already copies `packages/lal/primer/` to
`packages/familiar/bundles/primer/` so the bundled `agent.js`'s
`new URL('./primer', import.meta.url)` resolves under the packaged
`bundles/` directory.
The mechanism the issue body describes ("lal carries the Primer
and injects it into the CAS on initialization") is therefore
already in place.
What remains is to verify it end to end in a fresh-install
configuration and to surface the rest of the gaps below.

The Familiar's `package.json` lists `electron` as both a runtime
dependency and a devDependency (Electron Forge needs it in
devDependencies for version detection; the runtime needs it as a
dependency).
The `dependencies` block today is:

```json
"dependencies": {
  "@endo/where": "workspace:^",
  "electron": "^40.8.0"
}
```

`@electron/packager` walks the dependency tree and only copies
files that pass `package-app.mjs`'s allowlist filter
(`/preload.js`, `/package.json`, `/bundles`, `/dist`, `/node`,
`/node.exe`).
That allowlist already excludes the entire `node_modules` tree
from the packaged app; the bundles are self-contained.

### What works today (assumed)

- The Electron app launches.
- The bundled daemon spawns under the embedded Node binary.
- The `lal` setup script provisions the manager guest.
- The agent sends a config form to the host inbox.
- The user fills in the form in Chat.
- The agent stores the Primer as a `readable-tree` in the daemon's
  CAS.
- Each spawned worker loop receives a `primer` reference.

### What does not work today (assumed gaps)

This audit identifies the discrepancies between the implemented
build pipeline and a downloadable preliminary release.
Each gap is itemized in the next section with severity, current
behaviour, target behaviour, and rough effort.

## Gaps

The gaps fall into four categories: build prerequisites, runtime
prerequisites, distribution and trust, and first-run experience.

### G1. Bundles directory not committed; build is mandatory

**Severity:** Blocker.
**Current:** `packages/familiar/bundles/` and
`packages/familiar/binaries/` are absent from a fresh checkout
(they are build outputs).
The build pipeline in `scripts/build.mjs` calls
`yarn workspace @endo/chat build` and the bundle/download/prepare
scripts; producing a release artifact requires Yarn, the chat
package's full toolchain (vite + plugins), and the esbuild
binary.
**Target:** A release engineer runs `yarn workspace @endo/familiar
build:package` on a single CI host per target platform; the
output `out/make/Familiar-<version>-<plat>-<arch>.zip` (and the
DMG on macOS) is the artifact users download.
The user installs the artifact and never needs Yarn.
**Effort:** Day, if CI is willing to run the existing pipeline.
The pipeline already exists; the missing step is automation.

### G2. macOS code signing and notarization

**Severity:** Deferred for MVR (resolved 2026-05-19).
**Current:** `package-app.mjs` calls
`@electron/packager` without `osxSign` or `osxNotarize` options.
A user who downloads the resulting `.dmg` from a browser is
greeted with Gatekeeper's "this app is damaged" or "cannot verify
the developer" dialog and must `xattr -d com.apple.quarantine`
the bundle by hand.
A non-developer will not do this and will assume the app is
broken.
**Target:** The build eventually runs `osxSign` with a Developer ID
Application certificate and `osxNotarize` against an Apple ID
configured in the build environment; the DMG carries a notarized
ticket that Gatekeeper accepts on a user's machine without
prompts.
**MVR resolution:** Skip the notarization integration for MVR.
The early user pool is small enough to accept the manual
`xattr -d com.apple.quarantine` workaround documented in the
README.
The certificate-acquisition process is tracked in a separate
issue (see G3 for the parallel ask on Windows; the macOS issue
covers the Developer ID Application certificate and the
App Store Connect API key administratively).
**Effort:** Multi-day to multi-week when undertaken, dominated by the
administrative cost of obtaining a Developer ID and an App
Store Connect API key, plus debugging the entitlements file
that notarization will demand.
The Electron docs describe the mechanism; the work is
configuration, not code.

### G3. Windows code signing

**Severity:** Out of scope for MVR (resolved 2026-05-19).
**Current:** No Windows signing.
A user double-clicking `Familiar-<version>-win32-x64.zip` and the
extracted `Familiar.exe` triggers SmartScreen's "unrecognised
publisher" dialog.
**Target:** Sign the exe with an EV (or OV) certificate; the EV
certificate yields immediate SmartScreen reputation, the OV
certificate accumulates reputation over downloads.
**MVR resolution:** MVR targets macOS only; Windows signing is out
of scope.
The certificate-acquisition process is tracked in a separate
issue (see Axis-2 followups) that records the steps for
beginning the EV / OV certificate process so that a future
maintainer can pick it up.
**Effort:** Multi-week when undertaken, dominated by certificate
acquisition (an EV cert ships on a hardware token); the in-tree
script change to add `signtool` invocation under
`make-distributables.mjs` is a day.

### G4. Linux distribution shape

**Severity:** Important (Linux).
**Current:** `make-distributables.mjs` emits a `.zip`.
A Linux user who unzips it gets a directory of files including
the `Familiar` ELF binary, `chrome-sandbox` (which must be
`chmod 4755 chrome-sandbox` and chowned to root for Chromium's
suid sandbox to work, otherwise Electron falls back to
`--no-sandbox` or refuses to launch), and a tree of Chromium
runtime files.
**Target:** Ship at least one of `.AppImage`, `.deb`, `.rpm`,
`.tar.gz`, or a Flatpak manifest, with documentation for the
chrome-sandbox setup.
The MVR position can defer downstream packaging and
ship the existing `.zip` plus a brief README; the followups
phase ships AppImage at minimum.
**Effort:** Day for the README; week for AppImage; multi-week
for `.deb`/`.rpm` if we want to host a repository.

### G5. Bundled Node binary version pin policy

**Severity:** Important.
**Current:** `scripts/download-node.mjs` defaults to
`v20.18.1` (a string literal in the script).
A vulnerability disclosure against Node 20.x or a Node EOL
event has no documented response cadence.
**Target:** A documented policy in the package README that
matches the Endo project's
[`skills/verify-upstream-state.md`](../skills/verify-upstream-state.md)
posture for external deps.
The release engineer pins to the latest LTS in each release
cycle and ships a security release if a CVE affecting the
embedded Node lands.
**Effort:** Day (write the policy and the release-cycle
checklist).

### G6. Auto-update channel

**Severity:** Out of scope (resolved 2026-05-19).
**Current:** None.
A user who installs Familiar 0.1.0 will still be running 0.1.0
when 0.2.0 ships unless they re-download.
**Target:** `electron-updater` against an S3 bucket (or GitHub
Releases) with a public update manifest, signature-verified
against the same code-signing certificate as G2/G3.
**MVR resolution:** Defer auto-update entirely (see Open
Question 6).
Users re-download when a new release is announced; the GitHub
Releases distribution channel (see Open Question 1) is the
publication venue.
**Effort:** Multi-day when revisited; signature verification
depends on G2 and G3 being in place first.

### G7. Application icon and metadata for `assets/icon`

**Severity:** Important.
**Current:** `scripts/package-app.mjs` references
`assets/icon` and (for DMG) `assets/icon.icns`.
The `assets/` directory is present in the repo; whether the
icon assets are correctly sized and exported per platform is
release-blocking but not part of code review.
**Target:** Verify (and, if absent, generate via
`scripts/generate-icons.sh`) the `.icns`, `.ico`, and `.png`
sets and confirm the macOS Info.plist `CFBundleIconFile`
resolution.
The `package.json` has no `productName` or
`CFBundleDisplayName`; the packager defaults to "Familiar"
which is acceptable.
**Effort:** Day.

### G8. The dev-mode `endo` CLI bundle is in the production runtime path

**Severity:** Important.
**Current:**
[`src/daemon-manager.js`](../packages/familiar/src/daemon-manager.js)
calls `runEndoCommand(['stop'])` and `['purge']` from menu
actions for "Restart Daemon" and "Purge Daemon".
These spawn the bundled `endo-cli.cjs` as a subprocess.
That bundle is shipped (it is in the `package-app.mjs`
allowlist), so this works in the packaged build, but it
pulls in roughly 20% of the daemon's transitive deps a
second time inside `endo-cli.cjs`.
**Target:** The MVR can ship the CLI bundle as-is.
A followup folds stop/purge into a direct CapTP message from
the Electron main, removing the need to bundle the CLI in the
production app.
**Effort:** Day for the followup; zero for MVR.

### G9. ENDO_ADDR and gateway port collision

**Severity:** Important.
**Current:** The daemon binds the gateway on port `8920` by
default
([`src/daemon-manager.js`](../packages/familiar/src/daemon-manager.js)
line 296).
A user who already runs an Endo daemon (as a developer might)
will see the Familiar's daemon collide with theirs on the
Unix socket (the Familiar checks for an existing socket and
joins it).
A user who has an unrelated process bound to TCP 8920 will see
the daemon fail to start.
**Target:** For MVR, document the collision case in the
README; the Familiar already detects the existing-daemon
case and joins the running daemon.
For followups, change the gateway default to a per-user
deterministic high port (or honour an `ENDO_ADDR` value of
`127.0.0.1:0` and persist the OS-assigned port).
**Effort:** Day for the README; day for the followup.

### G10. State directory shape on a fresh install

**Severity:** Important.
**Current:** The Familiar uses `@endo/where` to resolve
`whereEndoState`, which on Linux is `~/.local/state/endo/`,
on macOS `~/Library/Application Support/endo/`, on Windows
`%LOCALAPPDATA%\endo\State\`.
A user who installs Familiar, uses it, then uninstalls, will
leave behind their state directory; the Purge menu item
deletes the daemon-managed contents but the directory itself
persists.
**Target:** Acceptable for MVR.
A first-run dialog could explain where state lives so the user
can delete it after uninstall; defer to followups.
**Effort:** Day for the dialog.

### G11. LLM credential entry UX

**Severity:** Important.
**Current:** The user supplies their LLM provider host, model
name, and auth token through a form sent to their inbox by the
agent.
The form's `authToken` field is marked `secret: true` and the
Chat UI honours the secret marker by masking the input.
On submission, the value is stored in the daemon's CAS.
**Target:** The MVR can ship this flow as-is; it is functional
and user-tested by the maintainer.
A followup adds an in-band "test connection" button on the
form so the user gets a positive confirmation before the
agent's first inference call.
**Effort:** Zero for MVR; day for the followup test button.

### G12. Outbound network policy

**Severity:** Important.
**Current:** The bundled `lal` agent fetches against
`https://api.anthropic.com/`, `https://api.openai.com/`,
`http://localhost:11434/v1/` (Ollama default), or whatever
host the user typed into the form.
The agent has unconfined `fetch` access (it is an unconfined
guest by construction).
**Target:** Acceptable for MVR; the Familiar sandbox is the
user's own machine and the agent is trusted code shipped by
us.
A followup constrains outbound HTTP to the user-configured
LLM host plus a documented allowlist; this is the
[`endoclaw-network-fetch`](endoclaw-network-fetch.md) work
already on the M1 milestone.
**Effort:** Zero for MVR; tracked under the existing design.

### G13. Telemetry, crash reporting, and error logs

**Severity:** Nice-to-have.
**Current:** `src/logger.js` writes to `familiar.log` in the
Endo state directory; the daemon writes to `endo.log` in the
same directory.
There is no upload mechanism, no opt-in, and no UI for
"submit logs".
**Target:** For MVR, document the log locations in the README
so a user can attach the file to a bug report.
A followup adds an opt-in Sentry-style uploader.
**Effort:** Day for the README; multi-week for the uploader.

### G14. Third-party license aggregation

**Severity:** Important.
**Current:** The packaged app contains Electron, the Node
binary, the bundled SDK code (`@anthropic-ai/sdk`, `openai`,
`ollama`), and many transitive dependencies through the
esbuild bundles.
None of their licenses or notices are surfaced in the
packaged app.
**Target:** Aggregate the LICENSE files of every package
included in the bundles via an `oss-attribution-generator`
or `license-checker` step in `make-distributables.mjs`, and
ship the result as `LICENSE.third-party.txt` next to the
binary.
**Effort:** Day.

### G15. macOS arm64 vs x64 build matrix

**Severity:** Important (macOS).
**Current:** `package-app.mjs` runs with `arch: process.arch`,
so the build host's architecture is what the build emits.
A user on Apple Silicon needs the `arm64` build; an Intel Mac
user needs `x64`.
**Target:** The build runs on both architectures (or uses
universal binaries via `@electron/universal`) and the
distribution surface offers both.
**Effort:** Day per CI host; multi-day for universal binaries.

### G16. Verify the Primer-into-CAS path in the packaged build

**Severity:** Blocker.
**Current:** `agent.js` calls `new URL('./primer',
import.meta.url)`.
In the bundled `bundles/agent.js`, `import.meta.url` resolves
to a `file://` URL inside the packaged app, and the bundle
script copies `packages/lal/primer/` to
`bundles/primer/`.
This *should* work, but it has not been documented as a
verified end-to-end test.
**Target:** A smoke test step in CI: build the app, launch it
under a clean state directory, exercise the form, submit
config, observe the Primer tree appearing in the host
namespace and the worker loop receiving a `primer`
reference.
**Effort:** Day for the test scaffold.

## Primer-into-CAS migration

The issue body offers a permission ("It may fall to the lal code
to carry the Primer and inject it into the CAS on initialization
instead of relying on the setup script").
The migration is **already implemented** in `agent.js`; this
section records the shape so future readers do not relitigate it.

### Current shape

The setup script
([`packages/lal/setup.js`](../packages/lal/setup.js)) provisions
the manager guest and launches the agent caplet.
It does **not** carry the Primer.

The agent caplet
([`packages/lal/agent.js`](../packages/lal/agent.js)) does carry
the Primer, after the form-loop initialises:

```js
const primerDirPath = new URL('./primer', import.meta.url).pathname;
const localPrimerTree = makeLocalTree(primerDirPath);
await E(agent).storeTree(localPrimerTree, 'lal-primer');
const primerTreeId = await E(agent).identify('lal-primer');
```

For each new sub-guest spawned in response to a form submission,
`provisionPrimer(guest)` does:

```js
if (!await E(guest).has('primer')) {
  await E(guest).storeIdentifier('primer', primerTreeId);
}
```

The `bundles/primer/` copy in
[`scripts/bundle.mjs`](../packages/familiar/scripts/bundle.mjs)
(line 99 onward) ensures `import.meta.url` resolves to a
sibling directory in the packaged build.

### What this design adds

A CI smoke test (G16) that exercises the path in the packaged
build, plus a brief mention in the package README that the
Primer ships with the agent and lands in the user's CAS at
first form submission.

The setup script need not change.
The agent need not change.
The bundle script need not change.

## Phased plan

### MVR: minimum to ship

The exit criterion is: a user on macOS arm64 (the maintainer's
primary platform) downloads a `.dmg`, double-clicks, drags
Familiar to Applications, launches it, fills in their LLM
provider details, and exchanges messages with `lal`.
No developer tooling is touched on the user's machine.

| Item | Resolves | Effort |
|---|---|---|
| Wire the existing build pipeline into a CI workflow that emits per-platform artifacts | G1 | day |
| macOS Developer ID + notarization | G2 | multi-week (admin) |
| Verify the Primer-into-CAS path in a packaged-build smoke test | G16 | day |
| Aggregate third-party LICENSE notices into the bundle | G14 | day |
| Document Node version pin policy in the package README | G5 | day |
| Document log locations and state directory in the package README | G10, G13 | day |
| Document the `127.0.0.1:8920` collision case in the README | G9 | day |
| Confirm icon assets resolve on every target platform | G7 | day |
| macOS arm64 build host (defer x64 until requested) | G15 (partial) | day |

The MVR is macOS arm64 only.
This is the maintainer's primary platform, the platform with the
most stringent distribution requirements (notarization), and the
one that exercises every interesting code path in the build
pipeline.
A successful macOS release is the proof that the bundling story
works.

### Followups (post-MVR, pre-Milestone-1-completion)

| Item | Resolves | Effort |
|---|---|---|
| Linux AppImage (or `.deb`) | G4 | week |
| Windows code signing (after EV cert acquisition) | G3 | multi-week (admin) |
| Auto-update channel | G6 | multi-day |
| First-run "test connection" on the LLM config form | G11 | day |
| Fold stop/purge into Electron main; drop the bundled CLI | G8 | day |
| OS-assigned gateway port to dodge collisions with developer daemons | G9 | day |
| First-run dialog explaining state-directory location for clean uninstall | G10 | day |
| Universal macOS binary via `@electron/universal` | G15 | multi-day |
| Opt-in crash reporter | G13 | multi-week |

### Out of scope for this release

- Any change to the `lal` agent's tool surface, capability set, or
  interaction model.
- Any change to the daemon's wire protocol.
- Outbound HTTP confinement
  ([`endoclaw-network-fetch`](endoclaw-network-fetch.md)) is
  tracked separately under Milestone 1.
- Self-hosting / Docker
  ([`daemon-docker-selfhost`](daemon-docker-selfhost.md)) is
  Milestone 1.
- The Endo Gateway split
  ([`endo-gateway`](endo-gateway.md)) is multi-milestone.
- Multi-agent provisioning (Familiar ships only `lal`; Fae,
  bundled in
  [`familiar-bundled-agents`](familiar-bundled-agents.md), is
  not in MVR scope).
- The Chat UI's pending command, edit-message, and slot-slash
  features tracked under Milestone 4.
- Auto-update (G6) is deferred entirely per the maintainer's
  resolution of Open Question 6.
- macOS code-signing and notarization (G2) are deferred for MVR
  per the maintainer's 2026-05-19 directive; an issue tracks the
  cert-acquisition admin work.
- Windows code-signing (G3) is out of scope for MVR; an issue
  tracks the EV / OV certificate-acquisition process.

## Open questions

These were the questions the original draft posed before MVR work
began.
The maintainer answered them in the 2026-05-19 review pass; the
answers are recorded inline below.

1. **Distribution channel.**
   *Resolution (2026-05-19):* Post artifacts as GitHub releases on
   `endojs/endo-but-for-bots`.
   This implies two follow-on processes that are out of scope for
   the release pipeline itself but on the roadmap for the surrounding
   project: a ferrying process to copy the release artifacts to the
   `endojs/endo` repository, and a process for proposing a PR on
   `endojs/endo` that updates a document for deployment on
   `docs.endojs.org`.
   Carry-over to `endojs.org` is out of band.

2. **Signing identity.**
   *Resolution (2026-05-19):* A separate issue records the
   instructions to set up the signing identity (see Axis-2
   followups).
   The macOS-side signing flow is itself deferred (see G2); the
   issue stages the certificate-acquisition work for whenever the
   project pursues notarization.

3. **Versioning policy.**
   `package.json` has `"version": "0.1.0"` and `"private": true`.
   For the first downloadable build, do we bump to `0.2.0`,
   `1.0.0-rc.1`, or stay at `0.1.0` and ship under "preview"?
   Does the `private` flag stay in place (it does not block the
   build) or do we publish a marker package to `npm` to claim
   the name?

4. **Operating-system coverage matrix for MVR.**
   Is macOS arm64 alone sufficient for the preliminary release,
   or does the maintainer want macOS x64 and Linux x64 in the
   same drop?
   Each additional target adds a CI host and an additional
   testing surface.

5. **Bundled daemon vs. published `@endo/daemon` package.**
   The Familiar today bundles the daemon at build time from
   workspace source.
   Should the released Familiar pin a published version of
   `@endo/daemon` (and the related `@endo/cli`,
   `@endo/lal`) so that release engineering can be done
   without a fresh checkout?
   The current shape (workspace bundling) is operationally
   simpler and is the recommendation for MVR; the question is
   whether the maintainer wants the npm-published shape on the
   roadmap.

6. **Auto-update opt-in posture.**
   *Resolution (2026-05-19):* Defer auto-update entirely.
   G6 is moved to the Out-of-scope section above; the opt-in vs.
   opt-out question is deferred until the project is ready to
   pursue auto-update at all.

## References

- Issue [#229](https://github.com/endojs/endo-but-for-bots/issues/229): source.
- [`familiar-electron-shell`](familiar-electron-shell.md): shell.
- [`familiar-daemon-bundling`](familiar-daemon-bundling.md): esbuild pipeline.
- [`familiar-bundled-agents`](familiar-bundled-agents.md): `lal` and Fae bundling.
- [`familiar-localhttp-protocol`](familiar-localhttp-protocol.md): weblet origins.
- [`familiar-unified-weblet-server`](familiar-unified-weblet-server.md): single-port shape.
- [`familiar-gateway-migration`](familiar-gateway-migration.md): gateway in the daemon.
- [`lal-fae-form-provisioning`](lal-fae-form-provisioning.md): form-based config.
- [`lal-reply-chain-transcripts`](lal-reply-chain-transcripts.md): agent transcripts.
- [`endoclaw-network-fetch`](endoclaw-network-fetch.md): outbound HTTP confinement (followup).
- [`gateway-bearer-token-auth`](gateway-bearer-token-auth.md): bearer-token auth (followup).
- [`endo-gateway`](endo-gateway.md): host-scoped Gateway (out of scope).
- [`daemon-docker-selfhost`](daemon-docker-selfhost.md): Docker (out of scope).
