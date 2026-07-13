//! Parser for Apple **Freeform** pasteboard data.
//!
//! When you copy board items in Freeform, macOS/iPadOS puts the same selection
//! on the pasteboard in several parallel flavors. This crate decodes the three
//! that carry structure — no Apple frameworks required, pure Rust:
//!
//! - `com.apple.drawing` — a `PencilKit` `PKDrawing` with every freehand stroke
//!   (per-point position, width, and pressure): [`decode_pk_drawing`].
//! - `com.apple.freeform.CRLNativeData` — the native object graph: board-item
//!   frames, fills, text, outlines, and embedded ink: [`decode_crl_native`].
//! - `com.apple.freeform.TSUDescription` — the board-item class manifest:
//!   [`parse_tsu_description`].
//!
//! [`decode_pasteboard`] assembles whatever flavors are present into one
//! [`FreeformPasteboard`]; [`classify_blob`] routes captured files to their
//! flavor. The formats are reverse-engineered (see `docs/FORMAT.md` in the
//! repository) and OS-version fragile, so decoding is best-effort by design:
//! a damaged or unsupported tier degrades to a missing tier, and no input —
//! truncated, corrupt, or hostile — panics.
//!
//! # Example
//!
//! ```no_run
//! use libfreeform::{FreeformBlobs, decode_pasteboard};
//!
//! let blobs = FreeformBlobs {
//!    drawing:         std::fs::read("com_apple_drawing").ok(),
//!    crl_native:      std::fs::read("com_apple_freeform_CRLNativeData").ok(),
//!    tsu_description: std::fs::read("com_apple_freeform_TSUDescription").ok(),
//!    render_png:      None,
//! };
//! let decoded = decode_pasteboard(blobs);
//! for stroke in decoded.drawing.iter().flat_map(|d| &d.strokes) {
//!    println!("{} stroke, {} points", stroke.ink_type, stroke.points.len());
//! }
//! for item in decoded.native.iter().flat_map(|n| &n.items) {
//!    println!("{}: {:?} {:?}", item.class_name, item.frame, item.text);
//! }
//! ```

pub mod bplist;
pub mod crl;
pub mod decode;
pub mod pkdrawing;
pub mod types;

pub use crl::{
   Sections, TsuEntry, decode_crl_native, manifest_item_count, parse_tsu_description,
   split_sections, srgb_colors, tswp_strings,
};
pub use decode::{BlobKind, classify_blob, decode_pasteboard, has_freeform_content};
pub use pkdrawing::{PK_POINT_STRIDE, decode_pk_drawing, is_pk_drawing};
pub use types::*;

// The npm package builds this exact crate with `--features wasm` for
// `wasm32-unknown-unknown`. Native users never compile the binding layer.
#[cfg(all(target_arch = "wasm32", feature = "wasm"))]
#[path = "../libfreeform-wasm/lib.rs"]
mod wasm;
