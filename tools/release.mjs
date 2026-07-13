#!/usr/bin/env node
/**
 * Cut a synchronized crates.io/npm release.
 *
 * Usage:
 *   bun tools/release.mjs [--dry-run] <version>
 *   bun tools/release.mjs [--dry-run] --resume <version>
 *
 * A normal run refuses a dirty checkout, updates both manifests, refreshes
 * Cargo.lock, commits, creates an annotated v<version> tag, and atomically
 * pushes the branch and tag to origin. `--resume` only accepts the exact three
 * staged version files left by an interrupted normal run, then completes its
 * validation/commit/tag/push steps. Registry publication remains tag-triggered
 * GitHub Actions workflow.
 */

import { execFileSync, spawnSync } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const cargoManifest = join(root, 'Cargo.toml');
const npmManifest = join(root, 'npm', 'package.json');
const versionFiles = ['Cargo.toml', 'Cargo.lock', 'npm/package.json'];
const args = process.argv.slice(2);
let dryRun = false;
let resume = false;
let version;

for (const arg of args) {
  if (arg === '--dry-run') {
    if (dryRun) throw new Error('duplicate --dry-run option');
    dryRun = true;
  } else if (arg === '--resume') {
    if (resume) throw new Error('duplicate --resume option');
    resume = true;
  } else if (version === undefined) {
    version = arg;
  } else {
    throw new Error('usage: bun tools/release.mjs [--dry-run] [--resume] <version>');
  }
}

if (version === undefined) {
  throw new Error('usage: bun tools/release.mjs [--dry-run] [--resume] <version>');
}
if (
  !/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/.test(
    version,
  )
) {
  throw new Error(`invalid semver version: ${version}`);
}

function git(args, { capture = false } = {}) {
  const output = execFileSync('git', args, {
    cwd: root,
    encoding: 'utf8',
    stdio: capture ? 'pipe' : 'inherit',
  });
  return capture ? output.trim() : '';
}

function hasGitRef(ref) {
  return (
    spawnSync('git', ['rev-parse', '--verify', '--quiet', ref], {
      cwd: root,
      stdio: 'ignore',
    }).status === 0
  );
}

function lines(value) {
  return value === '' ? [] : value.split('\n');
}

function samePaths(actual, expected) {
  return actual.length === expected.length && actual.every(path => expected.includes(path));
}

function packageVersion(manifest) {
  const packageStart = manifest.indexOf('[package]');
  const packageEnd = manifest.indexOf('\n[', packageStart + 1);
  const packageSection = manifest.slice(packageStart, packageEnd === -1 ? undefined : packageEnd);
  const match = packageSection.match(/^version\s*=\s*"([^"]+)"$/m);
  if (match?.[1] === undefined) throw new Error('could not read Cargo package version');
  return match[1];
}

function withCargoVersion(manifest, version) {
  const packageStart = manifest.indexOf('[package]');
  const packageEnd = manifest.indexOf('\n[', packageStart + 1);
  const before = manifest.slice(0, packageStart);
  const packageSection = manifest.slice(packageStart, packageEnd === -1 ? undefined : packageEnd);
  const after = packageEnd === -1 ? '' : manifest.slice(packageEnd);
  const updated = packageSection.replace(/^version\s*=\s*"[^"]+"$/m, `version = "${version}"`);
  if (updated === packageSection) throw new Error('could not update Cargo package version');
  return `${before}${updated}${after}`;
}

function assertReleaseTree() {
  const status = git(['status', '--porcelain=v1'], { capture: true });
  if (!resume) {
    if (status !== '') throw new Error('refusing to release from a dirty checkout');
    return;
  }

  const staged = lines(git(['diff', '--cached', '--name-only'], { capture: true }));
  const unstaged = lines(git(['diff', '--name-only'], { capture: true }));
  const untracked = lines(git(['ls-files', '--others', '--exclude-standard'], { capture: true }));
  if (!samePaths(staged, versionFiles) || unstaged.length !== 0 || untracked.length !== 0) {
    throw new Error('resume requires only staged Cargo.toml, Cargo.lock, and npm/package.json');
  }
}

assertReleaseTree();
const branch = git(['branch', '--show-current'], { capture: true });
if (branch === '') throw new Error('refusing to release from a detached HEAD');
git(['remote', 'get-url', 'origin'], { capture: true });

const tag = `v${version}`;
if (hasGitRef(`refs/tags/${tag}`)) throw new Error(`tag already exists: ${tag}`);

const cargo = readFileSync(cargoManifest, 'utf8');
const npm = JSON.parse(readFileSync(npmManifest, 'utf8'));
const cargoVersion = packageVersion(cargo);
if (cargoVersion !== npm.version) {
  throw new Error(`manifest versions disagree: Cargo ${cargoVersion}, npm ${npm.version}`);
}
if (resume ? cargoVersion !== version : cargoVersion === version) {
  throw new Error(resume ? `resume requires version ${version}` : `already at version ${version}`);
}

console.log(`${dryRun ? 'would release' : 'releasing'} ${resume ? 'resumed ' : ''}${cargoVersion}${resume ? '' : ` -> ${version}`} on ${branch}`);
if (dryRun) {
  if (resume) {
    console.log(`would validate the staged version files, commit, tag ${tag}, and atomically push`);
  } else {
    console.log(`would update Cargo.toml, Cargo.lock, and npm/package.json`);
    console.log(`would commit "chore: bumped to version ${version}", tag ${tag}, and atomically push`);
  }
  process.exit(0);
}

if (resume) {
  execFileSync('cargo', ['check', '--locked'], { cwd: root, stdio: 'inherit' });
} else {
  writeFileSync(cargoManifest, withCargoVersion(cargo, version));
  npm.version = version;
  writeFileSync(npmManifest, `${JSON.stringify(npm, null, 2)}\n`);
  execFileSync('cargo', ['check'], { cwd: root, stdio: 'inherit' });
  git(['add', '--', ...versionFiles]);
}

git(['commit', '-m', `chore: bumped to version ${version}`]);
git(['tag', '-a', tag, '-m', tag]);
git(['push', '--atomic', 'origin', `HEAD:refs/heads/${branch}`, `refs/tags/${tag}`]);

console.log(`released ${tag}`);
