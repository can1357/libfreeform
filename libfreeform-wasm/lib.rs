//! WebAssembly bindings for [`libfreeform`], published to npm as `libfreeform`.
//!
//! Decoded values cross the boundary as plain JavaScript objects with
//! camelCase keys (via `serde-wasm-bindgen`); decode errors surface as thrown
//! `Error`s. The npm package's `index.d.ts` is the typed public surface —
//! keep it in sync with the shapes serialized here (which are the `serde`
//! feature's camelCase view of `libfreeform::types`).
//!
//! The raw exports take positional byte slices; the JS wrapper (`api.js` in
//! the npm package) provides the `{ drawing, crlNative, tsuDescription }`
//! object-style API on top.

use serde::Serialize;
use wasm_bindgen::prelude::*;

use crate::{BlobKind, FreeformBlobs, FreeformDrawing, FreeformNative};

/// Serialize into plain JS objects: maps become `{}` records (not `Map`),
/// `None` becomes `undefined`.
fn to_js<T: Serialize>(value: &T) -> Result<JsValue, JsError> {
   let ser = serde_wasm_bindgen::Serializer::new().serialize_maps_as_objects(true);
   value
      .serialize(&ser)
      .map_err(|err| JsError::new(&err.to_string()))
}

const fn kind_name(kind: BlobKind) -> &'static str {
   match kind {
      BlobKind::Drawing => "drawing",
      BlobKind::CrlNative => "crlNative",
      BlobKind::TsuDescription => "tsuDescription",
      BlobKind::RenderPng => "renderPng",
   }
}

/// Identify which Freeform pasteboard flavor a dropped/loaded file is, from
/// its dump-tool filename and content signature. Returns `undefined` for
/// unrelated files.
#[wasm_bindgen(js_name = classifyBlob)]
pub fn classify_blob(name: &str, bytes: &[u8]) -> Option<String> {
   crate::classify_blob(name, bytes).map(|kind| kind_name(kind).to_owned())
}

/// True when `bytes` starts with the `PKDrawing` `"wrd"` magic.
#[wasm_bindgen(js_name = isPkDrawing)]
pub fn is_pk_drawing(bytes: &[u8]) -> bool {
   crate::is_pk_drawing(bytes)
}

/// Decode a `com.apple.drawing` `PKDrawing` blob into normalized strokes.
/// Throws when the header/version is unrecognized.
#[wasm_bindgen(js_name = decodePkDrawing)]
pub fn decode_pk_drawing(bytes: &[u8]) -> Result<JsValue, JsError> {
   let drawing = crate::decode_pk_drawing(bytes).map_err(|err| JsError::new(&err.to_string()))?;
   to_js(&drawing)
}

/// Decode `com.apple.freeform.CRLNativeData`, optionally joined with the
/// `TSUDescription` blob to attach a class + hints to each board item.
/// Throws on a structurally invalid blob.
#[wasm_bindgen(js_name = decodeCrlNative)]
pub fn decode_crl_native(
   bytes: &[u8],
   tsu_description: Option<Box<[u8]>>,
) -> Result<JsValue, JsError> {
   let native = crate::decode_crl_native(bytes, tsu_description.as_deref())
      .map_err(|err| JsError::new(&err.to_string()))?;
   to_js(&native)
}

/// Parse the `TSUDescription` manifest into per-item classes + routing hints.
/// Throws on an unparseable bplist.
#[wasm_bindgen(js_name = parseTsuDescription)]
pub fn parse_tsu_description(bytes: &[u8]) -> Result<JsValue, JsError> {
   let entries =
      crate::parse_tsu_description(bytes).map_err(|err| JsError::new(&err.to_string()))?;
   to_js(&entries)
}

/// Camel-case view of the decodable tiers of `FreeformPasteboard`; the
/// `renderPng` passthrough is reattached on the JS side without a wasm
/// round-trip.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PasteboardOut {
   #[serde(skip_serializing_if = "Option::is_none")]
   drawing: Option<FreeformDrawing>,
   #[serde(skip_serializing_if = "Option::is_none")]
   native:  Option<FreeformNative>,
}

/// Decode whatever flavors are present into a single selection. Decode
/// failures (unsupported version, truncated Universal Clipboard transfer)
/// degrade to a missing tier rather than throwing.
#[wasm_bindgen(js_name = decodePasteboard)]
pub fn decode_pasteboard(
   drawing: Option<Box<[u8]>>,
   crl_native: Option<Box<[u8]>>,
   tsu_description: Option<Box<[u8]>>,
) -> Result<JsValue, JsError> {
   let decoded = crate::decode_pasteboard(FreeformBlobs {
      drawing:         drawing.map(Into::into),
      crl_native:      crl_native.map(Into::into),
      tsu_description: tsu_description.map(Into::into),
      render_png:      None,
   });
   to_js(&PasteboardOut { drawing: decoded.drawing, native: decoded.native })
}
