#!/usr/bin/env node
// Cross-runtime smoke test. The same file runs under Node, Bun, and Deno:
//   node tests/smoke.mjs
//   bun tests/smoke.mjs
//   deno run -R tests/smoke.mjs
// Run tests/setup.mjs once beforehand to link the built package.

import { readFileSync } from 'node:fs';
import * as api from 'libfreeform';
import { runChecks } from './checks.mjs';

const fixture = name =>
  new Uint8Array(readFileSync(new URL(`../fixtures/${name}`, import.meta.url)));

const count = runChecks(api, {
  inkPen: fixture('ink-pen.drawing'),
  nativeMixed: fixture('native-mixed.crlnative'),
  tsuDescription: fixture('native-mixed.tsudescription'),
  realBoardNative: fixture('real-board.crlnative'),
  realBoardTsu: fixture('real-board.tsudescription'),
});

const runtime =
  globalThis.Deno?.version?.deno !== undefined
    ? `deno ${globalThis.Deno.version.deno}`
    : globalThis.Bun?.version !== undefined
      ? `bun ${globalThis.Bun.version}`
      : `node ${globalThis.process.version}`;
console.log(`PASS ${count} checks (${runtime})`);
