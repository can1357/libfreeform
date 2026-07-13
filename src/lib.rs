//! Parser for Apple **Freeform** pasteboard data.
//!
//! A single copy contains several parallel flavors: `PencilKit` ink, the native
//! object graph, routing metadata, assets, state flags, text selections, and
//! rendered fallbacks. [`decode_pasteboard`] preserves every supplied flavor,
//! decodes each tier independently, and reports failures without discarding
//! tiers that remain usable.
//!
//! The private CRL formats are reverse-engineered and OS-version fragile.
//! Unsupported records retain their original bytes; truncated, corrupt, and
//! hostile inputs return structured errors instead of fabricated values.
//!
//! # Example
//!
//! ```no_run
//! use libfreeform::{FreeformBlobs, FreeformFlavor, FreeformTier, decode_pasteboard};
//!
//! let flavors = [
//!    ("com.apple.drawing", "com_apple_drawing"),
//!    ("com.apple.freeform.CRLNativeData", "com_apple_freeform_CRLNativeData"),
//!    ("com.apple.freeform.TSUDescription", "com_apple_freeform_TSUDescription"),
//! ]
//! .into_iter()
//! .filter_map(|(uti, path)| {
//!    std::fs::read(path)
//!       .ok()
//!       .map(|bytes| FreeformFlavor { uti: uti.into(), bytes })
//! })
//! .collect();
//! let decoded = decode_pasteboard(FreeformBlobs { change_count: None, flavors });
//! if let FreeformTier::Decoded(drawing) = decoded.drawing {
//!    for stroke in drawing.strokes {
//!       println!("{} stroke, {} controls", stroke.ink_type, stroke.points.len());
//!    }
//! }
//! ```

pub mod bplist;
pub mod crl;
pub mod decode;
pub mod pkdrawing;
pub mod types;

pub use crl::{decode_crl_native, join_crl_tsu, parse_tsu_description};
pub use decode::{BlobKind, classify_blob, decode_pasteboard, has_freeform_content};
pub use pkdrawing::{PK_POINT_STRIDE, decode_pk_drawing, is_pk_drawing};
pub use types::*;

// The npm package builds this exact crate with `--features wasm` for
// `wasm32-unknown-unknown`. Native users never compile the binding layer.
#[cfg(all(target_arch = "wasm32", feature = "wasm"))]
#[path = "../libfreeform-wasm/lib.rs"]
mod wasm;
