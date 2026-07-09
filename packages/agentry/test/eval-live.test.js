// @ts-check

// The live-model git code-mode eval: the SAME scenarios and SAME outcome scorers
// as the no-LLM tests, but driven by a real provider. It runs ONLY via its own
// `test:live` command and a dedicated ava config (`ava-live.config.js`); it is
// deliberately excluded from the default `yarn test` so that a host with
// `ENDO_LLM_*` / `LAL_*` credentials in its environment does not reach a real
// provider as a side effect of a plain `yarn test`. It is also gated on those
// same credentials: when they are absent every row skips rather than failing. To
// run it, set `ENDO_LLM_HOST` / `ENDO_LLM_MODEL` / `ENDO_LLM_AUTH_TOKEN` (or
// their `LAL_*` aliases) to point at an OpenAI-compatible endpoint, then:
//
//   yarn workspace @endo/agentry test:live
//
// Pass = the repository reached the target end-state (outcome assertion), not a
// transcript score. A failure here is an eval signal about the model, not a
// harness bug; the no-LLM tests are what prove the harness.
//
// The test is table-driven over the matrix registry, so the credential-gating
// logic lives in one place and adding an eval adds one scenario spec rather than
// one more gated file.

/* global globalThis */

import test from '@endo/ses-ava/prepare-endo.js';

import {
  defaultEvalConditions,
  makeDefaultGitScenarioSpecs,
  renderEvalMatrixMarkdownTable,
  runEvalMatrix,
  resolveEvalModelFromEnv,
} from '../src/eval/index.js';
import { readText } from './_eval-fixture.js';

const env =
  /** @type {{ process?: { env?: Record<string, string | undefined> } }} */ (
    globalThis
  ).process?.env || {};
const live = resolveEvalModelFromEnv(env);
const liveTest = live ? test : test.skip;

liveTest(
  'a live model runs every git eval scenario across every condition',
  async t => {
    // `live` is defined here (otherwise this test was skipped at registration).
    const { model, getApiKey } = /** @type {NonNullable<typeof live>} */ (live);
    const result = await runEvalMatrix({
      scenarios: makeDefaultGitScenarioSpecs(),
      conditions: defaultEvalConditions,
      models: [{ model, getApiKey }],
      repeats: 1,
      readText,
    });

    const failures = result.rows.filter(row => !row.pass);
    t.deepEqual(
      failures,
      [],
      `live matrix did not reach every target end-state:\n${renderEvalMatrixMarkdownTable(
        result.aggregates,
      )}\n${JSON.stringify(failures, null, 2)}`,
    );
  },
);
