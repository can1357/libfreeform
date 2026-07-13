#!/usr/bin/env node
// Links the built npm package into tests/node_modules so every runtime
// (Node, Bun, Deno-with-node_modules, and the browser page served by
// serve.mjs) resolves `libfreeform` through real package.json `exports`.
// A plain symlink keeps the link live across rebuilds — no reinstall.

import { existsSync, mkdirSync, rmSync, symlinkSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const testsDir = dirname(fileURLToPath(import.meta.url));
const target = join(testsDir, '..', 'npm');
if (!existsSync(join(target, 'dist', 'index.js'))) {
  console.error('npm/dist is missing — run `node npm/build.mjs` first');
  process.exit(1);
}

const link = join(testsDir, 'node_modules', 'libfreeform');
mkdirSync(dirname(link), { recursive: true });
rmSync(link, { recursive: true, force: true });
symlinkSync(target, link, 'dir');
console.log(`linked ${link} -> ${target}`);
