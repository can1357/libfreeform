//! Normalized, format-agnostic data produced by the Apple Freeform
//! pasteboard decoders. Decoders stay pure — bytes in, plain data out —
//! and colors are plain sRGB `#rrggbb` strings.
//!
//! With the `serde` feature, every type here derives `Serialize` and
//! `Deserialize` with camelCase field names (the shape the WebAssembly
//! bindings expose to JavaScript).

/// A sampled ink point in Freeform canvas space (stroke transform applied).
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformInkPoint {
   pub x:     f64,
   pub y:     f64,
   /// Per-point pressure, 0..1.
   pub force: f64,
   /// Per-point stroke width in points, as `PencilKit` stored it.
   pub width: f64,
}

/// One freehand stroke from either `PKDrawing` or native item storage.
/// `ink_type` is the `PencilKit` family (`pen`/`pencil`/`marker`/other).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformInkStroke {
   pub ink_type: String,
   /// sRGB `#rrggbb`.
   pub color:    String,
   /// Ink alpha, 0..1.
   pub opacity:  f64,
   pub points:   Vec<FreeformInkPoint>,
}

/// Everything recovered from the `com.apple.drawing` `PKDrawing` blob.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformDrawing {
   pub strokes: Vec<FreeformInkStroke>,
}

/// Canvas-space rectangle in Freeform points (1pt ~ 1 CSS px).
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformFrame {
   pub x:        f64,
   pub y:        f64,
   pub w:        f64,
   pub h:        f64,
   /// Rotation in radians.
   pub rotation: f64,
}

/// One entry of the native object graph's ordered board-item list.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformBoardItem {
   /// Position in the paste's board-item order.
   pub index:          usize,
   /// Item UUID from the index plist (`XXXXXXXX-XXXX-…`).
   pub uuid:           Option<String>,
   /// Local outline vertices for native polygonal shapes, when stored plainly.
   pub outline:        Option<Vec<(f64, f64)>>,
   /// Class from `TSUDescription`, `Freeform.` prefix stripped.
   pub class_name:     String,
   /// Cheap routing hints from `TSUDescription` (`textbox`, ...): key -> raw
   /// value.
   pub hints:          std::collections::HashMap<String, String>,
   /// Canvas-space frame decoded from the native `GeometryArchive`.
   pub frame:          Option<FreeformFrame>,
   /// Primary sRGB fill `#rrggbb` for this item.
   pub fill:           Option<String>,
   /// User text (sticky/label/cell content).
   pub text:           Option<String>,
   /// Native freehand strokes in item-local space (when `com.apple.drawing` is
   /// absent).
   pub native_strokes: Option<Vec<FreeformInkStroke>>,
}

/// Everything recovered from `com.apple.freeform.CRLNativeData` (+
/// `TSUDescription`).
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct FreeformNative {
   /// Paste UUID from the index plist.
   pub paste_id: Option<String>,
   pub items:    Vec<FreeformBoardItem>,
   /// Heuristically recovered TSWP text runs, byte order.
   pub texts:    Vec<String>,
   /// Heuristically recovered sRGB fills, byte order.
   pub colors:   Vec<String>,
}

/// A decoded Freeform selection assembled from one or more flavors.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FreeformPasteboard {
   pub drawing:    Option<FreeformDrawing>,
   pub native:     Option<FreeformNative>,
   /// Full-board `public.png` render (premium-image crops).
   pub render_png: Option<Vec<u8>>,
}

/// Raw pasteboard flavors captured from a Freeform copy.
#[derive(Debug, Clone, Default)]
pub struct FreeformBlobs {
   /// `com.apple.drawing` — flattened freehand ink (`PKDrawing`).
   pub drawing:         Option<Vec<u8>>,
   /// `com.apple.freeform.CRLNativeData` — the native object graph.
   pub crl_native:      Option<Vec<u8>>,
   /// `com.apple.freeform.TSUDescription` — the board-item class manifest.
   pub tsu_description: Option<Vec<u8>>,
   /// `public.png` — full-board render fallback.
   pub render_png:      Option<Vec<u8>>,
}

/// A blob's header/version is not one this decoder understands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreeformDecodeError(pub String);

impl std::fmt::Display for FreeformDecodeError {
   fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
      write!(f, "freeform decode: {}", self.0)
   }
}

impl std::error::Error for FreeformDecodeError {}
