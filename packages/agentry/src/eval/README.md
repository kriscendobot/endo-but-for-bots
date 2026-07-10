# Git eval condition matrix

A harness for testing git agents across execution conditions: drive an agent
against a git repository and score whether the repository reached a target
end-state.

## Eval vs. optimize

These are two different concerns, and this module is the first one only.

- **Eval** (this module) measures a fixed agent: run a scenario, then make a
  pass/fail judgment by **outcome assertion**. Pass means the repository reached
  the target end-state (a commit exists, the tree and index match, the file
  contents are right). It answers "did it work?" for one model and one prompt.
- **Optimize** (out of scope here, deferred) searches for a better prompt:
  GEPA / `ax`-style prompt-tuning loops that mutate the system prompt and select
  on a score. It answers "what prompt works best?".

Optimization consumes an eval as its objective, but the eval stands alone and
ships first. This module does no prompt-tuning; it only runs an agent and
asserts the outcome.

## Why outcome assertion, not trace scoring

A code-mode agent's only tool is `execute`: it runs `E(git).x()` and
`E(workspace).x()` **inside** a Compartment, so the outer pi-agent tool-call
trace sees a single opaque `execute` call, not the individual git operations.
There is therefore no git-op trace to score with edit distance, and there
should not need to be: a correct run might stage in a different order, read
status an extra time, or use a different method to the same end.
Outcome assertion reads the repository's actual final state through the live
`git` capability and checks it against the target, so it needs no
in-compartment instrumentation and accepts any alternate-but-correct path.

Capturing the run's events is still useful for debugging _why_ a scenario
failed, but it is a diagnostic, never the gate.

## Conditions

The matrix compares two ways of giving the same repository authority to an
agent:

- `code-mode` — the existing code-mode loop over the live `workspace` and `git`
  capabilities.
- `tool-calls` — direct pi-agent tool calls over the same `workspace` and `git`
  capabilities, using the mounted filesystem and mount-bridged git tool
  catalog.

## Metrics

Every `runGitScenario` result includes `metrics` alongside `outcome`.
The metrics record summed provider token usage, including reasoning tokens when
reported, total provider cost, completed turns, assistant messages, tool
executions, tool execution errors, and wall time for the agent run.
They come from the same pi-agent-core event stream that powers diagnostics, so
they report the real provider usage carried by assistant messages instead of
estimating from transcript text.

Metrics are recorded for comparison and reporting only.
The scenario's outcome assertion remains the only pass/fail gate.

## Layout

The harness splits along the seam between a **shared harness** (the runner, the
env model, the shared types, the export surface, the README, and the generic
outcome primitives) and **per-eval content** (one scenario's prompt, its outcome
assertion, and the repository it runs against).
The shared harness is scenario-agnostic and changes rarely; per-eval content
grows with each new eval, so each eval gets its own folder.

Shared harness (this directory's root):

- `index.js` — the `@endo/agentry/eval` export surface: re-exports the shared
  harness and each eval's public symbols (the per-folder barrels).
- `run.js` — `runGitScenarioUnder(condition, { model, workspace, git, scenario, readText, ... })`
  runs one condition and scores by outcome assertion while returning diagnostic
  run metrics.
  `runGitScenario(...)` remains the code-mode default for existing
  callers.
- `conditions/` — one module per execution condition plus the default condition
  registry.
- `matrix.js` — `runEvalMatrix({ scenarios, conditions, models, repeats, readText })`
  runs `scenarios x conditions x models x repeats`, returns per-run rows plus
  aggregates, and renders the comparison markdown table.
- `repo.js` — Node-side eval repository helpers: temp git repo bootstrap,
  `workspace` + `git` power construction, and the UTF-8 `readText` scorer
  dependency.
- `metrics.js` — `makeRunMetricsRecorder()`: subscribes to plain pi-agent-core
  events and snapshots per-run usage, turn, tool execution, and wall-time
  metrics.
- `env-model.js` — `resolveEvalModelsFromEnv(env)`: builds a live model list
  plus `getApiKey` hooks from `ENDO_EVAL_MODELS` / `ENDO_LLM_MODELS` /
  `ENDO_LLM_MODEL` / `LAL_MODEL`.
  `resolveEvalModelFromEnv(env)` remains as the
  first-model compatibility shim.
- `types.ts` — `GitScenario`, `GitScenarioSpec`, `ReadText`: the contract every
  scenario implements.
- `outcome-kit.js` — the shared outcome primitives: `check()`, the
  `OutcomeReport` shape, and the small shared readers (`readTrackedFileAt`
  reads a tracked file at a ref through `filesystemAt`; `branchLog` resolves a
  branch's commit list).
  Per-eval scorers build on these so each stays short.
  Cap-based and portable; the byte reader is injected.

Per-eval content (one folder under `scenarios/`):

- `scenarios/index.js` — the landed scenario registry used by the matrix CLI.
- `scenarios/stage-and-commit/` — the minimal-success eval: `scenario.js`
  (`makeStageAndCommitScenario(...)`, stage an untracked file and commit it with
  a given message), `provision.js` (fresh repository setup), `outcome.js`
  (`assertGitCommitOutcome(...)`, the scorer that reads HEAD's commit message,
  the file tracked at HEAD and its content, and the working-tree status), and
  `index.js` (the barrel re-exporting the folder's public helpers).

A scenario's no-LLM assertion-path test and its per-eval repository fixture live
together under `test/eval/` (see "Running" below), mirroring this source layout.

## Running

- **No credentials (anywhere):** `test/eval/stage-and-commit.test.js` runs the
  full harness with a scripted faux provider standing in for the model. This is
  the assertion-path test; it needs no network and no secrets, and each eval's
  test co-locates with its per-eval repository fixture
  (`_stage-and-commit-repo.js`) under `test/eval/`.
  It runs under the default
  `yarn test`.
- **Live model (credentialed host):** `test/eval-live.test.js` runs the same
  scenarios and scorers against a real provider across the default condition
  matrix.
  It is **not** part of the default `yarn test`: it runs only via its own
  `test:live` command, under a dedicated ava config (`ava-live.config.js`), so
  that a host that happens to have the credentials in its environment does not
  reach a real provider as a side effect of a plain `yarn test` at the package or
  workspace root.
  The live test additionally skips every row unless the credentials are present.
  To run it, set `ENDO_LLM_HOST` / `ENDO_LLM_MODEL` /
  `ENDO_LLM_AUTH_TOKEN` (or their `LAL_*` aliases) in the environment to point
  at an OpenAI-compatible endpoint, then:

  ```sh
  yarn workspace @endo/agentry test:live
  ```

  Supply the token through the environment only; it never appears in code,
  config, or a committed file.

- **Matrix CLI (credentialed host):** run every landed scenario across the
  default conditions, one or more live models, and any repeat count:

  ```sh
  yarn workspace @endo/agentry eval:matrix --models anthropic/claude-opus-4-5-20251101,openai/gpt-4o-mini --repeats 3 --out ./agentry-eval-matrix.json
  ```

  The command writes a JSON artifact with a provenance header (`recordedAt`,
  providers, and model list), per-run rows (`scenario`, `condition`, `model`,
  `repeat`, `pass`, `metrics`, `outcome`), and aggregates.
  It also prints a markdown comparison table with pass rate, mean and median
  tokens, turns, and wall time.

  Model configuration can come from the CLI or environment:

  - `--models` or `ENDO_EVAL_MODELS` / `ENDO_LLM_MODELS` accepts a
    comma-separated list of pi-ai model specs such as
    `anthropic/claude-opus-4-5-20251101,openai/gpt-4o-mini`.
    Each model reads
    its provider key from `<PROVIDER>_API_KEY`.
  - With `ENDO_LLM_HOST` / `LAL_HOST` set, each model spec is treated as an
    OpenAI-compatible endpoint model id and uses `ENDO_LLM_AUTH_TOKEN` /
    `LAL_AUTH_TOKEN`.
  - `--conditions code-mode,tool-calls` can narrow or reorder the default
    condition list.
