# Freeform pasteboard formats

The native schemas below were checked against Freeform **5.1 (656.40.9.0.2)**
and the Coherence framework shipped with macOS 27.2. They are private formats,
not a stable Apple interchange specification. The Rust core does not require
Apple frameworks; native APIs were used to establish reference behavior.

## Pasteboard transport

Preserve the exact ordered UTI/byte pairs from one pasteboard change count:

```sh
swift tools/dump_pasteboard.swift pb_dump
```

| Flavor | Purpose |
|---|---|
| `com.apple.drawing` | Flattened PencilKit drawing |
| `com.apple.freeform.CRLNativeData` | Native object tree |
| `com.apple.freeform.CRLNativeMetadata` | Paste correlation metadata |
| `com.apple.freeform.TSUDescription` | Ordered top-level classes and routing hints |
| `com.apple.freeform.CRLAsset.<id>` | Embedded asset bytes |
| `com.apple.freeform.pasteboardState.*` | Routing-state bytes |
| `com.apple.freeform.stylepasteboard` | Style-only selection |
| PNG, TIFF, PDF, plain text, RTF | Independent rendered/text representations |

Freeform's native data provider splits large native, metadata, and asset
payloads into the base flavor followed by `.1`, `.2`, and subsequent numeric
suffixes. Capture order is not chunk order. The decoder assembles contiguous
chunks before parsing; missing or duplicate chunks fail their transport without
discarding independent tiers. The final chunk has no separate transport marker,
so the inner format must also validate completeness.

Asset identifiers can themselves end in numbers. Native asset descriptors
resolve ambiguous asset continuations; the decoder does not guess from a suffix
alone. Original flavor order and bytes remain in `FreeformPasteboard.flavors`.
Dump-tool aliases with underscores are accepted alongside exact UTIs.

## Native archive framing

Every serialized object, including the pasteboard root, has this layout:

```text
[u64 little-endian ObjectInfo length]
[CRLProto.ObjectInfo protobuf]
[named data regions, in directory order]
[each referenced child object, recursively, in reference order]
```

`ObjectInfo` fields established by Freeform's generated protobuf name map:

| Field | Meaning |
|---|---|
| 1 | Persistence type |
| 2 | Minimum required version, repeated uint32 components |
| 3 | Child object references, including UUIDs |
| 4 | Asset references |
| 5 | Data directory entries: field 1 name, field 2 byte size |
| 6 | Object-option flags |
| 7 | Version required for good-enough fidelity |
| 8 | Version required for full fidelity |
| 9 | Persistence subtype |

The pasteboard root has persistence type 99. Its `objectData` region is a binary
plist containing `id`, `isSmartCopyPaste`, and ordered `boardItems` UUIDs. Those
UUIDs must match the root child references.

A directory entry supplies the **exact boundary** of its payload. Neither a
search for a binary-plist trailer nor a scan for `crdt` markers is needed to
frame the archive. Marker bytes inside a payload have no framing significance.
The parser rejects truncated regions, duplicate directory names or object IDs,
reference/index disagreement, and bytes beyond the declared object tree.

Known directory keys are `objectData`, `commonCRDTData`, `specificCRDTData`,
`capsuleData`, `alternateCommonCRDTData`, and `alternateSpecificCRDTData`.
Unknown named regions remain available through `recordRanges` into `rawArchive`.
Ranges include the original payload headers. The obsolete duplicated per-item
`rawData` is replaced by these named ranges.

Nested objects are not top-level pasteboard items. In the real mixed capture,
the table owns a separately serialized shape. It is exposed through
`table.children`, with a parent UUID and an explicit cell anchor.

## Coherence documents

Known CRDT payloads begin with `crdt` and a little-endian uint32 encoding version;
this decoder handles versions 6 and 7. The body is a `Proto_Document` protobuf.

| Document field | Meaning |
|---|---|
| 1 | Inline CRDT root |
| 3 | Capsule root reference |
| 4 | Capsule reference records |
| 6 | Document-local constant tables |

Constants contain packed UUIDs, repeated strings, and repeated opaque data.
References must be resolved against their own document, never a neighbouring
record. Capsule reference key paths select records in the document's object pool.

A CRDT composite has string keys and CRDT values. A CRRegister contains a
timestamp and a separately tagged CRValue. Custom CRValues use integer coding
keys; omitted key arrays denote dense zero-based order. The decoder distinguishes
these representations rather than recursively looking for plausible floats,
strings, or UUID-shaped bytes.

Supported CRValue alternatives include unsigned and zigzag-signed integers,
float32/float64, constant-table strings and UUIDs, opaque data, optionals,
repeated values, custom values, and capsule references. Ordered collections
use their sequence structure and timestamps; table axes are not inferred from
reference-record order.

## Native item semantics

| Persistence type | Item |
|---|---|
| 2, subtype 0 or absent | Group |
| 2, subtype 1 | Table |
| 2, subtype 2 | Surface container; raw payloads retained |
| 3 | Word-processing shape/text box |
| 4 | Connector |
| 5–9 | Image, movie, file, URL, USDZ |
| 10 | Freehand drawing container |
| 11 | Freehand drawing shape/stroke |
| 12 | Sticky note |

Native discriminators drive decoding even without TSU. TSU is decoded
independently and joined only after correlation. Its `textbox: true` hint
refines a `CRLWPShapeItem` into a text box. Other shapes retain their styled
labels in `Shape.text`.

### Geometry, paths, and styles

Common CRDT data carries object identity, parent affinity, and shadow state.
Parent affinity supplies native geometry; shape-derived affinities also carry
path-source data. Geometry includes position, size, width/height validity, and
flips. Clockwise archived degrees become negative radians. Zero-sized autogrowing
text-box geometry is retained rather than replaced with a guessed frame.

Bézier paths decode their native command records, including endpoint-relative
Float16 cubic controls. Parametric point/scalar/callout paths retain their native
controls in `FreeformPath.source`; they are not replaced with invented curves.
Path-local flips remain distinct from item-geometry flips.

Native fills in this schema are color fills, not the previously assumed gradient
or image-fill messages. Colors preserve RGB, CMYK, or monochrome components and
the source color space. `hex` is available only when sRGB conversion is
established; device CMYK and Display P3 are not mislabeled as sRGB.

Stroke styles retain native cap/join/line-end settings, smart or PencilKit brush
configuration, and separate brush-pattern offsets. Dash lengths and phase are
**factors of rendered stroke width**, not absolute point measurements. Rendering
round-dot patterns additionally requires the native cap/screen-scale behavior.

### Text and tables

Text storage is `CRAttributedString`, not a flat board-item protobuf string.
Its content and instructional text are separate. Consecutive UTF-16 attribute
ranges are converted to UTF-8 byte offsets; invalid surrogate boundaries fail
rather than shifting formatting onto another character. Presets and explicit
attributes populate font, size, decorations, fill, alignment, writing direction,
lists, and hyperlinks. Native text inset is a scalar point measurement.

A table's `capsuleData` holds ordered row/column dictionaries and a sparse cell
dictionary, whose values reference records in the capsule. Missing cells are
materialized with native defaults. Cell IDs are `ROWUUID/COLUMNUUID` strings.
Table-wide fill, stroke, border options, rounded-corner, autogrow, and direction
flags remain separate from individual cell styles.

An anchored child's `tableAnchor` retains its cell ID and **cell-local geometry**.
The normal item geometry remains parent-table-local. Cells reference their
anchored children by UUID; no geometry is assigned by byte proximity.

### Graphs, assets, and native ink

Connector attachments are UUID registers plus inherited shape geometry/path
state. Free endpoints, routing, line ends, and flips come from those native
structures. Groups expose ordered child references and recursively decoded
children; freehand containers use the same nonduplicative graph representation.

Media descriptors use exact asset identifiers, not the owning board-item UUID.
Captured `CRLAsset` bytes join those identifiers after semantic decoding.
Premium assets can legitimately have no embedded bytes. USDZ placement matrices
and model-orientation quaternions are separate values.

Native ink combines its shape path with `CRLProto_PKStrokePathData` delta
channels and mask. Unchanged channels reuse the previous native value. Genuine
PencilKit drawing payloads reuse the PencilKit decoder. Recognized shapes without
PencilKit stroke data retain their decoded paths/styles as shapes.

## PencilKit drawing bytes

`com.apple.drawing` begins with `wrd`; stroke points are packed according to
separate per-point and constant-channel bitmasks. There is **no universal point
stride**. Constant channels are applied before variable channels, and masks,
counts, and lengths must agree.

Decoded channels include local coordinates, time, width/height, pressure,
opacity, azimuth, altitude, secondary scale, threshold, and edge width. Points
remain spline controls; apply the preserved affine transform or use Apple's
`PKDrawing` API to obtain rendered samples. Bounds are unions of archived
canvas-space render bounds, not extrema of control points. Explicit random seeds
are retained; unproven identity-derived seeds are not fabricated.

Masked and erased strokes keep one stroke record (field 5) with the shared path
and list each visible stroke as a substroke in field 11. Every substroke is a
separate `PKStroke`: its ink, transform, and flags override the record's when
present and inherit them otherwise; its render bounds are its own. Substroke
field 10 holds closed mask polygons as stroke-local float32 `(x, y)` pairs, and
field 12 holds visible parametric ranges as consecutive inclusive `(start, end)`
values; every capture so far holds one range per substroke, so the pairing of
several ranges is inferred. An eraser splits one record into several substrokes
that all reference the full path.

## Verification and fidelity boundaries

The checked-in mixed-board capture exercises text-box routing, styled sticky
text, fills, paths, connector geometry, image references, table defaults/styling,
and a nested cell-anchored shape. PencilKit captures include varied channels,
noncircular sizes, nontrivial transforms, constant templates, and masked and
erased substrokes cross-checked against Apple's public API.

`FreeformCompatibility` takes the maximum minimum/good-enough-fidelity requirement
across the complete object tree. The captured root declares 2.4, but its table
requires 4.2 and premium image requires 4.4; reporting only the root's version
would hide those requirements. This gate is **not a promise of complete rendering
parity**.
Unknown item kinds and fields retain raw data; unsupported known encodings fail
explicitly. Fonts/layout, parametric path generation, color-profile conversion,
brush rendering, and unprojected newer fields may require a renderer or a
PNG/TIFF/PDF fallback. The decoder does not merge collaborative editing deltas
or implement Freeform's rendering engine.

When adding a mapping, change one property in Freeform, capture all flavors,
and compare against the native serializer or API. Synthetic data is useful for
malformed-input regressions, but cannot establish a private schema by itself.
