// @ts-check
/// <reference types="ses"/>

/** @import { CodeModeGlobal } from './tool.js' */

import { gitCodeModeTypeDeclarations } from './git-types.js';

/**
 * The git exo's per-mode generated TypeScript declarations, keyed by code-mode
 * surface: ordinary read/write, history rewrite, and read-only inspection.
 * A consumer composing its own code-mode agent can read these directly to inject
 * git types into a hand-built global.
 */
export { gitCodeModeTypeDeclarations };

/**
 * Build the code-mode global descriptor for an `@endo/exo-git` Git capability.
 * The three-way split is a prompt-surface choice: `readOnly` selects the
 * inspection-only declaration, `historyRewrite` selects the elevated rewrite
 * declaration, and the default selects the ordinary read/write declaration.
 * Runtime authority remains enforced by the exo guard.
 *
 * @param {object} options
 * @param {string} options.name JS-identifier lexical binding name.
 * @param {string | string[]} [options.petName] Pet name to look the capability
 *   up by; defaults to `name`.
 * @param {boolean} [options.readOnly] Select the read-only prompt surface.
 * @param {boolean} [options.historyRewrite] Select the history-rewrite prompt
 *   surface.
 * @returns {CodeModeGlobal}
 */
export const makeGitGlobal = ({
  name,
  petName = name,
  readOnly = false,
  historyRewrite = false,
}) =>
  harden({
    name,
    petName,
    description: readOnly
      ? 'Read-only @endo/exo-git Git capability for repository inspection.'
      : historyRewrite
        ? 'History-rewrite @endo/exo-git Git capability for amend, reword, and rebase.'
        : 'Read/write @endo/exo-git Git capability for repository changes.',
    declaration: readOnly
      ? gitCodeModeTypeDeclarations.gitReadOnly
      : historyRewrite
        ? gitCodeModeTypeDeclarations.gitHistory
        : gitCodeModeTypeDeclarations.git,
  });
harden(makeGitGlobal);
