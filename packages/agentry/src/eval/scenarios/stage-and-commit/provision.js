// @ts-check
/// <reference types="ses"/>

import fs from 'node:fs';
import path from 'node:path';

import { initRepo, makePowersOver } from '../../repo.js';

/**
 * Provision a real git repository for the stage-and-commit scenario.
 *
 * @param {object} options
 * @param {string} options.path Repository-relative path of the untracked file.
 * @param {string} options.content Its content.
 * @returns {Promise<{
 *   repoRoot: string,
 *   workspace: unknown,
 *   git: unknown,
 *   shell: unknown,
 *   cleanup: () => Promise<void>,
 * }>}
 */
export const provisionStageAndCommitRepo = async ({
  path: filePath,
  content,
}) => {
  const { repoRoot, run, cleanup } = await initRepo({ branch: 'main' });
  await fs.promises.writeFile(path.join(repoRoot, '.keep'), '');
  await run(['add', '.keep']);
  await run(['commit', '-q', '-m', 'chore: initialize repository']);
  await fs.promises.writeFile(path.join(repoRoot, filePath), content);

  const { workspace, git, shell } = makePowersOver(repoRoot);
  return harden({ repoRoot, workspace, git, shell, cleanup });
};
harden(provisionStageAndCommitRepo);
