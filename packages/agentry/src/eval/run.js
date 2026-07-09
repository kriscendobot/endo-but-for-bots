// @ts-check
/// <reference types="ses"/>

/** @import { RunGitScenarioOptions, RunGitScenarioResult } from './types.js' */

import { codeModeCondition } from './conditions/code-mode.js';
import { makeRunMetricsRecorder } from './metrics.js';

/**
 * Run one git scenario under a specific execution condition and score it by
 * outcome assertion.
 *
 * The condition supplies the pi-agent-core `Agent`; the run loop and metrics
 * recorder stay uniform across code mode, tool calls, and shell capability.
 *
 * @param {import('./types.js').EvalCondition} condition
 * @param {RunGitScenarioOptions} options
 * @returns {Promise<RunGitScenarioResult>}
 */
export const runGitScenarioUnder = async (
  condition,
  {
    model,
    workspace,
    git,
    shell,
    scenario,
    readText,
    getApiKey,
    thinkingLevel,
    streamFn,
  },
) => {
  const agent = condition.makeAgent({
    model,
    workspace,
    git,
    shell,
    getApiKey,
    thinkingLevel,
    streamFn,
  });

  // `agent` is a local pi-agent-core instance (not a remotable), so drive it
  // directly rather than through eventual-send, matching the code-mode tests.
  const metricsRecorder = makeRunMetricsRecorder();
  const unsubscribeMetrics = agent.subscribe(metricsRecorder.listener);
  await null; // safe-await-separator
  try {
    await agent.prompt(scenario.prompt);
    await agent.waitForIdle();
  } finally {
    unsubscribeMetrics();
  }

  const outcome = await scenario.assertOutcome({ git, workspace, readText });
  return harden({ outcome, metrics: metricsRecorder.snapshot() });
};
harden(runGitScenarioUnder);

/**
 * Run one git code-mode scenario end to end and score it by outcome assertion.
 *
 * The agent is the real code-mode git-loop preset: its sole tool is `execute`,
 * which evaluates JavaScript against the live `workspace` and `git` powers in a
 * Compartment. Only the model varies between a no-LLM run (a scripted faux
 * model) and a live run (a credentialed provider): the agent, the powers, and
 * the scorer are identical, so the no-LLM path exercises the same machinery the
 * live path does.
 *
 * @param {RunGitScenarioOptions} options
 * @returns {Promise<RunGitScenarioResult>}
 */
export const runGitScenario = options =>
  runGitScenarioUnder(codeModeCondition, options);
harden(runGitScenario);
