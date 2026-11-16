//! Lossless, format-agnostic data produced by the Apple Freeform pasteboard
//! decoders.
//!
//! Values with unknown provenance remain optional, unsupported records retain
//! their raw bytes, and pasteboard tiers report failures independently.

use std::collections::BTreeMap;

/// A two-dimensional point in Freeform coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformPoint {
   /// Horizontal coordinate in points.
   pub x: f64,
   /// Vertical coordinate in points.
   pub y: f64,
}

/// A two-dimensional size in Freeform points.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformSize {
   /// Horizontal extent in points.
   pub width:  f64,
   /// Vertical extent in points.
   pub height: f64,
}

/// A complete affine transform using Core Graphics component order.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformTransform {
   /// Horizontal scale/rotation component.
   pub a:  f64,
   /// Vertical shear/rotation component.
   pub b:  f64,
   /// Horizontal shear/rotation component.
   pub c:  f64,
   /// Vertical scale/rotation component.
   pub d:  f64,
   /// Horizontal translation.
   pub tx: f64,
   /// Vertical translation.
   pub ty: f64,
}

impl Default for FreeformTransform {
   fn default() -> Self {
      Self { a: 1.0, b: 0.0, c: 0.0, d: 1.0, tx: 0.0, ty: 0.0 }
   }
}

/// Canvas-space rectangle in Freeform points.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformFrame {
   /// Horizontal origin.
   pub x:        f64,
   /// Vertical origin.
   pub y:        f64,
   /// Width in points.
   pub w:        f64,
   /// Height in points.
   pub h:        f64,
   /// Rotation in radians when the archive exposes a decomposed angle.
   pub rotation: f64,
}

/// Local bounds and transform for a native board item.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformGeometry {
   /// Archive frame, when decoded without invention.
   pub frame:           Option<FreeformFrame>,
   /// Full affine transform, when present in the archive.
   pub transform:       Option<FreeformTransform>,
   /// Transform anchor in local coordinates.
   pub anchor:          Option<FreeformPoint>,
   /// Explicit horizontal flip state.
   pub horizontal_flip: Option<bool>,
   /// Explicit vertical flip state.
   pub vertical_flip:   Option<bool>,
   /// Whether the archived width is authoritative.
   pub width_valid:     Option<bool>,
   /// Whether the archived height is authoritative.
   pub height_valid:    Option<bool>,
}

/// Native color components, without lossy conversion between color models.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")
)]
pub enum FreeformColorComponents {
   /// Three channels in the declared RGB color space.
   Rgb {
      /// Red channel in the declared RGB space.
      red:   f64,
      /// Green channel in the declared RGB space.
      green: f64,
      /// Blue channel in the declared RGB space.
      blue:  f64,
   },
   /// Device CMYK channels requiring a color profile for RGB conversion.
   Cmyk {
      /// Native cyan component.
      cyan:    f64,
      /// Native magenta component.
      magenta: f64,
      /// Native yellow component.
      yellow:  f64,
      /// Native black component.
      black:   f64,
   },
   /// One white channel in the declared monochrome space.
   Monochrome {
      /// Native monochrome white component.
      white: f64,
   },
}

/// A source-space color with an optional exact sRGB convenience representation.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformColor {
   /// Source color-space identifier.
   pub color_space: String,
   /// Components in the archived color model and color space.
   pub components:  FreeformColorComponents,
   /// Alpha channel.
   pub alpha:       f64,
   /// Rounded CSS RGB value when conversion to sRGB is established.
   pub hex:         Option<String>,
}

/// A native fill or stroke paint.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")
)]
pub enum FreeformPaint {
   /// A solid color.
   Solid {
      /// Color and alpha in their source space.
      color: FreeformColor,
   },
   /// An unsupported paint whose archive bytes remain available.
   Unknown {
      /// Original unsupported paint payload.
      raw_data: Vec<u8>,
   },
}

/// Native `PencilKit` brush configuration attached to a geometric stroke.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformPencilKitStrokeStyle {
   /// Exact native ink identifier.
   pub ink_type:        String,
   /// Native brush width.
   pub width:           f64,
   /// Explicit fountain-pen implementation selector, when archived.
   pub fountain_pen_v2: Option<bool>,
}

/// Stroke styling for shapes and connectors.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformStrokeStyle {
   /// Stroke paint.
   pub paint:                   FreeformPaint,
   /// Stroke width in local points.
   pub width:                   f64,
   /// Native dash lengths as factors of the rendered stroke width.
   pub dash:                    Vec<f64>,
   /// Native dash phase as a factor of the rendered stroke width.
   pub dash_offset:             Option<f64>,
   /// Brush-pattern offset, distinct from dash phase.
   pub pattern_offset_distance: Option<f64>,
   /// Native smart-stroke brush family.
   pub smart_stroke:            Option<String>,
   /// `PencilKit` brush configuration when the stroke uses one.
   pub pencil_kit:              Option<FreeformPencilKitStrokeStyle>,
   /// Native cap name.
   pub cap:                     Option<String>,
   /// Native join name.
   pub join:                    Option<String>,
   /// Miter limit.
   pub miter_limit:             Option<f64>,
   /// Tail decoration.
   pub tail_end:                Option<String>,
   /// Head decoration.
   pub head_end:                Option<String>,
}

/// One native shadow effect.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformShadow {
   /// Shadow kind such as drop, contact, or curved.
   pub kind:     String,
   /// Shadow color.
   pub color:    FreeformColor,
   /// Horizontal offset.
   pub offset_x: f64,
   /// Vertical offset.
   pub offset_y: f64,
   /// Blur radius.
   pub radius:   f64,
   /// Shadow opacity when distinct from the color alpha.
   pub opacity:  Option<f64>,
   /// Native angle when present.
   pub angle:    Option<f64>,
   /// Native curvature parameter for curved shadows.
   pub curve:    Option<f64>,
}

/// Shared visual styling for a native board item.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformStyle {
   /// Whole-item opacity.
   pub opacity: Option<f64>,
   /// Fill paint.
   pub fill:    Option<FreeformPaint>,
   /// Stroke style.
   pub stroke:  Option<FreeformStrokeStyle>,
   /// Ordered shadow effects.
   pub shadows: Vec<FreeformShadow>,
}

/// Whether decoded ink points are spline controls or rendered samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub enum FreeformInkPointRole {
   /// Native uniform-cubic-B-spline control records.
   SplineControl,
   /// Points sampled from the rendered curve.
   RenderedSample,
}

/// A `PencilKit` point with every channel the decoder could identify.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformInkPoint {
   /// Local horizontal coordinate unless the stroke role documents otherwise.
   pub x:               f64,
   /// Local vertical coordinate unless the stroke role documents otherwise.
   pub y:               f64,
   /// Time offset from the beginning of the stroke.
   pub time_offset:     Option<f64>,
   /// Nib width.
   pub width:           Option<f64>,
   /// Nib height.
   pub height:          Option<f64>,
   /// Per-point opacity.
   pub opacity:         Option<f64>,
   /// Per-point pressure.
   pub force:           Option<f64>,
   /// Stylus azimuth in radians.
   pub azimuth:         Option<f64>,
   /// Stylus altitude in radians.
   pub altitude:        Option<f64>,
   /// Versioned secondary scale/radius channel.
   pub secondary_scale: Option<f64>,
   /// Versioned clipping threshold.
   pub threshold:       Option<f64>,
   /// Native edge-width channel in local points.
   pub edge_width:      Option<f64>,
}

/// A visible interval of a masked `PencilKit` path.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformInkRange {
   /// Inclusive parametric start.
   pub start: f64,
   /// Inclusive parametric end.
   pub end:   f64,
}

/// One freehand stroke from `PencilKit` or native item storage.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformInkStroke {
   /// Normalized ink family.
   pub ink_type:       String,
   /// Exact archived ink identifier.
   pub ink_identifier: String,
   /// Ink color when decoded.
   pub color:          Option<FreeformColor>,
   /// Stroke-local to canvas transform.
   pub transform:      FreeformTransform,
   /// Meaning of entries in `points`.
   pub point_role:     FreeformInkPointRole,
   /// Ordered point/control records.
   pub points:         Vec<FreeformInkPoint>,
   /// Visible path intervals after applying a `PencilKit` mask.
   pub visible_ranges: Option<Vec<FreeformInkRange>>,
   /// Native clipping path in stroke-local coordinates; `PencilKit` stores
   /// closed polygons.
   pub mask:           Option<FreeformPath>,
   /// Seed used by randomized ink textures.
   pub random_seed:    Option<u32>,
   /// Original stroke record for unsupported channels or future replay;
   /// strokes erased from one record share it.
   pub raw_data:       Vec<u8>,
}

/// Everything recovered from a `com.apple.drawing` `PKDrawing` blob.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformDrawing {
   /// `PencilKit` content version when decoded.
   pub required_content_version: Option<u32>,
   /// Drawing bounds in canvas coordinates.
   pub bounds:                   Option<FreeformFrame>,
   /// Ordered strokes.
   pub strokes:                  Vec<FreeformInkStroke>,
}

/// A command in a local shape path.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")
)]
pub enum FreeformPathCommand {
   /// Starts a new subpath.
   Move {
      /// Local endpoint or native control position.
      point: FreeformPoint,
   },
   /// Adds a straight segment.
   Line {
      /// Local endpoint or native control position.
      point: FreeformPoint,
   },
   /// Adds a quadratic Bézier segment.
   Quadratic {
      /// Local quadratic control point.
      control: FreeformPoint,
      /// Local endpoint or native control position.
      point:   FreeformPoint,
   },
   /// Adds a cubic Bézier segment.
   Cubic {
      /// First control point.
      control_1: FreeformPoint,
      /// Second control point.
      control_2: FreeformPoint,
      /// Segment endpoint.
      point:     FreeformPoint,
   },
   /// Closes the current subpath.
   Close,
}

/// Parameters for native paths that are generated rather than stored as Bézier
/// commands.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")
)]
pub enum FreeformPathSource {
   /// Point-controlled native path.
   Point {
      /// Native parametric path identifier.
      name:  String,
      /// Local endpoint or native control position.
      point: FreeformPoint,
   },
   /// Scalar-controlled native path.
   Scalar {
      /// Native parametric path identifier.
      name:             String,
      /// Archived path-control parameter.
      scalar:           f64,
      /// Whether the native curve uses continuous curvature.
      curve_continuous: bool,
      /// Whether the native path exposes its adjustment knob.
      show_knob:        bool,
   },
   /// Speech-callout path with its exact native controls.
   Callout {
      /// Callout tail position in local coordinates.
      tail_position: FreeformPoint,
      /// Native callout tail-size parameter.
      tail_size:     f64,
      /// Native callout corner radius.
      corner_radius: f64,
      /// Whether the tail is centered on its attachment.
      center_tail:   bool,
   },
}

/// An ordered local shape path with explicit subpaths and curves.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformPath {
   /// Path commands in archive order.
   pub commands:        Vec<FreeformPathCommand>,
   /// Natural source size used to scale preset paths.
   pub natural_size:    Option<FreeformSize>,
   /// Native parameters when no explicit Bézier command list is stored.
   pub source:          Option<FreeformPathSource>,
   /// Path-local flips, distinct from the containing item's geometry flips.
   pub horizontal_flip: Option<bool>,
   /// Path-local vertical flip.
   pub vertical_flip:   Option<bool>,
   /// Original path payload.
   pub raw_data:        Vec<u8>,
}

/// Recursive value retained from a `TSUDescription` routing hint.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(tag = "kind", content = "value", rename_all = "camelCase")
)]
pub enum TsuValue {
   /// Null plist value.
   Null,
   /// Boolean plist value.
   Bool(bool),
   /// Signed integer plist value.
   Integer(i64),
   /// Floating-point plist value.
   Real(f64),
   /// String plist value.
   String(String),
   /// Raw data plist value.
   Data(Vec<u8>),
   /// Ordered array plist value.
   Array(Vec<Self>),
   /// Dictionary plist value.
   Dictionary(BTreeMap<String, Self>),
}

/// One `TSUDescription` board-item entry.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct TsuEntry {
   /// Exact class name with only a leading `Freeform.` namespace removed.
   pub class_name: String,
   /// Structured routing hints.
   pub hints:      BTreeMap<String, TsuValue>,
}

/// One styled range in a native text storage.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformTextRun {
   /// Byte start in the plain UTF-8 string.
   pub start:               usize,
   /// Byte end in the plain UTF-8 string.
   pub end:                 usize,
   /// Font postscript identifier.
   pub font_name:           Option<String>,
   /// Font size in points.
   pub font_size:           Option<f64>,
   /// Bold state.
   pub bold:                Option<bool>,
   /// Italic state.
   pub italic:              Option<bool>,
   /// Underline style.
   pub underline:           Option<String>,
   /// Strikethrough style.
   pub strikethrough:       Option<String>,
   /// Character fill.
   pub fill:                Option<FreeformPaint>,
   /// Paragraph alignment.
   pub paragraph_alignment: Option<String>,
   /// Base writing direction.
   pub writing_direction:   Option<String>,
   /// Hyperlink target.
   pub hyperlink:           Option<String>,
   /// Native list-style name.
   pub list_style:          Option<String>,
   /// Explicit starting number for a numbered list.
   pub list_start:          Option<i64>,
   /// Native paragraph/list nesting level.
   pub paragraph_level:     Option<i64>,
}

/// Native text with preserved style runs.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformText {
   /// Complete UTF-8 text.
   pub plain:              String,
   /// Character and paragraph style runs.
   pub runs:               Vec<FreeformTextRun>,
   /// Uniform text inset in local points.
   pub inset:              Option<f64>,
   /// Native vertical-alignment name.
   pub vertical_alignment: Option<String>,
   /// Whether the container shrinks text to fit.
   pub shrink_to_fit:      Option<bool>,
}

/// One endpoint of a native connector.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformConnectorEndpoint {
   /// Connected item UUID.
   pub item_id:             Option<String>,
   /// Native magnet name.
   pub magnet:              Option<String>,
   /// Normalized magnet position.
   pub normalized_position: Option<FreeformPoint>,
   /// Unattached local endpoint.
   pub point:               Option<FreeformPoint>,
   /// Line-end decoration.
   pub line_end:            Option<String>,
}

/// One native table cell.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformTableCell {
   /// Stable cell identifier.
   pub id:                Option<String>,
   /// Zero-based row index.
   pub row:               Option<usize>,
   /// Zero-based column index.
   pub column:            Option<usize>,
   /// Number of spanned rows.
   pub row_span:          Option<usize>,
   /// Number of spanned columns.
   pub column_span:       Option<usize>,
   /// Cell fill/style.
   pub style:             FreeformStyle,
   /// Cell text.
   pub text:              Option<FreeformText>,
   /// UUIDs of shapes anchored inside the cell.
   pub anchored_item_ids: Vec<String>,
}

/// A referenced native asset and any captured bytes.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformAsset {
   /// Archive asset identifier.
   pub id:             String,
   /// Original filename when present.
   pub filename:       Option<String>,
   /// Pasteboard UTI carrying the bytes.
   pub uti:            Option<String>,
   /// Captured bytes; absent for premium or incomplete transfers.
   pub bytes:          Option<Vec<u8>>,
   /// Whether the item is premium/stock media.
   pub premium:        Option<bool>,
   /// Intrinsic media size.
   pub intrinsic_size: Option<FreeformSize>,
   /// Unsupported raw asset descriptor.
   pub raw_descriptor: Vec<u8>,
}

/// Native table rendering and layout flags, independent of per-cell paint.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformTableOptions {
   /// Archived border-option bit set.
   pub border_options:  Option<i64>,
   /// Whether the native table has rounded outer corners.
   pub rounded_corners: Option<bool>,
   /// Whether cells grow automatically with their contents.
   pub cell_autogrow:   Option<bool>,
   /// Whether the native table is laid out right-to-left.
   pub right_to_left:   Option<bool>,
}

/// Item-specific native payload.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")
)]
pub enum FreeformItemKind {
   /// A geometric or library shape.
   Shape {
      /// Native shape-library preset identifier.
      preset: Option<String>,
      /// Explicit local path when stored in the archive.
      path:   Option<FreeformPath>,
      /// Styled label carried by a word-processing shape.
      text:   Option<FreeformText>,
   },
   /// A standalone text box.
   TextBox {
      /// Decoded content and style runs, when available.
      text: Option<FreeformText>,
   },
   /// A sticky note.
   StickyNote {
      /// Decoded content and style runs, when available.
      text: Option<FreeformText>,
   },
   /// A connector or arrow.
   Connector {
      /// Tail endpoint.
      tail:    FreeformConnectorEndpoint,
      /// Head endpoint.
      head:    FreeformConnectorEndpoint,
      /// Routing kind such as straight, corner, or curved.
      routing: Option<String>,
      /// Explicit routed path.
      path:    Option<FreeformPath>,
   },
   /// A native table.
   Table {
      /// Row heights in points.
      row_heights:   Vec<f64>,
      /// Column widths in points.
      column_widths: Vec<f64>,
      /// Cells in archive order.
      cells:         Vec<FreeformTableCell>,
      /// Native table-wide rendering and layout flags.
      options:       FreeformTableOptions,
   },
   /// An image item.
   Image {
      /// Referenced asset ID.
      asset_id: Option<String>,
      /// Crop rectangle in source coordinates.
      crop:     Option<FreeformFrame>,
      /// Optional mask path.
      mask:     Option<FreeformPath>,
   },
   /// A movie or other media item.
   Media {
      /// Identifier used to resolve captured asset bytes.
      asset_id:   Option<String>,
      /// Archived media type, when available.
      media_type: Option<String>,
   },
   /// A file attachment.
   File {
      /// Identifier used to resolve captured asset bytes.
      asset_id: Option<String>,
      /// Original attachment filename.
      filename: Option<String>,
   },
   /// A URL/link-preview item.
   Url {
      /// Resolved link target.
      url:   Option<String>,
      /// Archived link-preview title.
      title: Option<String>,
   },
   /// A USDZ spatial item.
   Usdz {
      /// Referenced USDZ asset identifier.
      asset_id:          Option<String>,
      /// Column-major native 4×4 placement matrix.
      spatial_transform: Option<Vec<f64>>,
      /// Native model orientation quaternion in x, y, z, w order.
      orientation:       Option<[f64; 4]>,
   },
   /// A group container.
   Group {
      /// Child UUIDs in group-local z-order.
      child_ids:         Vec<String>,
      /// Transform that preserves child placement when leaving the group.
      counter_transform: Option<FreeformTransform>,
   },
   /// Native freehand ink used only when `PKDrawing` is unavailable.
   Ink {
      /// Ordered native strokes in local coordinates.
      strokes: Vec<FreeformInkStroke>,
   },
   /// An unsupported item whose payload remains available.
   Unknown,
}

/// Named payload range in [`FreeformNative::raw_archive`], including its format
/// header.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformRecordRange {
   /// Exact native data-directory key.
   pub name:   String,
   /// Offset from the beginning of the native object archive.
   pub offset: usize,
   /// Record payload length.
   pub length: usize,
}

/// A child item's cell-relative placement within its parent table.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformTableAnchor {
   /// Stable row UUID and column UUID separated by `/`.
   pub cell_id:  String,
   /// Native geometry relative to the containing cell.
   pub geometry: FreeformGeometry,
}

/// One entry of the native object graph's ordered board-item list.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformBoardItem {
   /// Position in the paste's top-level item order.
   pub index:               usize,
   /// Item UUID from the typed native index.
   pub uuid:                String,
   /// Parent group, table, or ink-container UUID for nested items.
   pub parent_id:           Option<String>,
   /// Cell-relative placement when the parent is a table.
   pub table_anchor:        Option<FreeformTableAnchor>,
   /// Native persistence discriminator, retained for newer item kinds.
   pub persistence_type:    u32,
   /// Native persistence subtype when present.
   pub persistence_subtype: Option<u32>,
   /// Class established by the native discriminator or a correlated TSU entry.
   pub class_name:          Option<String>,
   /// Structured TSU routing hints.
   pub hints:               BTreeMap<String, TsuValue>,
   /// Item geometry.
   pub geometry:            FreeformGeometry,
   /// Item style.
   pub style:               FreeformStyle,
   /// Item-specific payload.
   pub kind:                FreeformItemKind,
   /// Nested objects in the order declared by this object's child references.
   pub children:            Vec<Self>,
   /// Exact named payload ranges; all original bytes remain in the native
   /// archive.
   pub record_ranges:       Vec<FreeformRecordRange>,
}

/// Compatibility of a decoded native archive, from the Freeform versions its
/// manifest declares.
///
/// A version is a list of dotted components (`[2, 4]` is Freeform 2.4). An
/// archive needs the newer of its minimum version and its version for
/// good-enough fidelity; it is supported when the decoders have been verified
/// on archives needing that version. The maximum includes nested objects:
/// Freeform 5.1's captured premium image requires 4.4 even when its pasteboard
/// root declares 2.4 for good-enough fidelity.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")
)]
pub enum FreeformCompatibility {
   /// The manifest declares no Freeform version.
   #[default]
   Unknown,
   /// The archive needs Freeform `version`, which the decoders are verified on.
   Supported {
      /// Required Freeform version as dotted components.
      version: Vec<u32>,
   },
   /// The archive needs Freeform `version`, newer than the decoders are
   /// verified on: its records may use layouts they misread or skip.
   Unsupported {
      /// Required Freeform version as dotted components.
      version: Vec<u32>,
   },
}

/// Everything recovered from `CRLNativeData` and its correlated flavors.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformNative {
   /// Paste UUID from the typed native index.
   pub paste_id:      String,
   /// Native archive compatibility status.
   pub compatibility: FreeformCompatibility,
   /// Top-level items in native order.
   pub items:         Vec<FreeformBoardItem>,
   /// Asset descriptors recovered from native item records, keyed by asset ID.
   pub assets:        BTreeMap<String, FreeformAsset>,
   /// Original manifest protobuf.
   pub raw_manifest:  Vec<u8>,
   /// Original index plist.
   pub raw_index:     Vec<u8>,
   /// Original object archive for unsupported records.
   pub raw_archive:   Vec<u8>,
}

/// Decoding failure category retained by pasteboard assembly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub enum FreeformDecodeErrorKind {
   /// Structurally invalid input.
   Invalid,
   /// Truncated or incomplete transfer.
   Incomplete,
   /// Recognized but unsupported format version.
   UnsupportedVersion,
   /// Flavors from different selections could not be correlated.
   CorrelationMismatch,
}

/// A structured decode error with enough context for fallback policy.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformDecodeError {
   /// Failure category.
   pub kind:    FreeformDecodeErrorKind,
   /// Human-readable detail.
   pub message: String,
}

impl FreeformDecodeError {
   /// Creates a structural invalid-input error.
   pub fn invalid(message: impl Into<String>) -> Self {
      Self { kind: FreeformDecodeErrorKind::Invalid, message: message.into() }
   }

   /// Creates a truncated-transfer error.
   pub fn incomplete(message: impl Into<String>) -> Self {
      Self { kind: FreeformDecodeErrorKind::Incomplete, message: message.into() }
   }

   /// Creates an unsupported-version error.
   pub fn unsupported(message: impl Into<String>) -> Self {
      Self { kind: FreeformDecodeErrorKind::UnsupportedVersion, message: message.into() }
   }

   /// Creates a flavor-correlation error.
   pub fn correlation(message: impl Into<String>) -> Self {
      Self { kind: FreeformDecodeErrorKind::CorrelationMismatch, message: message.into() }
   }
}

impl std::fmt::Display for FreeformDecodeError {
   fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
      write!(f, "freeform decode: {}", self.message)
   }
}

impl std::error::Error for FreeformDecodeError {}

/// Outcome of decoding one independent pasteboard tier.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(tag = "status", content = "value", rename_all = "camelCase")
)]
#[derive(Default)]
pub enum FreeformTier<T> {
   /// The flavor was not supplied.
   #[default]
   Absent,
   /// The flavor decoded successfully.
   Decoded(T),
   /// The flavor was retained but could not be decoded.
   Failed(FreeformDecodeError),
}

/// Raw bytes for one exact pasteboard flavor.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformFlavor {
   /// Exact UTI or captured pasteboard type.
   pub uti:   String,
   /// Flavor bytes.
   pub bytes: Vec<u8>,
}

/// Atomic set of raw flavors captured from one pasteboard change count.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformBlobs {
   /// `NSPasteboard` change count observed before and after capture.
   pub change_count: Option<i64>,
   /// Exact flavors in pasteboard order.
   pub flavors:      Vec<FreeformFlavor>,
}

/// Correlation metadata from `CRLNativeMetadata`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformNativeMetadata {
   /// Paste UUID shared with the `CRL` native index.
   pub paste_id: Option<String>,
   /// Original metadata protobuf.
   pub raw_data: Vec<u8>,
}

/// A rendered fallback flavor.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformRender {
   /// Exact render UTI.
   pub uti:   String,
   /// Encoded PNG/TIFF/PDF bytes.
   pub bytes: Vec<u8>,
}

/// A non-fatal assembly diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformDiagnostic {
   /// Flavor or subsystem producing the diagnostic.
   pub source:  String,
   /// Diagnostic detail.
   pub message: String,
}

/// A decoded Freeform selection assembled without losing supplied flavors.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformPasteboard {
   /// Original flavors in capture order, before transport chunks are assembled.
   pub flavors:         Vec<FreeformFlavor>,
   /// `PencilKit` tier outcome.
   pub drawing:         FreeformTier<FreeformDrawing>,
   /// Native graph tier outcome.
   pub native:          FreeformTier<FreeformNative>,
   /// Standalone TSU manifest tier outcome.
   pub manifest:        FreeformTier<Vec<TsuEntry>>,
   /// Native correlation metadata.
   pub metadata:        FreeformTier<FreeformNativeMetadata>,
   /// Captured asset payloads keyed by asset ID.
   pub assets:          BTreeMap<String, FreeformAsset>,
   /// Render fallbacks in pasteboard order.
   pub renders:         Vec<FreeformRender>,
   /// Exact pasteboard-state values.
   pub state:           BTreeMap<String, Vec<u8>>,
   /// Style-only pasteboard flavors.
   pub styles:          Vec<FreeformFlavor>,
   /// Plain-text and rich-text selection flavors.
   pub text:            Vec<FreeformFlavor>,
   /// Unknown flavors retained for future decoders.
   pub unknown_flavors: Vec<FreeformFlavor>,
   /// Non-fatal correlation and partial-fidelity diagnostics.
   pub diagnostics:     Vec<FreeformDiagnostic>,
}
