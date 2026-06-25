/* global process */
import 'ses';
import fs from 'fs';
import { makeBundle } from '@endo/compartment-mapper/bundle.js';
import { fileURLToPath, pathToFileURL } from 'url';

const resolve = (rel, abs) => fileURLToPath(new URL(rel, abs).toString());
const root = new URL('..', import.meta.url).toString();

const read = async location => fs.promises.readFile(fileURLToPath(location));
const write = async (target, content) => {
  const location = resolve(target, root);
  await fs.promises.writeFile(location, content);
};

// The node prelude header is the hand-written ES5/CommonJS glue that adapts the
// eshost VM context (a bare vm.createContext with only setTimeout, require,
// console, and print) for the bundled pass-style modules.  It is kept in a
// standalone source file — scripts/node-prelude-header.js — so that it is
// subject to eslint, prettier, and the rest of the repo's validation rather
// than living as an inert template literal here.  That file's own header
// comment documents the cross-realm TextEncoder/TextDecoder problem it solves.
const nodePreludeHeader = await fs.promises.readFile(
  fileURLToPath(new URL('./node-prelude-header.js', import.meta.url)),
  'utf8',
);

const main = async () => {
  const nodePreludeBundle = await makeBundle(
    read,
    pathToFileURL(
      resolve('../src/node-prelude.js', import.meta.url),
    ).toString(),
  );
  const nodePrelude = nodePreludeHeader + nodePreludeBundle;
  const xsPrelude = await makeBundle(
    read,
    pathToFileURL(resolve('../src/xs-prelude.js', import.meta.url)).toString(),
  );

  await fs.promises.mkdir('prelude', { recursive: true });
  await write('prelude/node.js', nodePrelude);
  await write('prelude/xs.js', xsPrelude);
};

main().catch(err => {
  console.error('Error running main:', err);
  process.exitCode = 1;
});
