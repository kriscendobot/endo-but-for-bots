// @ts-check
/// <reference types="ses"/>

import { codeModeCondition } from './code-mode.js';
import { toolCallsCondition } from './tool-calls.js';

export { codeModeCondition } from './code-mode.js';
export { toolCallsCondition } from './tool-calls.js';

export const defaultEvalConditions = harden([
  codeModeCondition,
  toolCallsCondition,
]);
harden(defaultEvalConditions);

export const evalConditionsByName = harden(
  Object.fromEntries(
    defaultEvalConditions.map(condition => [condition.name, condition]),
  ),
);
harden(evalConditionsByName);
