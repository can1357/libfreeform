#!/usr/bin/env node
// CI driver for the browser smoke test: serves tests/ on an ephemeral port,
// loads browser.html in headless Chromium via Playwright, and asserts the
// page-side runChecks result. Requires `npm install` +
// `npx playwright install chromium` in tests/ first.

import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright';

const testsDir = dirname(fileURLToPath(import.meta.url));

const server = spawn(process.execPath, [join(testsDir, 'serve.mjs'), '0'], {
  stdio: ['ignore', 'pipe', 'inherit'],
});
const [banner] = await once(server.stdout, 'data');
const url = String(banner).match(/listening on (\S+)/)?.[1];
if (url === undefined) {
  server.kill();
  throw new Error(`server did not report a URL: ${banner}`);
}

let result;
const browser = await chromium.launch();
try {
  const page = await browser.newPage();
  page.on('console', message => console.log(`[page] ${message.text()}`));
  await page.goto(url);
  await page.waitForFunction(() => window.__result !== undefined, undefined, { timeout: 30_000 });
  result = await page.evaluate(() => window.__result);
} finally {
  await browser.close();
  server.kill();
}

console.log(result);
if (!String(result).startsWith('PASS')) process.exit(1);
