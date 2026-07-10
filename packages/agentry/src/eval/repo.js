// @ts-check
/// <reference types="ses"/>

import { execFile } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { promisify as nodePromisify } from 'node:util';

import { E } from '@endo/eventual-send';
import { makeGit } from '@endo/exo-git';

/** @import { EndoGit } from '@endo/exo-git' */
import { iterateBytesReader } from '@endo/exo-stream/iterate-bytes-reader.js';
import { makeNativeGitBackend } from '@endo/git';
import { makeNodeFilesystem } from '@endo/platform/fs/extended';
import { makeFilePowers } from '@endo/daemon/src/daemon-node-powers.js';
import { lineageOf, makeMount } from '@endo/daemon/src/mount.js';

const execFileAsync = nodePromisify(execFile);

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
 * Build the live `workspace` and `git` powers over an existing git worktree.
 *
 * @param {string} repoRoot
 * @returns {{ workspace: unknown, git: EndoGit }}
 */
export const makePowersOver = repoRoot => {
  const workspace = makeNodeFilesystem({ rootPath: repoRoot });
  const filePowers = makeFilePowers({ fs, path });
  const mount = makeMount({ rootPath: repoRoot, readOnly: false, filePowers });
  const backend = makeNativeGitBackend({ repoRoot });
  const git = makeGit({ mount, backend, lineageOf });
  return harden({ workspace, git });
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
