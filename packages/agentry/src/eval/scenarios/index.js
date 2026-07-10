// @ts-check
/// <reference types="ses"/>

/** @import { GitScenarioSpec } from '../types.js' */

import {
  makeStageAndCommitScenario,
  provisionStageAndCommitRepo,
} from './stage-and-commit/index.js';

/**
 * @returns {GitScenarioSpec}
 */
export const makeStageAndCommitScenarioSpec = () =>
  harden({
    name: 'stage-and-commit',
    makeScenario: () => makeStageAndCommitScenario(),
    provisionRepo: ({ scenario }) =>
      provisionStageAndCommitRepo({
        path: scenario.expected.path,
        content: scenario.expected.content,
      }),
  });
harden(makeStageAndCommitScenarioSpec);

/**
 * Landed eval scenarios.
 * Each matrix run provisions a fresh repo from the spec
 * so conditions and repeats do not share state.
 *
 * @returns {GitScenarioSpec[]}
 */
export const makeDefaultGitScenarioSpecs = () =>
  harden([makeStageAndCommitScenarioSpec()]);
harden(makeDefaultGitScenarioSpecs);
