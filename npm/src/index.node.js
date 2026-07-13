// Node-flavored entry: Node.js, Bun, and Deno (which resolve the "node"
// export condition and implement node:fs). Fully synchronous — no top-level
// await — so modern Node can even require() it from CommonJS.

import { readFileSync } from 'node:fs';
import { initSync } from './libfreeform.js';

initSync({ module: readFileSync(new URL('./libfreeform_bg.wasm', import.meta.url)) });

export {
  classifyBlob,
  decodeCrlNative,
  decodePasteboard,
  decodePkDrawing,
  hasFreeformContent,
  isPkDrawing,
  parseTsuDescription,
} from './api.js';
