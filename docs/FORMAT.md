# Extracting Freeform Copy/Paste Data

> How to read what Apple **Freeform** puts on the system pasteboard when you copy
> board items, and how to turn it into something your app can import. Covers ink,
> shapes, sticky notes / text boxes, connectors, tables, images, and groups.
>
> **Status: reverse-engineered.** None of the `com.apple.freeform.*` types are
> documented by Apple; there is no spec and no public header. Everything below
> that is not a standard Apple API (`PKDrawing`, `PropertyListSerialization`) was
> derived by inspecting real pasteboard payloads and the Freeform binary
> (`/System/Applications/Freeform.app`, macOS 26.x). Treat the private formats as
> **unstable across OS versions** and gate any decoder on a version check. Claims
> marked `[inferred]` were reasoned from the binary's symbols/flags but not yet
> verified against a fixture containing that shape.

---

## 0. TL;DR — what to actually use

Freeform writes the same selection in several parallel representations. Preserve
all of them: each tier has different stability and fallback value.

| Need | Flavor | Library result |
|---|---|---|
| Pixels | `public.png` / `public.tiff` / PDF | ordered `FreeformRender` fallbacks |
| Freehand ink | `com.apple.drawing` | local B-spline controls, affine transform, proven point channels, exact ink metadata, raw stroke record |
| Item inventory | `com.apple.freeform.TSUDescription` | ordered classes and recursive typed hints |
| Native scene | `com.apple.freeform.CRLNativeData` | record-bounded typed items plus retained unknown records |
| Assets and routing state | `CRLAsset.*`, metadata/state/style/text flavors | exact bytes, correlation metadata, and typed known state |

Apple's `PKDrawing(data:)` remains the highest-fidelity ink renderer. The pure
Rust decoder does not call PencilKit: it preserves the private packed controls
and every proven channel, but keeps unsupported channels in `rawData` rather
than describing the result as a rendered sample.

> **Tooling.** [`tools/dump_pasteboard.swift`](../tools/dump_pasteboard.swift)
> captures an atomic flavor snapshot with an exact UTI manifest. The Rust and
> npm packages decode that snapshot into independent tier outcomes; one damaged
> flavor never discards another usable tier.

`CRL` = Freeform's internal class prefix. The app is built on the iWork shared
stack (the `TS*` frameworks: `TSUtility`, `TSAccessibility`, …), so the native
format is iWork-TSP-flavoured protobuf + CRDT, not a clean public schema.

---

## 1. How Freeform writes the pasteboard

A single Freeform copy produces these pasteboard types (verified on a freehand
selection; type *names* confirmed in the Freeform binary):

```
com.apple.freeform.CRLNativeMetadata     # tiny: paste UUID + flags
com.apple.freeform.CRLNativeData         # the full native object graph (the big one)
com.apple.freeform.TSUDescription        # binary plist: item-class manifest
com.apple.freeform.pasteboardState.*     # boolean routing flags (see §5)
com.apple.drawing                        # PKDrawing of all freehand ink (PencilKit)
public.png / "Apple PNG pasteboard type" # flat render
public.tiff / "NeXT TIFF v4.0 ..."       # flat render (large)
```

Other types appear **conditionally** (confirmed as registered names in the
binary; presence depends on the selection):

```
com.apple.freeform.CRLAsset.<id>         # raw bytes of an EMBEDDED (non-premium) asset; absent for premium/stock media
com.apple.freeform.stylepasteboard       # present for "Copy Style" (style only, no geometry)
public.rtf / public.utf8-plain-text      # only when copying a TEXT SELECTION, not whole items [inferred]
```

`CRLNativeMetadata` is `[8-byte LE length][protobuf]`; the protobuf carries the
**paste UUID** (16 bytes, big-endian) which also appears as `id` in
`CRLNativeData`'s index plist (§4). Use it to correlate the blobs.

---

## 2. Enumerate the pasteboard (Swift)

There is no first-class CLI for non-text types; `pbpaste` only sees text. Use
`NSPasteboard` directly:

```swift
import AppKit
let pb = NSPasteboard.general
print("changeCount:", pb.changeCount)
for t in pb.types ?? [] {
    let n = pb.data(forType: t)?.count ?? 0
    print(String(format: "%12d  %@", n, t.rawValue))
}
```

Run with `swift file.swift`. Dump any type to disk with
`pb.data(forType: NSPasteboard.PasteboardType("com.apple.drawing"))?.write(to:)`.

> `osascript -e 'clipboard info'` shows the classic flavor types (PNG/TIFF/…) but
> **not** the reverse-DNS `com.apple.freeform.*` ones — use the Swift enumeration.

---

## 3. Extraction tiers

### 3.1 Raster fallback — `public.png` / `public.tiff`

Flat, pre-rendered, transparent background. The PNG can be enormous (a sample
was 8840×10800). Only useful as a thumbnail or last resort. **Avoid** the TIFF
(it was ~380 MB uncompressed for the same selection).

### 3.2 Ink — `com.apple.drawing` is a `PKDrawing` (RECOMMENDED)

Freeform flattens **all** `CRLFreehandDrawingItem`s in the selection into one
PencilKit `PKDrawing` and puts it on `com.apple.drawing`. This is the single
best path for ink because `PKDrawing` is a **public, stable** API:

```swift
import PencilKit
let data = try Data(contentsOf: drawingURL)        // com.apple.drawing bytes
let drawing = try PKDrawing(data: data)
for stroke in drawing.strokes {
    let t = stroke.transform                        // CGAffineTransform — REQUIRED
    for p in stroke.path.interpolatedPoints(by: .parametricStep(0.5)) {
        let loc = p.location.applying(t)            // canvas-space point
        _ = (loc, p.size.width, p.force, p.opacity, p.azimuth, p.altitude)
    }
    _ = (stroke.ink.inkType, stroke.ink.color)      // .pen/.pencil/.marker, color
}
```

**Load-bearing details (verified):**

- **`stroke.transform` is mandatory.** Each stroke's path points are in
  *stroke-local* coordinates near the origin; the transform places (and may
  scale/rotate) the stroke on the canvas. Skip it and every stroke collapses to
  the top-left corner. Scale strokes' width by `sqrt(|a·d − b·c|)` of the
  transform if you flatten to a constant width.
- **Pressure is per-point** (`force`, plus `size.width`). SVG `stroke-width` is
  per-path, so a faithful export needs either one `<path>` per stroke at mean
  width (cheap, fine for thin pens) or a variable-width filled outline (use
  e.g. **perfect-freehand**, fed the `{x, y, pressure}` stream).
- `stroke.path` is a B-spline of control points; `interpolatedPoints(by:)`
  samples the rendered curve. `.parametricStep(0.5)` is a good density/size
  trade-off.
- Bounds come from `drawing.bounds`; ink color was sRGB, inkType `.pen` in the
  sample.

A reference `PKDrawing → SVG` conversion (one `<path>` per stroke, transform
applied, mean pressure-width, round caps) round-trips 1218 strokes into an
~830 KB SVG that matches the original visually 1:1.

### 3.3 Inventory — `com.apple.freeform.TSUDescription`

A **binary plist** — a per-item *hint manifest*. Parse with `plutil -p` or
`PropertyListSerialization`:

```
{ "appData": {…},
  "boardItems": [
    { "class": "Freeform.CRLWPShapeItem", "textbox": true, "text": [ {} ] },
    { "class": "Freeform.CRLWPStickyNoteItem",
      "text": [ { "hasText": true, "hasVisibleText": true } ] },
    { "class": "Freeform.CRLConnectionLineItem" },
    { "class": "Freeform.CRLTableItem", "disallowAnchoringToCRLTable": true },
    { "class": "Freeform.CRLImageItem", "containsPremiumContent": true },
    … ] }
```

Besides `class`, each entry carries cheap routing hints: `textbox` (text box vs
labeled shape), `text[].hasText` / `hasVisibleText`, `disallowAnchoringToCRLTable`,
`containsPremiumContent`. **Crucially, this `boardItems` array is the same length
and order as `CRLNativeData`'s index `boardItems` (§3.4-B)** — zip them to attach
a class + hints to each item UUID without touching the CRDT graph. (Verified on a
10-item mixed paste.)

### 3.4 Full native — `com.apple.freeform.CRLNativeData`

This holds the real object graph (geometry, text, style, connector endpoints,
asset refs). **Layout, verified on freehand (5-item), mixed (10-item), and table (1-item) pastes:**

```
┌────────────────────────────────────────────────────────────────────┐
│ A. Manifest    [8-byte LE length][protobuf]                          │
│      protobuf: field1=varint, field3=repeated 20-byte entries        │
│      each 20-byte entry = { 16-byte item UUID } ref.                 │
│      The repeat count == number of top-level board items.            │
├────────────────────────────────────────────────────────────────────┤
│ B. Index plist (binary plist, bounded — NOT to EOF)                  │
│      { id: "<paste UUID>",                                           │
│        isSmartCopyPaste: Bool,                                       │
│        boardItems: ["<item UUID>", …] }   // UUID strings, ordered   │
├────────────────────────────────────────────────────────────────────┤
│ C. Object archive (rest of the blob)                                 │
│      A CRDT object graph (strings "commonCRDTData"/"specificCRDTData"│
│      appear inline). Records are protobuf (iWork-TSP/`CRLProto_*`     │
│      messages, e.g. `CRLProto_ObjectMetadata`), keyed by 16-byte     │
│      UUIDs, with HETEROGENEOUS framing (mixed 8-byte-LE-length-       │
│      prefixed records and inline sub-messages). Ink references appear │
│      as `com.apple.ink.pen`; stroke geometry is duplicated here.     │
└────────────────────────────────────────────────────────────────────┘
```

**Parsing notes:**

- Section B does **not** run to EOF. Don't hand the whole blob to a plist
  parser — it reads the wrong 32-byte trailer and dies. Find the bplist trailer
  (5 zero bytes, then `offsetIntSize∈1..8`, `objectRefSize∈1..8`, a sane
  `numObjects`, and `offsetTableOffset` inside the slice), slice to it, then
  parse just that slice.
- The 16-byte UUIDs are big-endian raw bytes; format as
  `XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX`. The paste `id` in B equals the UUID in
  `CRLNativeMetadata`.
- Section C is **CRDT-encoded** (Freeform is collaborative — even a copy carries
  CRDT bookkeeping). Inline markers: `commonCRDTData`, `specificCRDTData`, and
  4-byte `crdt`/`bcrdt`/`dcrdt`/`fcrdt` tags fronting length-prefixed CRDT blobs.
  Verified recoverable as plain strings: shape **preset names** (e.g.
  `Parallelogram_950`), **font ids** (`com.apple.Freeform.system.font.regular` /
  `.semibold`), **text-style keys** (`fontSize`, `bold`, `paragraphAlignment`,
  `baseWritingDirection`, `characterFill`, `listStyle`, `capsuleData`), and
  **asset references** (`<UUID>.jpg`, `thumbnail`).
- **Text content *and* styling DECODE** — they're plain iWork `TSWP` protobuf; the
  CRDT blobs (`commonCRDTData`/`specificCRDTData`) are separate bookkeeping, not a
  lock. The text-storage message parses directly:
  - field **1** = the **UTF-8 string**, contiguous (verified: cell text `"aaa"`
    reads straight out — the earlier "interleaved/uncrackable" claim was **wrong**).
  - a character **property table** of `{propID → value}` runs. Verified by value:
    `propID 10` = **fontSize** (f32 points, e.g. `72`); `propID 9` = **characterFill**.
    Still unlabelled (values present, names need a one-property diff): `bold`,
    `paragraphAlignment`, `listStyle`, `baseWritingDirection`.
  - **Colors are sRGB**, encoded as three `#15 fixed32` channels nested under a
    `#14` colorspace wrapper (model index `0` = sRGB), optional alpha — NOT a flat
    RGB triple, which is why a naïve float scan misses them. Verified: pink
    `#EB539F` = `(0.922, 0.325, 0.624)`, blue `#5AC4F6` = `(0.353, 0.769, 0.965)`.
- Geometry (position/size/transform) and connector endpoints still need per-shape
  diffing (§8) before you trust the field numbers — but fills / text / size are solved.

### 3.5 Worked round-trip: native → vector SVG (verified)

A 2×2 table copy (`hasOnlyTableBoardItems`) was rebuilt as vector SVG **purely
from the decoded `CRLNativeData`** — no PencilKit, no tracing the raster.
What came out of the bytes, and how it cross-checked
against Freeform's own PNG:

| Quantity | From native decode | Confirmed in render (2 px/pt) |
|---|---|---|
| Column width | `344` pt | divider at 688 px = 2×344 |
| Bottom row height | `258` pt | 516 px |
| Embedded square | `150×150` pt at cell offset `(97, 109.5)` pt | 299×299 px at the matching spot |
| Pink text | `fontSize 72`, fill `#EB539F` | 144 px tall, exact hue |
| Cell text | `"aaa"` ×3 (TSWP field 1) | matches |
| Grid / cell fill | `#BFBFBF` / `#FFFFFF` | matches |

Takeaways: **the render scale is a clean 2 px/pt (Retina)**; geometry *is* present
in the blob as f32 points (column/row sizes, the square's frame), not just
inferable from pixels; and text + color + size all decode. The one cell whose
`fontSize` wasn't decoded (the small black labels) used the cell default, read
off the raster as ~17 pt. (Separately, the mixed fixture's sticky note decoded
its text `"hi"` and yellow fill `#FFE16C` — so note/label text content is
recoverable too.)

---

## 4. Board-item catalogue

The board-item classes Freeform can place on the pasteboard (names from the
binary; `Freeform.<Class>` is the form used in `TSUDescription`):

| Class | What it is | Where the data lives | Practical extraction |
|---|---|---|---|
| `CRLFreehandDrawingItem` | Pen/pencil/marker ink | `com.apple.drawing` (flattened) **and** `CRLNativeData §C` | **Tier 2 `PKDrawing`** — lossless |
| `CRLFreehandDrawingShapeItem` | Ink auto-recognized into a shape `[inferred]` | `CRLNativeData §C` | native decode |
| `CRLShapeItem` | Geometric shape with **no** text (rectangle, oval, line, polygon, library shapes) `[inferred]` | `CRLNativeData §C` (preset name + transform + fill/stroke) | native decode |
| `CRLWPShapeItem` ✓ | **Text box or labeled shape.** `TSUDescription` `textbox:true` ⇒ text box; else a shape carrying text. Preset name in §C (e.g. `Parallelogram_950`) | `CRLNativeData §C` (preset + transform + fill/stroke + TSWP text) | native decode; text + fill + size decode (§3.4) |
| `CRLWPStickyNoteItem` ✓ | **Sticky note** — square note with text | `CRLNativeData §C` (transform + fill + TSWP text) | native decode; text + fill + size decode (§3.4) |
| `CRLConnectionLineItem` ✓ | **Connector / arrow** | `CRLNativeData §C` (endpoints bind to item UUIDs; routing straight/orthogonal/curved) | native decode |
| `CRLTableItem` ✓ | **Table — self-contained.** Copying a table = **one** board item (`hasOnlyTableBoardItems`); cell text **and embedded shapes** are nested in its §C record, *not* sibling items | `CRLNativeData §C` (per-cell `TSWP` text + nested shape objects, each with its own `commonCRDTData`) | native decode; verified — cell text, font size, and fills (incl. an embedded square `#5AC4F6`) all decode |
| `CRLImageItem` ✓ | **Image** | §C references the asset by `<UUID>.<ext>` + a `thumbnail`; bytes arrive on `com.apple.freeform.CRLAsset.<id>` **only when not premium** | pull `CRLAsset.*` blob; premium/stock ⇒ reference only, use `public.png` |
| `CRLMovieItem` / `CRLMediaItem` | Video / media | `CRLAsset.<id>` blob `[inferred]` | pull the asset blob |
| `CRLImagePlaygroundItem` | Image-Playground image | as `CRLImageItem` `[inferred]` | pull the asset blob |
| `CRLGroupItem` | Group container `[inferred]` | `CRLNativeData §C` (holds child UUID refs) | recurse into children |

> **Sticky note vs text box vs shape (verified).** A **sticky note** is its own
> class (`CRLWPStickyNoteItem`). A **text box** and a **labeled shape** are both
> `CRLWPShapeItem`, told apart by `TSUDescription`'s `textbox` flag. A plain
> `CRLShapeItem` (no text) also exists. The `…NativeTextBoxItems` state flags
> track the text-box case. `✓` rows above were confirmed against a real paste.

---

## 5. `pasteboardState.*` flags — route without parsing

Freeform writes boolean flag types you can probe cheaply (presence/value) to
branch before doing any heavy decode. Verified flag names in the binary:

```
hasNativeBoardItems              hasNativeText
hasFreehandDrawingBoardItems     hasOnlyFreehandDrawingBoardItems
hasNativeBoardItemsContainingText
hasSingleNativeImageBoardItem    hasSingleNativeMovieBoardItem
hasOnlyNativeTextBoxItems        hasSingleNativeTextBoxItem
hasOnlyTableBoardItems           hasSingleTableBoardItem
hasUnsupportedCRLTableContent    hasNativeTypes
hasTextStoragesAttachmentsNotAllowed
```

Names use the `com.apple.freeform.pasteboardState.` prefix; premium content uses
`com.apple.apps.pasteboardState.TSAPasteboardStateTypeHasPremiumContent`. Probe
presence/value to branch before any heavy decode. Examples:
`hasOnlyFreehandDrawingBoardItems` ⇒ ignore `CRLNativeData`, take the `PKDrawing`;
`hasSingleNativeImageBoardItem` ⇒ grab `public.png` / the `CRLAsset` blob and skip
the graph; `…HasPremiumContent` ⇒ expect missing asset bytes (reference only).

---

## 6. Recommended import pipeline

1. Capture an atomic snapshot with `tools/dump_pasteboard.swift`. The generated
   `manifest.json` is authoritative for UTI names, flavor order, filenames, and
   `changeCount`.
2. Pass every exact UTI/byte pair to `decodePasteboard`; do not pre-collapse
   dynamic assets or state flags into fixed slots.
3. Check each `FreeformTier`: `decoded`, `failed`, or `absent`. Keep TSU,
   native, drawing, and rendered fallbacks independent.
4. Treat `com.apple.drawing` as the canonical ink source when it decodes.
   Points are local B-spline controls; apply the preserved affine transform or
   sample the spline before rendering.
5. Use the strictly correlated TSU/native result for non-ink items. Item kinds
   retain geometry/style, paths, styled text, connector endpoints, table cells,
   assets/media, group relationships, and bounded raw data for unsupported
   fields.
6. Resolve `CRLAsset.<id>` bytes through `FreeformNative.assets`. A missing
   payload with premium state is expected; an unmatched or truncated asset is a
   diagnostic rather than an invented image.
7. Fall back to the ordered render flavors when native compatibility or a tier
   failure prevents the required fidelity.

---

## 7. Caveats

- **Unstable native schema.** The decoder validates the CRL envelope, index,
  manifest, UUIDs, record ownership, and known compatibility metadata before
  semantic decoding. Unsupported record fields remain in `rawData`.
- **CRDT object graph.** Section C contains references and nested collaboration
  records. Values are associated only inside bounded owner records, never by
  nearest byte offset.
- **Private PencilKit bytes.** PencilKit's framework API is version-tolerant;
  the Rust byte decoder is a versioned reverse-engineered subset with raw
  fallback for unproven channels.
- **Partial transfer.** Universal Clipboard may leave only TSU or a render.
  Independent tier outcomes preserve those useful fallbacks.
- **Premium media.** Premium image/movie items can carry descriptors and a
  render without a `CRLAsset` payload. Absence is represented explicitly.

---

## 8. Producing fixtures & verifying new shape types

The reliable way to nail §3.4/§4 for a given shape is **differential analysis**:

1. In Freeform, put exactly one shape on a board, select it, copy.
2. Dump every pasteboard type to disk (§2), tagged by shape kind.
3. Change **one** property (color, then position, then text) and re-dump; diff
   the `CRLNativeData §C` bytes to localize each field.
4. Repeat per shape kind: rectangle, oval, sticky note, text box, arrow/
   connector, table, image, group.

**Verified by the checked-in real and synthetic fixtures:** strict A/B/C
envelope parsing; canonical UUID and manifest/index reconciliation; independent
TSU parsing and recursive hints; bounded item dispatch; local geometry and
flips; preset and Bézier path retention; solid/gradient style records; native
ink channels and transforms; connector/group references; styled text and table
cell structure; media descriptors and dynamic asset joins; and lossless raw
fallbacks for fields not yet mapped.

New OS releases and shape-specific fields still require differential captures:
change one property, capture atomically, and add the resulting record mapping
only when its ownership, wire type, and semantics are independently established.
