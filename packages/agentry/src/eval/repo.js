// @ts-check
/// <reference types="ses"/>

import { execFile } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import process from 'node:process';
import { promisify as nodePromisify } from 'node:util';

import { E } from '@endo/eventual-send';
import { makeGit } from '@endo/exo-git';
import { makeShell } from '@endo/exo-shell';
import { iterateBytesReader } from '@endo/exo-stream/iterate-bytes-reader.js';
import { makeNativeGitBackend } from '@endo/git';
import { makeHostSpawner } from '@endo/host-spawner';
import { makeNodeFilesystem } from '@endo/platform/fs/extended';
import { makeFilePowers } from '@endo/daemon/src/daemon-node-powers.js';
import {
  getMountBacking,
  lineageOf,
  makeMount,
} from '@endo/daemon/src/mount.js';

const execFileAsync = nodePromisify(execFile);

export const DEFAULT_EVAL_SHELL_POLICY = harden({
  allowedCommands: ['cat', 'git', 'ls', 'mkdir', 'pwd', 'sed', 'printf'],
  timeoutMs: 30_000,
  maxOutputBytes: 64_000,
  env: { CI: 'true' },
});
harden(DEFAULT_EVAL_SHELL_POLICY);

/**
 * @param {unknown} policy
 * @returns {{
 *   allowedCommands: string[],
 *   timeoutMs: number,
 *   maxOutputBytes: number,
 *   env: Record<string, string>,
 *   searchPath: string,
 * }}
 */
const normalizeEvalShellPolicy = policy => {
  if (!policy || typeof policy !== 'object') {
    throw new Error('eval shell policy must be an object');
  }
  const { allowedCommands, timeoutMs, maxOutputBytes, env, searchPath } =
    /** @type {Record<string, unknown>} */ (policy);
  if (
    !Array.isArray(allowedCommands) ||
    allowedCommands.length === 0 ||
    !allowedCommands.every(c => typeof c === 'string' && c.length > 0)
  ) {
    throw new Error(
      'eval shell policy.allowedCommands must be a non-empty array of command-name strings',
    );
  }
  if (!Number.isInteger(timeoutMs) || /** @type {number} */ (timeoutMs) <= 0) {
    throw new Error('eval shell policy.timeoutMs must be a positive integer');
  }
  if (
    !Number.isInteger(maxOutputBytes) ||
    /** @type {number} */ (maxOutputBytes) <= 0
  ) {
    throw new Error(
      'eval shell policy.maxOutputBytes must be a positive integer',
    );
  }
  const timeoutMsValue = /** @type {number} */ (timeoutMs);
  const maxOutputBytesValue = /** @type {number} */ (maxOutputBytes);
  /** @type {Record<string, string>} */
  const normalizedEnv = {};
  if (env !== undefined) {
    if (typeof env !== 'object' || env === null || Array.isArray(env)) {
      throw new Error(
        'eval shell policy.env must be a record of string values',
      );
    }
    for (const [key, value] of Object.entries(env)) {
      if (typeof value !== 'string') {
        throw new Error(
          `eval shell policy.env[${JSON.stringify(key)}] must be a string`,
        );
      }
      normalizedEnv[key] = value;
    }
  }
  const normalizedSearchPath =
    searchPath === undefined ? process.env.PATH || '' : searchPath;
  if (typeof normalizedSearchPath !== 'string') {
    throw new Error('eval shell policy.searchPath must be a string');
  }
  return harden({
    allowedCommands: harden([...allowedCommands]),
    timeoutMs: timeoutMsValue,
    maxOutputBytes: maxOutputBytesValue,
    env: harden(normalizedEnv),
    searchPath: normalizedSearchPath,
  });
};

/**
 * @param {unknown} readerRef
 * @returns {Promise<Uint8Array>}
 */
const collectBytes = async readerRef => {
  /** @type {Uint8Array[]} */
  const chunks = [];
  let total = 0;
  for await (const chunk of iterateBytesReader(
    /** @type {any} */ (readerRef),
  )) {
    chunks.push(chunk);
    total += chunk.length;
  }
  const bytes = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.length;
  }
  return bytes;
};

/**
 * Read an `@endo/platform/fs` File capability's content as UTF-8.
 *
 * @param {unknown} file
 * @returns {Promise<string>}
 */
export const readText = async file => {
  const fileRef = /** @type {any} */ (file);
  const stat = await E(fileRef).getStat();
  const openFile = await E(fileRef).open({ read: true });
  try {
    const reader = await E(openFile).read(0n, stat.size ?? 0n);
    return new TextDecoder().decode(await collectBytes(reader));
  } finally {
    await E(openFile).close();
  }
};
harden(readText);

/**
 * Mint an eval shell capability over a daemon mount using the same composition
 * as the daemon shell formula maker.
 *
 * @param {object} mount
 * @param {unknown} [policy]
 * @returns {unknown}
 */
export const makeEvalShellOverMount = (
  mount,
  policy = DEFAULT_EVAL_SHELL_POLICY,
) => {
  const backing = getMountBacking(mount);
  if (!backing) {
    throw new Error('eval shell requires a daemon-minted mount');
  }
  if (backing.kind !== 'physical') {
    throw new Error('eval shell requires a physical mount');
  }
  const normalizedPolicy = normalizeEvalShellPolicy(policy);
  const searchPath = normalizedPolicy.searchPath || process.env.PATH || '';
  const baseEnv = harden({ PATH: searchPath, LC_ALL: 'C' });
  const spawner = makeHostSpawner({
    searchPath,
    defaultEnv: baseEnv,
    killProcessGroup: true,
  });
  return makeShell({
    cwd: backing.currentDir,
    policy: harden({
      allowedCommands: harden([...normalizedPolicy.allowedCommands]),
      timeoutMs: normalizedPolicy.timeoutMs,
      maxOutputBytes: normalizedPolicy.maxOutputBytes,
      env: harden({ ...normalizedPolicy.env }),
    }),
    spawner,
    readOnly: backing.readOnly,
  });
};
harden(makeEvalShellOverMount);

/**
 * Build the live `workspace`, `git`, and confined shell powers over an existing
 * git worktree.
 *
 * @param {string} repoRoot
 * @returns {{ workspace: unknown, git: unknown, shell: unknown }}
 */
export const makePowersOver = repoRoot => {
  const workspace = makeNodeFilesystem({ rootPath: repoRoot });
  const filePowers = makeFilePowers({ fs, path });
  const mount = makeMount({ rootPath: repoRoot, readOnly: false, filePowers });
  const backend = makeNativeGitBackend({ repoRoot });
  const git = makeGit({ mount, backend, lineageOf });
  const shell = makeEvalShellOverMount(mount);
  return harden({ workspace, git, shell });
};
harden(makePowersOver);

/**
 * Bootstrap an empty git repository for one eval run.
 *
 * @param {object} [options]
 * @param {string} [options.branch]
 * @param {string} [options.prefix]
 * @returns {Promise<{
 *   repoRoot: string,
 *   run: (args: string[]) => Promise<{ stdout: string, stderr: string }>,
 *   cleanup: () => Promise<void>,
 * }>}
 */
export const initRepo = async ({
  branch = 'main',
  prefix = 'agentry-eval-git-',
} = {}) => {
  const repoRoot = await fs.promises.mkdtemp(path.join(os.tmpdir(), prefix));
  const run = args => execFileAsync('git', args, { cwd: repoRoot });
  await run(['init', '-q', '-b', branch]);
  await run(['config', '--local', 'commit.gpgsign', 'false']);
  await run(['config', '--local', 'tag.gpgsign', 'false']);
  await run(['config', '--local', 'user.email', 'eval@example.invalid']);
  await run(['config', '--local', 'user.name', 'Eval']);
  const cleanup = () =>
    fs.promises.rm(repoRoot, { recursive: true, force: true });
  return harden({ repoRoot, run, cleanup });
};
harden(initRepo);
