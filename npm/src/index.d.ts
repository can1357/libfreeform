// Public types for the `libfreeform` npm package. These mirror the Rust
// types in `src/types.rs` under the crate's `serde` feature (camelCase field
// names); keep the two in sync.

/** A sampled ink point in Freeform canvas space (stroke transform applied). */
export interface FreeformInkPoint {
  readonly x: number;
  readonly y: number;
  /** Per-point pressure, 0..1. */
  readonly force: number;
  /** Per-point stroke width in points, as PencilKit stored it. */
  readonly width: number;
}

/**
 * One freehand stroke from either PKDrawing or native item storage.
 * `inkType` is the PencilKit family (`pen`/`pencil`/`marker`/other).
 */
export interface FreeformInkStroke {
  readonly inkType: string;
  /** sRGB `#rrggbb`. */
  readonly color: string;
  /** Ink alpha, 0..1. */
  readonly opacity: number;
  readonly points: readonly FreeformInkPoint[];
}

/** Everything recovered from the `com.apple.drawing` PKDrawing blob. */
export interface FreeformDrawing {
  readonly strokes: readonly FreeformInkStroke[];
}

/** Canvas-space rectangle in Freeform points (1pt ~ 1 CSS px). */
export interface FreeformFrame {
  readonly x: number;
  readonly y: number;
  readonly w: number;
  readonly h: number;
  /** Rotation in radians. */
  readonly rotation: number;
}

/** One entry of the native object graph's ordered board-item list. */
export interface FreeformBoardItem {
  /** Position in the paste's board-item order. */
  readonly index: number;
  /** Item UUID from the index plist (`XXXXXXXX-XXXX-…`). */
  readonly uuid?: string;
  /** Local outline vertices for native polygonal shapes, when stored plainly. */
  readonly outline?: readonly [number, number][];
  /** Class from `TSUDescription`, `Freeform.` prefix stripped (`?` when unjoined). */
  readonly className: string;
  /** Cheap routing hints from `TSUDescription` (`textbox`, ...): key -> raw value. */
  readonly hints: Readonly<Record<string, string>>;
  /** Canvas-space frame decoded from the native GeometryArchive. */
  readonly frame?: FreeformFrame;
  /** Primary sRGB fill `#rrggbb` for this item. */
  readonly fill?: string;
  /** User text (sticky/label/cell content). */
  readonly text?: string;
  /** Native freehand strokes in item-local space (when `com.apple.drawing` is absent). */
  readonly nativeStrokes?: readonly FreeformInkStroke[];
}

/** Everything recovered from `com.apple.freeform.CRLNativeData` (+ `TSUDescription`). */
export interface FreeformNative {
  /** Paste UUID from the index plist. */
  readonly pasteId?: string;
  readonly items: readonly FreeformBoardItem[];
  /** Heuristically recovered TSWP text runs, byte order. */
  readonly texts: readonly string[];
  /** Heuristically recovered sRGB fills, byte order. */
  readonly colors: readonly string[];
}

/** A decoded Freeform selection assembled from one or more flavors. */
export interface FreeformPasteboard {
  readonly drawing?: FreeformDrawing;
  readonly native?: FreeformNative;
  /** Full-board `public.png` render, passed through from the input blobs. */
  readonly renderPng?: Uint8Array;
}

/** One `TSUDescription` board-item entry: class plus cheap routing hints. */
export interface TsuEntry {
  readonly className: string;
  readonly hints: Readonly<Record<string, string>>;
}

/** Which Freeform pasteboard flavor a blob is. */
export type FreeformBlobKind = 'drawing' | 'crlNative' | 'tsuDescription' | 'renderPng';

/** Raw pasteboard flavors captured from a Freeform copy (e.g. via the dump tool). */
export interface FreeformBlobs {
  /** `com.apple.drawing` — flattened freehand ink (PKDrawing). */
  readonly drawing?: Uint8Array;
  /** `com.apple.freeform.CRLNativeData` — the native object graph. */
  readonly crlNative?: Uint8Array;
  /** `com.apple.freeform.TSUDescription` — the board-item class manifest. */
  readonly tsuDescription?: Uint8Array;
  /** `public.png` — full-board render fallback. */
  readonly renderPng?: Uint8Array;
}

/**
 * Identify which Freeform pasteboard flavor a dropped/loaded file is, from its
 * dump-tool filename and content signature. Returns undefined for unrelated files.
 */
export function classifyBlob(name: string, bytes: Uint8Array): FreeformBlobKind | undefined;

/** True when `bytes` starts with the PKDrawing `"wrd"` magic. */
export function isPkDrawing(bytes: Uint8Array): boolean;

/**
 * Decode a `com.apple.drawing` PKDrawing blob into normalized strokes.
 * Throws an `Error` when the header/version is unrecognized.
 */
export function decodePkDrawing(bytes: Uint8Array): FreeformDrawing;

/**
 * Decode `com.apple.freeform.CRLNativeData`, optionally joined with the
 * `TSUDescription` blob to attach a class + hints to each ordered board item.
 * Throws an `Error` on a structurally invalid blob.
 */
export function decodeCrlNative(bytes: Uint8Array, tsuDescription?: Uint8Array): FreeformNative;

/**
 * Parse the `TSUDescription` manifest into per-item classes + routing hints.
 * Throws an `Error` on an unparseable bplist.
 */
export function parseTsuDescription(bytes: Uint8Array): TsuEntry[];

/**
 * Decode whatever flavors are present into a single selection. Decode
 * failures (unsupported version, truncated Universal Clipboard transfer)
 * degrade to a missing tier rather than throwing.
 */
export function decodePasteboard(blobs: FreeformBlobs): FreeformPasteboard;

/** True when at least one flavor present in `blobs` could carry Freeform content. */
export function hasFreeformContent(blobs: FreeformBlobs): boolean;
