// Default entry: browsers, Deno via URL imports, edge runtimes, and bundlers
// (Vite, webpack 5, and Rollup all resolve the `new URL(..., import.meta.url)`
// asset pattern). Fetches and instantiates the wasm at module load.

import init from './libfreeform.js';

await init({ module_or_path: new URL('./libfreeform_bg.wasm', import.meta.url) });

export {
  classifyBlob,
  decodeCrlNative,
  decodePasteboard,
  decodePkDrawing,
  hasFreeformContent,
  isPkDrawing,
  parseTsuDescription,
} from './api.js';
