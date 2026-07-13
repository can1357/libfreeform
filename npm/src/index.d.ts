/** Lossless JavaScript declarations mirroring the Rust serde camelCase model. */

/** A point in Freeform coordinates. */
export interface FreeformPoint {
  readonly x: number;
  readonly y: number;
}
/** A size in Freeform points. */
export interface FreeformSize {
  readonly width: number;
  readonly height: number;
}
/** A complete Core Graphics affine transform. */
export interface FreeformTransform {
  readonly a: number;
  readonly b: number;
  readonly c: number;
  readonly d: number;
  readonly tx: number;
  readonly ty: number;
}
/** A canvas-space rectangle. */
export interface FreeformFrame {
  readonly x: number;
  readonly y: number;
  readonly w: number;
  readonly h: number;
  readonly rotation: number;
}
/** Local bounds and transform for an item. */
export interface FreeformGeometry {
  readonly frame?: FreeformFrame;
  readonly transform?: FreeformTransform;
  readonly anchor?: FreeformPoint;
  readonly horizontalFlip?: boolean;
  readonly verticalFlip?: boolean;
  readonly widthValid?: boolean;
  readonly heightValid?: boolean;
}
/** A normalized sRGB color and its source color space. */
export interface FreeformColor {
  readonly colorSpace: string;
  readonly red: number;
  readonly green: number;
  readonly blue: number;
  readonly alpha: number;
  readonly hex: string;
}
/** One gradient stop. */
export interface FreeformGradientStop {
  readonly fraction: number;
  readonly inflection?: number;
  readonly color: FreeformColor;
}
/** A native fill or stroke paint. */
export type FreeformPaint =
  | { readonly kind: 'solid'; readonly color: FreeformColor }
  | {
      readonly kind: 'linearGradient';
      readonly start: FreeformPoint;
      readonly end: FreeformPoint;
      readonly stops: readonly FreeformGradientStop[];
    }
  | {
      readonly kind: 'radialGradient';
      readonly center: FreeformPoint;
      readonly radius: number;
      readonly stops: readonly FreeformGradientStop[];
    }
  | { readonly kind: 'image'; readonly assetId: string; readonly technique?: string }
  | { readonly kind: 'unknown'; readonly rawData: Uint8Array };
/** Stroke styling for shapes and connectors. */
export interface FreeformStrokeStyle {
  readonly paint: FreeformPaint;
  readonly width: number;
  readonly dash: readonly number[];
  readonly dashOffset?: number;
  readonly cap?: string;
  readonly join?: string;
  readonly miterLimit?: number;
  readonly tailEnd?: string;
  readonly headEnd?: string;
}
/** One native shadow effect. */
export interface FreeformShadow {
  readonly kind: string;
  readonly color: FreeformColor;
  readonly offsetX: number;
  readonly offsetY: number;
  readonly radius: number;
  readonly opacity?: number;
  readonly angle?: number;
}
/** Shared item styling. */
export interface FreeformStyle {
  readonly opacity?: number;
  readonly fill?: FreeformPaint;
  readonly stroke?: FreeformStrokeStyle;
  readonly shadows: readonly FreeformShadow[];
}
/** Meaning of decoded ink points. */
export type FreeformInkPointRole = 'splineControl' | 'renderedSample';
/** One PencilKit point with every recognized channel. */
export interface FreeformInkPoint {
  readonly x: number;
  readonly y: number;
  readonly timeOffset?: number;
  readonly width?: number;
  readonly height?: number;
  readonly opacity?: number;
  readonly force?: number;
  readonly azimuth?: number;
  readonly altitude?: number;
  readonly secondaryScale?: number;
  readonly threshold?: number;
}
/** A visible interval of a masked PencilKit path. */
export interface FreeformInkRange {
  readonly start: number;
  readonly end: number;
}
/** One freehand stroke. */
export interface FreeformInkStroke {
  readonly inkType: string;
  readonly inkIdentifier: string;
  readonly color?: FreeformColor;
  readonly transform: FreeformTransform;
  readonly pointRole: FreeformInkPointRole;
  readonly points: readonly FreeformInkPoint[];
  readonly visibleRanges?: readonly FreeformInkRange[];
  readonly randomSeed?: number;
  readonly rawData: Uint8Array;
}
/** Everything recovered from a PKDrawing blob. */
export interface FreeformDrawing {
  readonly requiredContentVersion?: number;
  readonly bounds?: FreeformFrame;
  readonly strokes: readonly FreeformInkStroke[];
}
/** A command in a local shape path. */
export type FreeformPathCommand =
  | { readonly kind: 'move'; readonly point: FreeformPoint }
  | { readonly kind: 'line'; readonly point: FreeformPoint }
  | { readonly kind: 'quadratic'; readonly control: FreeformPoint; readonly point: FreeformPoint }
  | {
      readonly kind: 'cubic';
      readonly control1: FreeformPoint;
      readonly control2: FreeformPoint;
      readonly point: FreeformPoint;
    }
  | { readonly kind: 'close' };
/** An ordered local shape path. */
export interface FreeformPath {
  readonly commands: readonly FreeformPathCommand[];
  readonly naturalSize?: FreeformSize;
  readonly rawData: Uint8Array;
}
/** Recursive value retained from a TSUDescription routing hint. */
export type TsuValue =
  | { readonly kind: 'null' }
  | { readonly kind: 'bool'; readonly value: boolean }
  | { readonly kind: 'integer'; readonly value: number }
  | { readonly kind: 'real'; readonly value: number }
  | { readonly kind: 'string'; readonly value: string }
  | { readonly kind: 'data'; readonly value: Uint8Array }
  | { readonly kind: 'array'; readonly value: readonly TsuValue[] }
  | { readonly kind: 'dictionary'; readonly value: Readonly<Record<string, TsuValue>> };
/** One TSUDescription board-item entry. */
export interface TsuEntry {
  readonly className: string;
  readonly hints: Readonly<Record<string, TsuValue>>;
}
/** One styled range in native text. */
export interface FreeformTextRun {
  readonly start: number;
  readonly end: number;
  readonly fontName?: string;
  readonly fontSize?: number;
  readonly bold?: boolean;
  readonly italic?: boolean;
  readonly underline?: string;
  readonly strikethrough?: string;
  readonly fill?: FreeformPaint;
  readonly paragraphAlignment?: string;
  readonly writingDirection?: string;
  readonly hyperlink?: string;
  readonly listStyle?: string;
}
/** Native text with style runs. */
export interface FreeformText {
  readonly plain: string;
  readonly runs: readonly FreeformTextRun[];
  readonly inset?: FreeformFrame;
  readonly verticalAlignment?: string;
  readonly shrinkToFit?: boolean;
}
/** One endpoint of a native connector. */
export interface FreeformConnectorEndpoint {
  readonly itemId?: string;
  readonly magnet?: string;
  readonly normalizedPosition?: FreeformPoint;
  readonly point?: FreeformPoint;
  readonly lineEnd?: string;
}
/** One native table cell. */
export interface FreeformTableCell {
  readonly id?: string;
  readonly row?: number;
  readonly column?: number;
  readonly rowSpan?: number;
  readonly columnSpan?: number;
  readonly style: FreeformStyle;
  readonly text?: FreeformText;
  readonly anchoredItemIds: readonly string[];
}
/** A referenced native asset and captured bytes when available. */
export interface FreeformAsset {
  readonly id: string;
  readonly filename?: string;
  readonly uti?: string;
  readonly bytes?: Uint8Array;
  readonly premium?: boolean;
  readonly intrinsicSize?: FreeformSize;
  readonly rawDescriptor: Uint8Array;
}
/** Item-specific native payload. */
export type FreeformItemKind =
  | { readonly kind: 'shape'; readonly preset?: string; readonly path?: FreeformPath }
  | { readonly kind: 'textBox'; readonly text?: FreeformText }
  | { readonly kind: 'stickyNote'; readonly text?: FreeformText }
  | {
      readonly kind: 'connector';
      readonly tail: FreeformConnectorEndpoint;
      readonly head: FreeformConnectorEndpoint;
      readonly routing?: string;
      readonly path?: FreeformPath;
    }
  | {
      readonly kind: 'table';
      readonly rowHeights: readonly number[];
      readonly columnWidths: readonly number[];
      readonly cells: readonly FreeformTableCell[];
    }
  | {
      readonly kind: 'image';
      readonly assetId?: string;
      readonly crop?: FreeformFrame;
      readonly mask?: FreeformPath;
    }
  | { readonly kind: 'media'; readonly assetId?: string; readonly mediaType?: string }
  | { readonly kind: 'file'; readonly assetId?: string; readonly filename?: string }
  | { readonly kind: 'url'; readonly url?: string; readonly title?: string }
  | {
      readonly kind: 'usdz';
      readonly assetId?: string;
      readonly spatialTransform?: readonly number[];
    }
  | {
      readonly kind: 'group';
      readonly childIds: readonly string[];
      readonly counterTransform?: FreeformTransform;
    }
  | { readonly kind: 'ink'; readonly strokes: readonly FreeformInkStroke[] }
  | { readonly kind: 'unknown' };
/** One structurally owned record in a native archive. */
export interface FreeformRecordRange {
  readonly offset: number;
  readonly length: number;
}

/** One entry of the native object's ordered board-item list. */
export interface FreeformBoardItem {
  readonly index: number;
  readonly uuid: string;
  readonly parentId?: string;
  readonly className?: string;
  readonly hints: Readonly<Record<string, TsuValue>>;
  readonly geometry: FreeformGeometry;
  readonly style: FreeformStyle;
  readonly kind: FreeformItemKind;
  readonly recordRanges: readonly FreeformRecordRange[];
  readonly rawData: Uint8Array;
}
/** Compatibility of a decoded native archive. */
export type FreeformCompatibility =
  | { readonly kind: 'unknown' }
  | { readonly kind: 'supported'; readonly version: number }
  | { readonly kind: 'unsupported'; readonly minimumVersion: number };
/** Everything recovered from CRLNativeData and correlated flavors. */
export interface FreeformNative {
  readonly pasteId: string;
  readonly compatibility: FreeformCompatibility;
  readonly items: readonly FreeformBoardItem[];
  readonly assets: Readonly<Record<string, FreeformAsset>>;
  readonly rawManifest: Uint8Array;
  readonly rawIndex: Uint8Array;
  readonly rawArchive: Uint8Array;
}
/** Failure category retained by pasteboard assembly. */
export type FreeformDecodeErrorKind =
  | 'invalid'
  | 'incomplete'
  | 'unsupportedVersion'
  | 'correlationMismatch';
/** A structured decoder failure suitable for fallback policy. */
export interface FreeformDecodeError {
  readonly kind: FreeformDecodeErrorKind;
  readonly message: string;
}
/** Outcome of one independently decoded pasteboard tier. */
export type FreeformTier<T> =
  | { readonly status: 'absent' }
  | { readonly status: 'decoded'; readonly value: T }
  | { readonly status: 'failed'; readonly value: FreeformDecodeError };
/** Exact bytes for one captured pasteboard flavor. */
export interface FreeformFlavor {
  readonly uti: string;
  readonly bytes: Uint8Array;
}
/** Atomic exact-flavor snapshot captured from one pasteboard change count. */
export interface FreeformBlobs {
  readonly changeCount?: number;
  readonly flavors: readonly FreeformFlavor[];
}
/** Native correlation metadata. */
export interface FreeformNativeMetadata {
  readonly pasteId?: string;
  readonly rawData: Uint8Array;
}
/** A rendered fallback flavor. */
export interface FreeformRender {
  readonly uti: string;
  readonly bytes: Uint8Array;
}
/** A non-fatal assembly diagnostic. */
export interface FreeformDiagnostic {
  readonly source: string;
  readonly message: string;
}
/** A decoded Freeform selection assembled without losing supplied flavors. */
export interface FreeformPasteboard {
  readonly drawing: FreeformTier<FreeformDrawing>;
  readonly native: FreeformTier<FreeformNative>;
  readonly manifest: FreeformTier<readonly TsuEntry[]>;
  readonly metadata: FreeformTier<FreeformNativeMetadata>;
  readonly assets: Readonly<Record<string, FreeformAsset>>;
  readonly renders: readonly FreeformRender[];
  readonly state: Readonly<Record<string, Uint8Array>>;
  readonly styles: readonly FreeformFlavor[];
  readonly text: readonly FreeformFlavor[];
  readonly unknownFlavors: readonly FreeformFlavor[];
  readonly diagnostics: readonly FreeformDiagnostic[];
}
/** Which recognized Freeform payload a file represents. */
export type FreeformBlobKind =
  | 'drawing'
  | 'crlNative'
  | 'nativeMetadata'
  | 'tsuDescription'
  | 'style'
  | 'state'
  | 'asset'
  | 'renderPng'
  | 'renderTiff'
  | 'renderPdf'
  | 'plainText'
  | 'richText';
/** Identify a blob from its exact captured type/name or prefix bytes. */
export function classifyBlob(name: string, bytes?: Uint8Array | null): FreeformBlobKind | undefined;
/** True when bytes start with PKDrawing's `wrd` signature. */
export function isPkDrawing(bytes: Uint8Array): boolean;
/** Decode a PKDrawing blob, throwing a structured {@link FreeformDecodeError}. */
export function decodePkDrawing(bytes: Uint8Array): FreeformDrawing;
/** Decode CRLNativeData, optionally joined with a TSUDescription blob. */
export function decodeCrlNative(
  bytes: Uint8Array,
  tsuDescription?: Uint8Array | null,
): FreeformNative;
/** Parse a TSUDescription manifest with recursive typed hint values. */
export function parseTsuDescription(bytes: Uint8Array): readonly TsuEntry[];
/** Decode an exact ordered flavor snapshot; each tier reports its own outcome. */
export function decodePasteboard(blobs: FreeformBlobs): FreeformPasteboard;
/** True when an exact snapshot includes a structure-carrying Freeform flavor. */
export function hasFreeformContent(blobs: FreeformBlobs): boolean;
