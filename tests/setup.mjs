#!/usr/bin/env node
// (Node, Bun, and Deno with node_modules resolution) resolves `libfreeform`
// through real package.json `exports`. A plain symlink keeps the link live
// across rebuilds — no install step.

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
