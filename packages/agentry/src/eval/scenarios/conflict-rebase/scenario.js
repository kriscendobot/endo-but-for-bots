// @ts-check
/// <reference types="ses"/>

/** @import { GitConflictRebaseTarget, GitScenario } from '../../types.js' */

import { assertGitConflictRebaseOutcome } from './outcome.js';

export const conflictRebasePrompt = `\
Rebase the current feature branch onto integration.
When app.txt conflicts, keep the integration wording, then add the feature
sentence after it.
Preserve the feature note and the integration note.
Leave the branch rebased, with a clean working tree.`;
harden(conflictRebasePrompt);

/**
 * A git code-mode scenario for a rebase that must stop for a content conflict,
 * resolve it deliberately, then continue replaying the remaining clean commit.
 *
 * @param {GitConflictRebaseTarget} [expectedTarget]
 * @returns {GitScenario<GitConflictRebaseTarget | undefined>}
 */
export const makeConflictRebaseScenario = expectedTarget => {
  const expected =
    expectedTarget === undefined
      ? undefined
      : (() => {
          const {
            featureBranch,
            integrationBranch,
            integrationOid,
            replayedSummaries,
            originalFeatureOids,
            expectedPatches,
            featureTreeOid,
            appText,
            notes,
          } = expectedTarget;
          return harden({
            featureBranch,
            integrationBranch,
            integrationOid,
            replayedSummaries,
            originalFeatureOids,
            expectedPatches,
            featureTreeOid,
            appText,
            notes,
          });
        })();
  return harden({
    name: 'conflict-rebase',
    requirements: harden({ allowHistoryRewrite: true }),
    expected,
    prompt: conflictRebasePrompt,
    assertOutcome: args => {
      if (expected === undefined) {
        throw new Error('conflict-rebase scenario target is not provisioned');
      }
      return assertGitConflictRebaseOutcome({ ...args, expected });
    },
  });
};
harden(makeConflictRebaseScenario);
