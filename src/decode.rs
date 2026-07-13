//! Pasteboard-level entry points: identify Freeform flavors and decode
//! whatever is present into a single `FreeformPasteboard`.

use super::{
   crl::decode_crl_native,
   pkdrawing::{decode_pk_drawing, is_pk_drawing},
   types::{FreeformBlobs, FreeformPasteboard},
};

/// Which Freeform pasteboard flavor a blob is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobKind {
   /// `com.apple.drawing` — flattened freehand ink (`PKDrawing`).
   Drawing,
   /// `com.apple.freeform.CRLNativeData` — the native object graph.
   CrlNative,
   /// `com.apple.freeform.TSUDescription` — the board-item class manifest.
   TsuDescription,
   /// `public.png` / `Apple PNG pasteboard type` — full-board render fallback.
   RenderPng,
}

const BPLIST_MAGIC: &[u8] = b"bplist";

/// Identify which Freeform pasteboard flavor a dropped/loaded file is, from
/// its dump-tool filename and content signature. `None` for unrelated files.
pub fn classify_blob(name: &str, bytes: &[u8]) -> Option<BlobKind> {
   let lower = name.to_lowercase();
   // dump-tool flavor names ("com_apple_freeform_CRLNativeData") and plain
   // `.crlnative`-style extensions both classify
   if lower.contains("crlnative") {
      return Some(BlobKind::CrlNative);
   }
   if lower.contains("tsudescription") {
      return Some(BlobKind::TsuDescription);
   }
   if lower.contains("public_png") || lower.contains("apple_png_pasteboard_type") {
      return Some(BlobKind::RenderPng);
   }
   if lower.contains("drawing") || is_pk_drawing(bytes) {
      return Some(BlobKind::Drawing);
   }
   if bytes.len() >= BPLIST_MAGIC.len() && &bytes[..BPLIST_MAGIC.len()] == BPLIST_MAGIC {
      return Some(BlobKind::TsuDescription);
   }
   None
}

/// Decode whatever flavors are present into a single selection.
///
/// Decode failures (unsupported version, truncated Universal Clipboard
/// transfer) degrade to a missing tier rather than failing, per the format's
/// best-effort contract.
pub fn decode_pasteboard(blobs: FreeformBlobs) -> FreeformPasteboard {
   let drawing = blobs
      .drawing
      .as_deref()
      .filter(|bytes| is_pk_drawing(bytes))
      .and_then(|bytes| decode_pk_drawing(bytes).ok());
   let native = blobs
      .crl_native
      .as_deref()
      .and_then(|bytes| decode_crl_native(bytes, blobs.tsu_description.as_deref()).ok());
   FreeformPasteboard { drawing, native, render_png: blobs.render_png }
}

/// True when at least one flavor present in `blobs` could carry Freeform
/// content.
pub fn has_freeform_content(blobs: &FreeformBlobs) -> bool {
   blobs.drawing.as_deref().is_some_and(is_pk_drawing) || blobs.crl_native.is_some()
}

#[cfg(test)]
mod tests {
   use super::*;

   const DRAWING: &[u8] = include_bytes!("../fixtures/ink-pen.drawing");
   const NATIVE: &[u8] = include_bytes!("../fixtures/native-mixed.crlnative");
   const TSU: &[u8] = include_bytes!("../fixtures/native-mixed.tsudescription");

   #[test]
   fn classifies_by_dump_tool_filename() {
      assert_eq!(
         classify_blob("com_apple_freeform_CRLNativeData.bin", &[]),
         Some(BlobKind::CrlNative)
      );
      assert_eq!(
         classify_blob("com_apple_freeform_TSUDescription.bin", &[]),
         Some(BlobKind::TsuDescription)
      );
      assert_eq!(classify_blob("public_png.bin", &[]), Some(BlobKind::RenderPng));
      assert_eq!(classify_blob("Apple_PNG_pasteboard_type.bin", &[]), Some(BlobKind::RenderPng));
      assert_eq!(classify_blob("com_apple_drawing.bin", &[]), Some(BlobKind::Drawing));
   }

   #[test]
   fn classifies_by_content_signature() {
      // "wrd" magic wins for arbitrary names.
      assert_eq!(classify_blob("blob.bin", DRAWING), Some(BlobKind::Drawing));
      // A bare bplist is assumed to be the TSUDescription manifest.
      assert_eq!(classify_blob("blob.bin", TSU), Some(BlobKind::TsuDescription));
      assert_eq!(classify_blob("notes.txt", b"hello"), None);
   }

   #[test]
   fn decodes_present_flavors_into_one_pasteboard() {
      let decoded = decode_pasteboard(FreeformBlobs {
         drawing:         Some(DRAWING.to_vec()),
         crl_native:      Some(NATIVE.to_vec()),
         tsu_description: Some(TSU.to_vec()),
         render_png:      Some(vec![1, 2, 3]),
      });
      assert_eq!(decoded.drawing.unwrap().strokes.len(), 2);
      let native = decoded.native.unwrap();
      assert_eq!(native.items.len(), 3);
      assert_eq!(native.items[0].class_name, "CRLWPStickyNoteItem");
      assert_eq!(decoded.render_png, Some(vec![1, 2, 3]));
   }

   #[test]
   fn decode_failures_degrade_to_a_missing_tier() {
      let decoded = decode_pasteboard(FreeformBlobs {
         drawing:         Some(b"wrd\xff\xff\xff\xff".to_vec()), // bad protobuf body
         crl_native:      Some(b"garbage".to_vec()),
         tsu_description: None,
         render_png:      None,
      });
      assert!(decoded.drawing.is_none());
      assert!(decoded.native.is_none());
      assert!(decoded.render_png.is_none());

      // A non-PKDrawing blob in the drawing slot is skipped, not decoded.
      let decoded = decode_pasteboard(FreeformBlobs {
         drawing: Some(TSU.to_vec()),
         ..FreeformBlobs::default()
      });
      assert!(decoded.drawing.is_none());
   }

   #[test]
   fn has_freeform_content_requires_a_carrying_flavor() {
      assert!(!has_freeform_content(&FreeformBlobs::default()));
      assert!(has_freeform_content(&FreeformBlobs {
         drawing: Some(DRAWING.to_vec()),
         ..FreeformBlobs::default()
      }));
      // A drawing slot without the "wrd" magic does not count...
      assert!(!has_freeform_content(&FreeformBlobs {
         drawing: Some(TSU.to_vec()),
         ..FreeformBlobs::default()
      }));
      // ...but any CRLNativeData does, even before decoding.
      assert!(has_freeform_content(&FreeformBlobs {
         crl_native: Some(vec![0]),
         ..FreeformBlobs::default()
      }));
   }
}
