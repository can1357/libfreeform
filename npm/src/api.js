// Object-style API over the raw wasm exports. Shared by both entry points
// (`index.js` for browsers, `index.node.js` for Node-flavored runtimes); the
// entries initialize the wasm module before re-exporting from here.

import * as wasm from './libfreeform.js';

export const classifyBlob = wasm.classifyBlob;
export const isPkDrawing = wasm.isPkDrawing;
export const decodePkDrawing = wasm.decodePkDrawing;
export const decodeCrlNative = wasm.decodeCrlNative;
export const parseTsuDescription = wasm.parseTsuDescription;

/**
 * Decode whatever flavors are present into a single selection. Decode
 * failures (unsupported version, truncated Universal Clipboard transfer)
 * degrade to a missing tier rather than throwing. `renderPng` passes through
 * untouched — it never crosses the wasm boundary.
 */
export function decodePasteboard(blobs) {
  const decoded = wasm.decodePasteboard(blobs.drawing, blobs.crlNative, blobs.tsuDescription);
  if (blobs.renderPng !== undefined) decoded.renderPng = blobs.renderPng;
  return decoded;
}

/** True when at least one flavor present in `blobs` could carry Freeform content. */
export function hasFreeformContent(blobs) {
  return (
    blobs.crlNative !== undefined ||
    (blobs.drawing !== undefined && wasm.isPkDrawing(blobs.drawing))
  );
}
