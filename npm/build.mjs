#!/usr/bin/env node
// Builds the npm package into npm/dist:
//   cargo build (wasm32-unknown-unknown, release)
//     -> wasm-bindgen --target web (ESM shim + .wasm + internal .d.ts)
//     -> wasm-opt -O3 (optional; warns and ships unoptimized when absent)
//     -> copy the hand-written entries/types from npm/src
//     -> copy README + licenses from the repo root for `npm publish`.
//
// The wasm-bindgen CLI version must match the crate pin in the root
// Cargo.toml; the script fails loudly when it drifts.

import { execFileSync } from 'node:child_process';
import { copyFileSync, mkdirSync, readFileSync, renameSync, rmSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const npmDir = dirname(fileURLToPath(import.meta.url));
const rootDir = dirname(npmDir);
const distDir = join(npmDir, 'dist');
const wasmArtifact = join(
  rootDir,
  'target',
  'wasm32-unknown-unknown',
  'release',
  'libfreeform.wasm',
);

const run = (cmd, args, opts = {}) =>
  execFileSync(cmd, args, { stdio: 'inherit', cwd: rootDir, ...opts });

// Fail fast on a wasm-bindgen crate/CLI version mismatch: the CLI refuses
// mismatched schemas with a confusing error, so surface the pin instead.
const wasmCargoToml = readFileSync(join(rootDir, 'Cargo.toml'), 'utf8');
const pinned = wasmCargoToml.match(/wasm-bindgen\s*=\s*\{\s*version\s*=\s*"=([\d.]+)"/)?.[1];
const cliVersion = execFileSync('wasm-bindgen', ['--version'], { encoding: 'utf8' })
  .trim()
  .split(' ')[1];
if (pinned === undefined || cliVersion !== pinned) {
  console.error(
    `wasm-bindgen CLI ${cliVersion} does not match the crate pin ${pinned}.\n` +
      `Install the matching CLI: cargo install wasm-bindgen-cli --version ${pinned} --locked`,
  );
  process.exit(1);
}

run('cargo', ['build', '--release', '--target', 'wasm32-unknown-unknown', '--features', 'wasm']);

rmSync(distDir, { recursive: true, force: true });
mkdirSync(distDir, { recursive: true });
run('wasm-bindgen', [
  wasmArtifact,
  '--out-dir',
  distDir,
  '--out-name',
  'libfreeform',
  '--target',
  'web',
  '--omit-default-module-path',
]);

const wasmOut = join(distDir, 'libfreeform_bg.wasm');
try {
  const tmp = `${wasmOut}.opt`;
  run('wasm-opt', [
    '-O3',
    '--enable-bulk-memory',
    '--enable-nontrapping-float-to-int',
    wasmOut,
    '-o',
    tmp,
  ]);
  renameSync(tmp, wasmOut);
} catch {
  console.warn('wasm-opt unavailable or failed; shipping unoptimized wasm');
}

for (const entry of ['api.js', 'index.js', 'index.node.js', 'index.d.ts']) {
  copyFileSync(join(npmDir, 'src', entry), join(distDir, entry));
}
for (const doc of ['README.md', 'LICENSE-MIT', 'LICENSE-APACHE']) {
  copyFileSync(join(rootDir, doc), join(npmDir, doc));
}
console.log(`built npm package -> ${distDir}`);
