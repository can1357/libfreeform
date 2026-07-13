// Object-style API over the raw wasm decoders. Cheap classification stays here:
// passing a whole file to wasm merely to inspect a name or a few prefix bytes is
// both unnecessary and observably copies the input in some wasm-bindgen builds.

import * as wasm from './libfreeform.js';

const DRAWING_MAGIC = [0x77, 0x72, 0x64];
const PNG_MAGIC = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
const PDF_MAGIC = [0x25, 0x50, 0x44, 0x46];

function hasPrefix(bytes, prefix) {
  return (
    bytes != null &&
    bytes.length >= prefix.length &&
    prefix.every((byte, index) => bytes[index] === byte)
  );
}

function normalizedName(name) {
  return typeof name === 'string' ? name.toLowerCase().replaceAll(/[^a-z0-9]/g, '') : '';
}

function isTiff(bytes) {
  return hasPrefix(bytes, [0x49, 0x49, 0x2a, 0x00]) || hasPrefix(bytes, [0x4d, 0x4d, 0x00, 0x2a]);
}

const BYTE_FIELDS = new Set([
  'bytes',
  'rawArchive',
  'rawData',
  'rawDescriptor',
  'rawIndex',
  'rawManifest',
]);

function decodedBytes(value) {
  return value instanceof Uint8Array ? value : Uint8Array.from(value);
}

function normalizeDecoded(value, field) {
  if (Array.isArray(value)) {
    if (BYTE_FIELDS.has(field)) return decodedBytes(value);
    return value.map(entry => normalizeDecoded(entry));
  }
  if (value == null || typeof value !== 'object' || value instanceof Uint8Array) return value;
  if (value.kind === 'data' && Array.isArray(value.value)) {
    value.value = decodedBytes(value.value);
  }
  for (const [key, entry] of Object.entries(value)) {
    if (key === 'state' && entry != null && typeof entry === 'object') {
      for (const [stateKey, bytes] of Object.entries(entry)) entry[stateKey] = decodedBytes(bytes);
    } else {
      value[key] = normalizeDecoded(entry, key);
    }
  }
  return value;
}

/** True when bytes start with PKDrawing's `wrd` signature. */
export function isPkDrawing(bytes) {
  return hasPrefix(bytes, DRAWING_MAGIC);
}

/** Identify a blob from its exact captured type/name or a small signature. */
export function classifyBlob(name, bytes) {
  const normalized = normalizedName(name);
  if (normalized.includes('crlnativedata')) return 'crlNative';
  if (normalized.includes('crlnativemetadata')) return 'nativeMetadata';
  if (normalized.includes('tsudescription')) return 'tsuDescription';
  if (normalized.includes('stylepasteboard')) return 'style';
  if (normalized.startsWith('comapplefreeformpasteboardstate')) return 'state';
  if (normalized.startsWith('comapplefreeformcrlasset')) return 'asset';
  if (normalized === 'publicutf8plaintext') return 'plainText';
  if (normalized === 'publicrtf') return 'richText';
  if (
    (normalized === 'publicpng' || normalized === 'applepngpasteboardtype') &&
    hasPrefix(bytes, PNG_MAGIC)
  )
    return 'renderPng';
  if ((normalized === 'publictiff' || normalized === 'nexttiffv40pasteboardtype') && isTiff(bytes))
    return 'renderTiff';
  if (
    (normalized === 'publicpdf' ||
      normalized === 'comadobepdf' ||
      normalized === 'applepdfpasteboardtype') &&
    hasPrefix(bytes, PDF_MAGIC)
  )
    return 'renderPdf';
  if (normalized === 'comappledrawing' || isPkDrawing(bytes)) return 'drawing';
  return undefined;
}

function requiredBytes(bytes, operation) {
  if (!(bytes instanceof Uint8Array)) throw new TypeError(`${operation} requires Uint8Array bytes`);
  return bytes;
}

/** Decode a PKDrawing blob, throwing the decoder's structured error on failure. */
export function decodePkDrawing(bytes) {
  return normalizeDecoded(wasm.decodePkDrawing(requiredBytes(bytes, 'decodePkDrawing')));
}

/** Decode CRLNativeData, optionally joined with a TSUDescription blob. */
export function decodeCrlNative(bytes, tsuDescription) {
  return normalizeDecoded(
    wasm.decodeCrlNative(
      requiredBytes(bytes, 'decodeCrlNative'),
      tsuDescription == null ? undefined : requiredBytes(tsuDescription, 'decodeCrlNative'),
    ),
  );
}

/** Parse a TSUDescription manifest with recursive typed hints. */
export function parseTsuDescription(bytes) {
  return normalizeDecoded(wasm.parseTsuDescription(requiredBytes(bytes, 'parseTsuDescription')));
}

/** Decode an exact ordered pasteboard snapshot with independent tier outcomes. */
export function decodePasteboard(blobs) {
  if (blobs == null || !Array.isArray(blobs.flavors)) {
    throw new TypeError('decodePasteboard requires { flavors: FreeformFlavor[] }');
  }
  return normalizeDecoded(wasm.decodePasteboard(blobs));
}

/** True when a snapshot contains a flavor with importable content. */
export function hasFreeformContent(blobs) {
  if (blobs == null || !Array.isArray(blobs.flavors)) return false;
  return blobs.flavors.some(flavor => {
    if (flavor == null || typeof flavor.uti !== 'string') return false;
    const kind = classifyBlob(flavor.uti, flavor.bytes);
    if (kind === 'drawing') return isPkDrawing(flavor.bytes);
    return [
      'asset',
      'crlNative',
      'plainText',
      'renderPdf',
      'renderPng',
      'renderTiff',
      'richText',
      'style',
      'tsuDescription',
    ].includes(kind);
  });
}
