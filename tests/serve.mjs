#!/usr/bin/env node

// Static server for the browser smoke test. Serves tests/ (following the
// node_modules/libfreeform symlink into ../npm) plus the crate fixtures under
// /fixtures/, with correct MIME types so wasm streaming instantiation works.
//
//   node tests/serve.mjs [port]   # port 0 (default) picks an ephemeral port
//
// Prints `listening on http://127.0.0.1:<port>` once ready.

import { readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { dirname, extname, join, normalize, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const testsDir = dirname(fileURLToPath(import.meta.url));
const fixturesDir = join(testsDir, '..', 'fixtures');

const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.json': 'application/json',
  '.wasm': 'application/wasm',
};

/** Resolve a URL path inside `root`, rejecting traversal above it. */
function resolveWithin(root, urlPath) {
  const relative = normalize(urlPath).replace(/^[/\\]+/, '');
  if (relative.split(sep).includes('..')) return undefined;
  return join(root, relative);
}

const server = createServer(async (req, res) => {
  const { pathname } = new URL(req.url, 'http://localhost');
  const file =
    pathname === '/'
      ? join(testsDir, 'browser.html')
      : pathname.startsWith('/fixtures/')
        ? resolveWithin(fixturesDir, pathname.slice('/fixtures/'.length))
        : resolveWithin(testsDir, pathname);
  try {
    if (file === undefined) throw new Error('forbidden');
    const body = await readFile(file);
    res.writeHead(200, { 'content-type': MIME[extname(file)] ?? 'application/octet-stream' });
    res.end(body);
  } catch {
    res.writeHead(404);
    res.end('not found');
  }
});

const port = Number(process.argv[2] ?? 0);
server.listen(port, '127.0.0.1', () => {
  console.log(`listening on http://127.0.0.1:${server.address().port}`);
});
