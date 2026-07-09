// @ts-check
/// <reference types="ses"/>

/** @import { ERef } from '@endo/eventual-send' */
/** @import { ShellToolCapability } from '@endo/agent-tools' */
/** @import { EvalCondition } from '../types.js' */

import { makeShellTool } from '@endo/agent-tools';
import { toPiAgentTool } from '@endo/agent-tools/pi';

import { makePiAgent } from '../../harness/pi-agent.js';

export const makeShellSystemPrompt =
  () => `You are an Endo eval shell-capability coding agent. Use the exec tool to inspect, edit, stage, and commit the repository.

The exec tool runs an allowlisted command in the scenario repository worktree with structured argv. Use git and simple file commands. Do not answer in prose when a tool call can complete the task.`;
harden(makeShellSystemPrompt);

/**
 * Shell condition: a pi-agent over the confined `Shell` capability minted for
 * the scenario repository.
 *
 * @type {EvalCondition}
 */
export const shellCondition = harden({
  name: 'shell',
  makeAgent: options => {
    const { model, shell, getApiKey, thinkingLevel, streamFn } = options;
    if (shell === undefined) {
      throw new Error('shell eval condition requires a shell capability');
    }
    const shellCap = /** @type {ERef<ShellToolCapability>} */ (shell);
    const tools = makeShellTool(shellCap).map(tool => toPiAgentTool(tool));
    return makePiAgent({
      model,
      tools,
      systemPrompt: makeShellSystemPrompt(),
      getApiKey,
      thinkingLevel,
      streamFn,
    });
  },
});
harden(shellCondition);
