// @ts-check
/// <reference types="ses"/>

import test from '@endo/ses-ava/prepare-endo.js';

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

import { gitClone, makeNativeGitBackend } from '../src/index.js';

const execFileAsync = promisify(execFile);

/**
 * @param {import('ava').ExecutionContext} t
 * @returns {Promise<string>}
 */
const provisionConflictRepo = async t => {
  const root = await fs.promises.mkdtemp(
    path.join(os.tmpdir(), 'native-git-conflict-'),
  );
  t.teardown(() => fs.promises.rm(root, { recursive: true, force: true }));
  await execFileAsync('git', ['init', '-q', '-b', 'main'], { cwd: root });
  await execFileAsync('git', ['config', 'user.email', 't@t'], { cwd: root });
  await execFileAsync('git', ['config', 'user.name', 'T'], { cwd: root });
  await fs.promises.writeFile(path.join(root, 'conflict.txt'), 'base\n');
  await fs.promises.writeFile(path.join(root, 'clean.txt'), 'clean\n');
  await execFileAsync('git', ['add', '.'], { cwd: root });
  await execFileAsync('git', ['commit', '-qm', 'base'], { cwd: root });
  await execFileAsync('git', ['switch', '-c', 'side'], { cwd: root });
  await fs.promises.writeFile(path.join(root, 'conflict.txt'), 'side\n');
  await execFileAsync('git', ['add', 'conflict.txt'], { cwd: root });
  await execFileAsync('git', ['commit', '-qm', 'side edit'], { cwd: root });
  await execFileAsync('git', ['switch', 'main'], { cwd: root });
  await fs.promises.writeFile(path.join(root, 'conflict.txt'), 'main\n');
  await execFileAsync('git', ['add', 'conflict.txt'], { cwd: root });
  await execFileAsync('git', ['commit', '-qm', 'main edit'], { cwd: root });
  await execFileAsync('git', ['merge', 'side'], { cwd: root }).catch(
    () => undefined,
  );
  return root;
};

/**
 * @param {import('ava').ExecutionContext} t
 * @param {'UD' | 'DU'} statusCode
 * @returns {Promise<string>}
 */
const provisionModifyDeleteRepo = async (t, statusCode) => {
  const root = await fs.promises.mkdtemp(
    path.join(os.tmpdir(), 'native-git-modify-delete-'),
  );
  t.teardown(() => fs.promises.rm(root, { recursive: true, force: true }));
  await execFileAsync('git', ['init', '-q', '-b', 'main'], { cwd: root });
  await execFileAsync('git', ['config', 'user.email', 't@t'], { cwd: root });
  await execFileAsync('git', ['config', 'user.name', 'T'], { cwd: root });
  await fs.promises.writeFile(path.join(root, 'conflict.txt'), 'base\n');
  await execFileAsync('git', ['add', 'conflict.txt'], { cwd: root });
  await execFileAsync('git', ['commit', '-qm', 'base'], { cwd: root });
  await execFileAsync('git', ['switch', '-c', 'side'], { cwd: root });
  if (statusCode === 'UD') {
    await execFileAsync('git', ['rm', '-q', 'conflict.txt'], { cwd: root });
  } else {
    await fs.promises.writeFile(path.join(root, 'conflict.txt'), 'side\n');
    await execFileAsync('git', ['add', 'conflict.txt'], { cwd: root });
  }
  await execFileAsync('git', ['commit', '-qm', 'side change'], { cwd: root });
  await execFileAsync('git', ['switch', 'main'], { cwd: root });
  if (statusCode === 'UD') {
    await fs.promises.writeFile(path.join(root, 'conflict.txt'), 'main\n');
    await execFileAsync('git', ['add', 'conflict.txt'], { cwd: root });
  } else {
    await execFileAsync('git', ['rm', '-q', 'conflict.txt'], { cwd: root });
  }
  await execFileAsync('git', ['commit', '-qm', 'main change'], { cwd: root });
  await execFileAsync('git', ['merge', 'side'], { cwd: root }).catch(
    () => undefined,
  );
  return root;
};

test('gitClone rejects unsafe clone boundaries before transport', async t => {
  const nonEmptyDestination = await fs.promises.mkdtemp(
    path.join(os.tmpdir(), 'git-clone-nonempty-'),
  );
  t.teardown(() =>
    fs.promises.rm(nonEmptyDestination, { recursive: true, force: true }),
  );
  await fs.promises.writeFile(path.join(nonEmptyDestination, 'occupied'), '');

  await t.throwsAsync(
    gitClone({
      url: 'http://github.com/example/repo.git',
      destPath: '/tmp/unused-clone',
    }),
    { message: /HTTP remotes are not supported/ },
  );
  await t.throwsAsync(
    gitClone({
      url: 'https://token@github.com/example/repo.git',
      destPath: '/tmp/unused-clone',
    }),
    { message: /must not include embedded credentials/ },
  );
  await t.throwsAsync(
    gitClone({
      url: 'file:///tmp/repo.git',
      destPath: '/tmp/unused-clone',
      allowLocalFileTransport: true,
      credential: { kind: 'bearer', material: { token: 'test-token' } },
    }),
    { message: /credentials require https remotes/ },
  );
  await t.throwsAsync(
    gitClone({
      url: 'file:///tmp/repo.git',
      destPath: '/tmp/unused-clone',
    }),
    { message: /file transport requires allowLocalFileTransport/ },
  );
  await t.throwsAsync(
    gitClone({
      url: 'file:///tmp/repo.git',
      destPath: nonEmptyDestination,
      allowLocalFileTransport: true,
    }),
    { message: /destination mount must be empty/ },
  );
});

test('checkoutConflict rejects a non-conflicted path before mutation', async t => {
  const root = await provisionConflictRepo(t);
  const backend = makeNativeGitBackend({ repoRoot: root });

  await t.throwsAsync(() => backend.checkoutConflict(['clean.txt'], 'ours'), {
    message: /path .*clean\.txt.*not an unmerged conflict/,
  });
  t.is(
    (await backend.status()).find(row => row.path === 'conflict.txt')?.index,
    'conflicted',
  );
});

test('checkoutConflict preflights mixed batches before mutation', async t => {
  const root = await provisionConflictRepo(t);
  const backend = makeNativeGitBackend({ repoRoot: root });

  await t.throwsAsync(
    () => backend.checkoutConflict(['conflict.txt', 'clean.txt'], 'ours'),
    { message: /path .*clean\.txt.*not an unmerged conflict/ },
  );
  const status = await backend.status();
  t.is(status.find(row => row.path === 'conflict.txt')?.index, 'conflicted');
  t.false(status.some(row => row.path === 'clean.txt'));
});

test('checkoutConflict rejects duplicate paths before mutation', async t => {
  const root = await provisionConflictRepo(t);
  const backend = makeNativeGitBackend({ repoRoot: root });

  await t.throwsAsync(
    () => backend.checkoutConflict(['conflict.txt', 'conflict.txt'], 'ours'),
    { message: /duplicate path.*conflict\.txt/ },
  );
  t.is(
    (await backend.status()).find(row => row.path === 'conflict.txt')?.index,
    'conflicted',
  );
});

test('checkoutConflict validates both modify/delete index stages', async t => {
  const cases =
    /** @type {Array<['UD' | 'DU', 'ours' | 'theirs', 'ours' | 'theirs']>} */ ([
      ['UD', 'ours', 'theirs'],
      ['DU', 'theirs', 'ours'],
    ]);
  for (const [statusCode, validSide, invalidSide] of cases) {
    // Each fixture is intentionally provisioned and checked serially so its
    // AVA teardown and git process state remain isolated.
    // eslint-disable-next-line no-await-in-loop
    const root = await provisionModifyDeleteRepo(
      t,
      /** @type {'UD' | 'DU'} */ (statusCode),
    );
    const backend = makeNativeGitBackend({ repoRoot: root });

    // eslint-disable-next-line no-await-in-loop
    await t.throwsAsync(
      () => backend.checkoutConflict(['conflict.txt'], invalidSide),
      {
        message: new RegExp(
          `path .*conflict\\.txt.*no ${invalidSide} index stage`,
        ),
      },
    );
    // eslint-disable-next-line no-await-in-loop
    await t.notThrowsAsync(() =>
      backend.checkoutConflict(['conflict.txt'], validSide),
    );
    // eslint-disable-next-line no-await-in-loop
    const status = await backend.status();
    t.false(
      status.some(
        row => row.path === 'conflict.txt' && row.index === 'conflicted',
      ),
    );
  }
});
