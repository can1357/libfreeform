//! WebAssembly bindings for [`libfreeform`], published to npm as `libfreeform`.
//!
//! Decoded values cross the boundary as plain JavaScript objects with camelCase
//! keys. The npm package owns inexpensive signature checks and the object-style
//! pasteboard API; this module only receives bytes that actually need decoding.

use serde::Serialize;
use wasm_bindgen::prelude::*;

/// Serialize into plain JS objects: maps become `{}` records (not `Map`) and
/// optional Rust fields become `undefined`.
fn to_js<T: Serialize>(value: &T) -> Result<JsValue, JsValue> {
   let serializer = serde_wasm_bindgen::Serializer::new().serialize_maps_as_objects(true);
   value
      .serialize(&serializer)
      .map_err(|error| JsError::new(&error.to_string()).into())
}

/// Serialize a direct decoder failure without losing its category.
fn decode_error(error: crate::FreeformDecodeError) -> JsValue {
   to_js(&error).unwrap_or_else(|serialization_error| serialization_error)
}

/// Decode a `com.apple.drawing` PKDrawing blob into normalized strokes.
///
/// Throws a `{ kind, message }`
/// [`FreeformDecodeError`](crate::FreeformDecodeError) object when the blob is
/// invalid or unsupported.
#[wasm_bindgen(js_name = decodePkDrawing)]
pub fn decode_pk_drawing(bytes: &[u8]) -> Result<JsValue, JsValue> {
   let drawing = crate::decode_pk_drawing(bytes).map_err(decode_error)?;
   to_js(&drawing)
}

/// Decode `com.apple.freeform.CRLNativeData`, optionally joined with a
/// `TSUDescription` blob to attach class and routing hints to board items.
///
/// Throws a `{ kind, message }`
/// [`FreeformDecodeError`](crate::FreeformDecodeError) object when the archive
/// cannot be decoded.
#[wasm_bindgen(js_name = decodeCrlNative)]
pub fn decode_crl_native(
   bytes: &[u8],
   tsu_description: Option<Box<[u8]>>,
) -> Result<JsValue, JsValue> {
   let native = crate::decode_crl_native(bytes).map_err(decode_error)?;
   let native = match tsu_description {
      Some(bytes) => {
         let entries = crate::parse_tsu_description(&bytes).map_err(decode_error)?;
         crate::join_crl_tsu(native, &entries).map_err(decode_error)?
      },
      None => native,
   };
   to_js(&native)
}

/// Parse the `TSUDescription` manifest into class names and recursive routing
/// hints.
///
/// Throws a `{ kind, message }`
/// [`FreeformDecodeError`](crate::FreeformDecodeError) object when the plist is
/// invalid.
#[wasm_bindgen(js_name = parseTsuDescription)]
pub fn parse_tsu_description(bytes: &[u8]) -> Result<JsValue, JsValue> {
   let entries = crate::parse_tsu_description(bytes).map_err(decode_error)?;
   to_js(&entries)
}

/// Decode an exact ordered pasteboard snapshot. Each supplied flavor remains in
/// the result even when one independently decoded tier fails.
///
/// `blobs` has the serde camelCase shape of
/// [`FreeformBlobs`](crate::FreeformBlobs): `{ changeCount?, flavors: [{ uti,
/// bytes }] }`.
#[wasm_bindgen(js_name = decodePasteboard)]
pub fn decode_pasteboard(blobs: JsValue) -> Result<JsValue, JsValue> {
   let blobs = serde_wasm_bindgen::from_value(blobs)
      .map_err(|error| decode_error(crate::FreeformDecodeError::invalid(error.to_string())))?;
   to_js(&crate::decode_pasteboard(blobs))
}
