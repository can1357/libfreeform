//! Lossless pasteboard assembly for Freeform selections.
//!
//! This module deliberately treats every pasteboard flavor as an independent
//! transport.  A damaged native graph must not discard a valid render, and a
//! stale `TSUDescription` must not be allowed to annotate unrelated native
//! records.

use std::{borrow::Cow, collections::BTreeMap};

use super::{
   crl::{decode_crl_native, join_crl_tsu, parse_tsu_description},
   pkdrawing::{decode_pk_drawing, is_pk_drawing},
   types::{
      FreeformAsset, FreeformBlobs, FreeformDecodeError, FreeformDiagnostic, FreeformFlavor,
      FreeformItemKind, FreeformNativeMetadata, FreeformPasteboard, FreeformRender, FreeformTier,
   },
   wire::{Fields, Value},
};

/// Which recognized Freeform pasteboard flavor a blob carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobKind {
   /// `com.apple.drawing` — flattened freehand ink (`PKDrawing`).
   Drawing,
   /// `com.apple.freeform.CRLNativeData` — the native object graph.
   CrlNative,
   /// `com.apple.freeform.CRLNativeMetadata` — paste correlation metadata.
   CrlNativeMetadata,
   /// `com.apple.freeform.TSUDescription` — the board-item class manifest.
   TsuDescription,
   /// `com.apple.freeform.stylepasteboard` — a style-only transfer.
   Style,
   /// `com.apple.freeform.pasteboardState.*` — an exact pasteboard state value.
   State,
   /// `com.apple.freeform.CRLAsset.<id>` — a dynamic embedded-media flavor.
   Asset,
   /// `public.png` or Apple’s PNG pasteboard type.
   RenderPng,
   /// `public.tiff` or `NeXT` TIFF pasteboard type.
   RenderTiff,
   /// `com.adobe.pdf` / `public.pdf` or Apple’s PDF pasteboard type.
   RenderPdf,
   /// `public.utf8-plain-text`.
   PlainText,
   /// `public.rtf`.
   RichText,
}

const PNG_MAGIC: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const PDF_MAGIC: &[u8; 5] = b"%PDF-";

/// Identify a known Freeform flavor from its exact UTI or dump-tool alias.
///
/// The aliases accepted here replace UTI punctuation with underscores, as the
/// bundled pasteboard dumper does.  This is intentionally not a substring
/// classifier: for example, a state whose key contains `CRLNativeData` is
/// still state.  Render flavors additionally require their container magic,
/// and an unnamed binary plist is never assumed to be a TSU manifest.
pub fn classify_blob(name: &str, bytes: &[u8]) -> Option<BlobKind> {
   let name = dump_name(name);
   let normalized = normalize_uti(name);
   if let Some(kind) = native_chunk_kind(name) {
      return Some(kind);
   }

   let kind = match normalized.as_str() {
      "comappledrawing" => BlobKind::Drawing,
      "comapplefreeformcrlnativedata" => BlobKind::CrlNative,
      "comapplefreeformcrlnativemetadata" => BlobKind::CrlNativeMetadata,
      "comapplefreeformtsudescription" => BlobKind::TsuDescription,
      "comapplefreeformstylepasteboard" => BlobKind::Style,
      "publicutf8plaintext" => BlobKind::PlainText,
      "publicrtf" => BlobKind::RichText,
      "publicpng" | "applepngpasteboardtype" if is_png(bytes) => BlobKind::RenderPng,
      "publictiff" | "nexttiffv40pasteboardtype" if is_tiff(bytes) => BlobKind::RenderTiff,
      "publicpdf" | "comadobepdf" | "applepdfpasteboardtype" if is_pdf(bytes) => {
         BlobKind::RenderPdf
      },
      _ if is_state_flavor(name) => BlobKind::State,
      _ if asset_id(name).is_some() => BlobKind::Asset,
      _ if is_pk_drawing(bytes) => BlobKind::Drawing,
      _ => return None,
   };
   Some(kind)
}

/// Decode all supplied flavors without allowing one tier to mask another.
///
/// Every recognized tier is attempted independently.  A decoded manifest is
/// joined to a decoded native graph only when the CRL decoder proves their
/// item cardinalities agree.  Failed joins retain both independent tiers and
/// become diagnostics rather than erasing valid data.
pub fn decode_pasteboard(blobs: FreeformBlobs) -> FreeformPasteboard {
   let mut pasteboard = FreeformPasteboard::default();
   let mut drawing = None;
   let mut native = None;
   let mut manifest = None;
   let mut metadata = None;

   let (transports, transport_errors) = assemble_transports(&blobs.flavors, Transport::Native);
   for flavor in &transports {
      let flavor = flavor.as_ref();
      match classify_blob(&flavor.uti, &flavor.bytes) {
         Some(BlobKind::Drawing) if drawing.is_none() => drawing = Some(flavor),
         Some(BlobKind::CrlNative) if native.is_none() => native = Some(flavor),
         Some(BlobKind::TsuDescription) if manifest.is_none() => manifest = Some(flavor),
         Some(BlobKind::CrlNativeMetadata) if metadata.is_none() => metadata = Some(flavor),
         Some(BlobKind::RenderPng | BlobKind::RenderTiff | BlobKind::RenderPdf) => {
            pasteboard
               .renders
               .push(FreeformRender { uti: flavor.uti.clone(), bytes: flavor.bytes.clone() });
         },
         Some(BlobKind::Style) => pasteboard.styles.push(flavor.clone()),
         Some(BlobKind::State) => {
            pasteboard
               .state
               .insert(flavor.uti.clone(), flavor.bytes.clone());
         },
         Some(BlobKind::Asset) => {},
         Some(
            kind @ (BlobKind::Drawing
            | BlobKind::CrlNative
            | BlobKind::TsuDescription
            | BlobKind::CrlNativeMetadata),
         ) => {
            pasteboard.unknown_flavors.push(flavor.clone());
            pasteboard.diagnostics.push(FreeformDiagnostic {
               source:  flavor.uti.clone(),
               message: format!("duplicate {kind:?} flavor retained as unknown"),
            });
         },
         Some(BlobKind::PlainText | BlobKind::RichText) => pasteboard.text.push(flavor.clone()),
         None => {
            if named_invalid_render(&flavor.uti) {
               pasteboard.diagnostics.push(FreeformDiagnostic {
                  source:  flavor.uti.clone(),
                  message: "render flavor has an invalid container signature".into(),
               });
            }
            pasteboard.unknown_flavors.push(flavor.clone());
         },
      }
   }

   pasteboard.drawing = decode_drawing(drawing);
   pasteboard.native = decode_native(native);
   pasteboard.manifest = decode_manifest(manifest);
   pasteboard.metadata = decode_metadata_tier(metadata);
   for (kind, source, error) in transport_errors {
      match kind {
         BlobKind::CrlNative => pasteboard.native = FreeformTier::Failed(error.clone()),
         BlobKind::CrlNativeMetadata => pasteboard.metadata = FreeformTier::Failed(error.clone()),
         _ => {},
      }
      pasteboard
         .diagnostics
         .push(FreeformDiagnostic { source, message: error.message });
   }

   join_manifest(&mut pasteboard);
   let descriptors = match &pasteboard.native {
      FreeformTier::Decoded(native) => Some(&native.assets),
      _ => None,
   };
   let (assets, errors) = assemble_transports(&blobs.flavors, Transport::Assets { descriptors });
   let mut asset_flavors = Vec::new();
   for asset in &assets {
      if asset_id(&asset.uti).is_some() {
         asset_flavors.push(asset.as_ref());
      }
   }
   for (_, source, error) in errors {
      for flavor in &blobs.flavors {
         if asset_id(&flavor.uti).is_some_and(|id| {
            id == source
               || id
                  .strip_prefix(&source)
                  .is_some_and(|tail| tail.starts_with(['.', '_']))
         }) {
            pasteboard.unknown_flavors.push(flavor.clone());
         }
      }
      pasteboard
         .diagnostics
         .push(FreeformDiagnostic { source, message: error.message });
   }
   merge_asset_flavors(&mut pasteboard, asset_flavors);
   correlate_metadata(&mut pasteboard);
   prefer_pk_drawing(&mut pasteboard);
   pasteboard.flavors = blobs.flavors;
   pasteboard
}

/// Return whether the capture contains independently valid Freeform content.
///
/// A valid `PKDrawing`, native graph, TSU manifest, or signature-validated
/// render counts.  Generic rich/plain text and arbitrary binary plists do not:
/// neither proves that the producing application was Freeform.
pub fn has_freeform_content(blobs: &FreeformBlobs) -> bool {
   let (flavors, errors) = assemble_transports(&blobs.flavors, Transport::Native);
   flavors
      .iter()
      .any(|flavor| match classify_blob(&flavor.uti, &flavor.bytes) {
         Some(BlobKind::Drawing) => decode_pk_drawing(&flavor.bytes).is_ok(),
         Some(BlobKind::CrlNative) => {
            !errors.iter().any(|(kind, ..)| *kind == BlobKind::CrlNative)
               && decode_crl_native(&flavor.bytes).is_ok()
         },
         Some(BlobKind::TsuDescription) => {
            has_bplist_header(&flavor.bytes) && parse_tsu_description(&flavor.bytes).is_ok()
         },
         Some(BlobKind::RenderPng | BlobKind::RenderTiff | BlobKind::RenderPdf) => true,
         _ => false,
      })
}

type TransportFailure = (BlobKind, String, FreeformDecodeError);
type AssembledTransports<'a> = (Vec<Cow<'a, FreeformFlavor>>, Vec<TransportFailure>);

enum Transport<'a> {
   Native,
   Assets { descriptors: Option<&'a BTreeMap<String, FreeformAsset>> },
}

/// Reassemble only native numbered transports. The suffix is a canonical,
/// positive decimal integer; neither lexical order nor input order is
/// meaningful. Asset IDs themselves may contain numbers, so descriptors must
/// disambiguate a trailing numeric component from a continuation index.
fn assemble_transports<'a>(
   flavors: &'a [FreeformFlavor],
   transport: Transport<'_>,
) -> AssembledTransports<'a> {
   let (assets, descriptors) = match transport {
      Transport::Native => (false, None),
      Transport::Assets { descriptors } => (true, descriptors),
   };
   let mut groups: BTreeMap<String, Vec<(Option<usize>, &'a FreeformFlavor)>> = BTreeMap::new();
   let mut output = Vec::new();
   for flavor in flavors {
      let name = dump_name(&flavor.uti);
      let mut target = None;
      if assets {
         if let Some(id) = asset_id(name) {
            let separator = if name
               .get(..4)
               .is_some_and(|prefix| prefix.eq_ignore_ascii_case("com_"))
            {
               '_'
            } else {
               '.'
            };
            let suffix = id
               .rsplit_once(separator)
               .filter(|(_, tail)| !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()));
            if let Some((base, tail)) = suffix {
               let exact = descriptors.is_some_and(|map| map.contains_key(id));
               let parent = descriptors.is_some_and(|map| map.contains_key(base));
               if exact && !parent {
                  target = Some((id.to_owned(), Some(0)));
               } else {
                  // Without a descriptor, even an apparent base flavor cannot
                  // prove that "id.1" is not an independent asset identifier.
                  target = Some((
                     base.to_owned(),
                     if parent && !exact {
                        chunk_index(tail)
                     } else {
                        None
                     },
                  ));
               }
            } else {
               target = Some((id.to_owned(), Some(0)));
            }
         }
      } else {
         for (uti, alias) in [
            ("com.apple.freeform.CRLNativeData", "com_apple_freeform_CRLNativeData"),
            ("com.apple.freeform.CRLNativeMetadata", "com_apple_freeform_CRLNativeMetadata"),
         ] {
            for (base, separator) in [(uti, '.'), (alias, '_')] {
               if name.eq_ignore_ascii_case(base) {
                  target = Some((uti.to_owned(), Some(0)));
               } else if name.len() > base.len()
                  && name
                     .get(..base.len())
                     .is_some_and(|prefix| prefix.eq_ignore_ascii_case(base))
                  && name.as_bytes()[base.len()] == separator as u8
               {
                  let tail = &name[base.len() + 1..];
                  if !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()) {
                     target = Some((uti.to_owned(), chunk_index(tail)));
                  }
               }
            }
         }
      }
      if let Some((key, index)) = target {
         groups.entry(key).or_default().push((index, flavor));
      } else if !assets {
         output.push(Cow::Borrowed(flavor));
      }
   }
   let mut errors = Vec::new();
   for (key, mut chunks) in groups {
      let kind = if assets {
         BlobKind::Asset
      } else if key.ends_with("Metadata") {
         BlobKind::CrlNativeMetadata
      } else {
         BlobKind::CrlNative
      };
      chunks.sort_by_key(|(index, _)| *index);
      let ambiguous = chunks.iter().any(|(index, _)| index.is_none())
         || chunks.windows(2).any(|pair| pair[0].0 == pair[1].0);
      let missing = chunks
         .iter()
         .enumerate()
         .any(|(expected, (index, _))| *index != Some(expected));
      if ambiguous || missing {
         let message = format!(
            "{key}: {}",
            if ambiguous {
               "ambiguous or duplicate numbered transport"
            } else {
               "missing numbered transport chunk"
            }
         );
         let error = if ambiguous {
            FreeformDecodeError::invalid(message)
         } else {
            FreeformDecodeError::incomplete(message)
         };
         errors.push((kind, key, error));
         if !assets {
            output.extend(chunks.into_iter().map(|(_, flavor)| Cow::Borrowed(flavor)));
         }
         continue;
      }
      if chunks.len() == 1 {
         output.push(Cow::Borrowed(chunks[0].1));
      } else {
         let mut bytes =
            Vec::with_capacity(chunks.iter().map(|(_, flavor)| flavor.bytes.len()).sum());
         for (_, flavor) in &chunks {
            bytes.extend_from_slice(&flavor.bytes);
         }
         output.push(Cow::Owned(FreeformFlavor { uti: chunks[0].1.uti.clone(), bytes }));
      }
   }
   (output, errors)
}

fn native_chunk_kind(name: &str) -> Option<BlobKind> {
   for (prefix, separator, kind) in [
      ("com.apple.freeform.CRLNativeData", '.', BlobKind::CrlNative),
      ("com_apple_freeform_CRLNativeData", '_', BlobKind::CrlNative),
      ("com.apple.freeform.CRLNativeMetadata", '.', BlobKind::CrlNativeMetadata),
      ("com_apple_freeform_CRLNativeMetadata", '_', BlobKind::CrlNativeMetadata),
   ] {
      if name
         .get(..prefix.len())
         .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
         && name.as_bytes().get(prefix.len()) == Some(&(separator as u8))
         && chunk_index(&name[prefix.len() + 1..]).is_some()
      {
         return Some(kind);
      }
   }
   None
}

fn chunk_index(tail: &str) -> Option<usize> {
   if tail.starts_with('0') {
      return None;
   }
   tail.parse().ok().filter(|index| *index > 0)
}

fn decode_drawing(flavor: Option<&FreeformFlavor>) -> FreeformTier<super::types::FreeformDrawing> {
   let Some(flavor) = flavor else {
      return FreeformTier::Absent;
   };
   if !is_pk_drawing(&flavor.bytes) {
      return FreeformTier::Failed(FreeformDecodeError::invalid(
         "PKDrawing: expected wrd signature",
      ));
   }
   decode_pk_drawing(&flavor.bytes).map_or_else(FreeformTier::Failed, FreeformTier::Decoded)
}

fn decode_native(flavor: Option<&FreeformFlavor>) -> FreeformTier<super::types::FreeformNative> {
   let Some(flavor) = flavor else {
      return FreeformTier::Absent;
   };
   decode_crl_native(&flavor.bytes).map_or_else(FreeformTier::Failed, FreeformTier::Decoded)
}

fn decode_manifest(flavor: Option<&FreeformFlavor>) -> FreeformTier<Vec<super::types::TsuEntry>> {
   let Some(flavor) = flavor else {
      return FreeformTier::Absent;
   };
   if !has_bplist_header(&flavor.bytes) {
      return FreeformTier::Failed(FreeformDecodeError::invalid(
         "TSUDescription: expected binary-plist signature",
      ));
   }
   parse_tsu_description(&flavor.bytes).map_or_else(FreeformTier::Failed, FreeformTier::Decoded)
}

fn decode_metadata_tier(flavor: Option<&FreeformFlavor>) -> FreeformTier<FreeformNativeMetadata> {
   let Some(flavor) = flavor else {
      return FreeformTier::Absent;
   };
   decode_native_metadata(&flavor.bytes).map_or_else(FreeformTier::Failed, FreeformTier::Decoded)
}

fn join_manifest(pasteboard: &mut FreeformPasteboard) {
   let (FreeformTier::Decoded(native), FreeformTier::Decoded(manifest)) =
      (&mut pasteboard.native, &pasteboard.manifest)
   else {
      return;
   };

   match join_crl_tsu(native.clone(), manifest) {
      Ok(joined) => *native = joined,
      Err(error) => pasteboard.diagnostics.push(FreeformDiagnostic {
         source:  "CRLNativeData/TSUDescription".into(),
         message: format!("manifest was not joined: {}", error.message),
      }),
   }
}

fn correlate_metadata(pasteboard: &mut FreeformPasteboard) {
   let (FreeformTier::Decoded(native), FreeformTier::Decoded(metadata)) =
      (&pasteboard.native, &pasteboard.metadata)
   else {
      return;
   };
   let Some(metadata_id) = metadata.paste_id.as_deref() else {
      return;
   };
   if !native.paste_id.is_empty() && native.paste_id != metadata_id {
      pasteboard.diagnostics.push(FreeformDiagnostic {
         source:  "CRLNativeMetadata".into(),
         message: FreeformDecodeError::correlation(format!(
            "metadata paste ID {metadata_id} does not match native paste ID {}",
            native.paste_id
         ))
         .message,
      });
   }
}

fn prefer_pk_drawing(pasteboard: &mut FreeformPasteboard) {
   let (FreeformTier::Decoded(drawing), FreeformTier::Decoded(native)) =
      (&pasteboard.drawing, &pasteboard.native)
   else {
      return;
   };
   let native_strokes = native
      .items
      .iter()
      .filter_map(|item| match &item.kind {
         FreeformItemKind::Ink { strokes } => Some(strokes.len()),
         _ => None,
      })
      .sum::<usize>();
   if native_strokes != 0 && !drawing.strokes.is_empty() {
      // PKDrawing is the canonical public ink representation.  Native stroke
      // records remain intact because the formats offer no shared stroke ID:
      // equal counts alone cannot prove that two strokes are duplicates.
      pasteboard.diagnostics.push(FreeformDiagnostic {
         source:  "com.apple.drawing".into(),
         message: "PKDrawing is canonical; uncorrelated native ink was retained losslessly".into(),
      });
   }
}

fn merge_asset_flavors(pasteboard: &mut FreeformPasteboard, flavors: Vec<&FreeformFlavor>) {
   if let FreeformTier::Decoded(native) = &pasteboard.native {
      pasteboard.assets = native.assets.clone();
   }
   for flavor in flavors {
      add_asset(&mut pasteboard.assets, flavor, &mut pasteboard.diagnostics);
   }
}

fn add_asset(
   assets: &mut BTreeMap<String, FreeformAsset>,
   flavor: &FreeformFlavor,
   diagnostics: &mut Vec<FreeformDiagnostic>,
) {
   let Some(id) = asset_id(&flavor.uti) else {
      return;
   };
   let asset = assets
      .entry(id.to_owned())
      .or_insert_with(|| FreeformAsset {
         id:             id.to_owned(),
         filename:       None,
         uti:            None,
         bytes:          None,
         premium:        None,
         intrinsic_size: None,
         raw_descriptor: Vec::new(),
      });
   if asset.bytes.is_some() {
      diagnostics.push(FreeformDiagnostic {
         source:  flavor.uti.clone(),
         message: "duplicate asset flavor was retained only in the input capture".into(),
      });
      return;
   }
   if asset.uti.is_none() {
      asset.uti = Some(flavor.uti.clone());
   }
   asset.bytes = Some(flavor.bytes.clone());
}

fn decode_native_metadata(bytes: &[u8]) -> Result<FreeformNativeMetadata, FreeformDecodeError> {
   let length = bytes
      .get(..8)
      .and_then(|length| length.try_into().ok())
      .map(u64::from_le_bytes)
      .ok_or_else(|| FreeformDecodeError::incomplete("CRLNativeMetadata: missing length"))?;
   let length = usize::try_from(length)
      .map_err(|_| FreeformDecodeError::invalid("CRLNativeMetadata: length exceeds platform"))?;
   let end = 8usize
      .checked_add(length)
      .ok_or_else(|| FreeformDecodeError::invalid("CRLNativeMetadata: length overflow"))?;
   let payload = bytes
      .get(8..end)
      .ok_or_else(|| FreeformDecodeError::incomplete("CRLNativeMetadata: truncated payload"))?;
   if end != bytes.len() {
      return Err(FreeformDecodeError::invalid("CRLNativeMetadata: bytes follow declared payload"));
   }

   // Schema-free: any top-level 16-byte length-delimited field is a candidate
   // paste UUID.
   let mut ids = Vec::new();
   for field in Fields::new(payload) {
      if let Value::Bytes(value) = field?.value
         && value.len() == 16
      {
         let id = format_uuid(value);
         if !ids.contains(&id) {
            ids.push(id);
         }
      }
   }
   let paste_id = (ids.len() == 1).then(|| ids.remove(0));
   Ok(FreeformNativeMetadata { paste_id, raw_data: payload.to_vec() })
}

fn format_uuid(bytes: &[u8]) -> String {
   const HEX: &[u8; 16] = b"0123456789ABCDEF";
   let mut out = String::with_capacity(36);
   for (index, byte) in bytes.iter().copied().enumerate() {
      if matches!(index, 4 | 6 | 8 | 10) {
         out.push('-');
      }
      out.push(HEX[(byte >> 4) as usize] as char);
      out.push(HEX[(byte & 15) as usize] as char);
   }
   out
}

fn has_bplist_header(bytes: &[u8]) -> bool {
   bytes.len() >= 8
      && bytes.starts_with(b"bplist")
      && bytes[6].is_ascii_digit()
      && bytes[7].is_ascii_digit()
}

fn is_png(bytes: &[u8]) -> bool {
   bytes.starts_with(PNG_MAGIC)
}

fn is_tiff(bytes: &[u8]) -> bool {
   matches!(bytes.get(..4), Some(b"II\x2a\0" | b"MM\0\x2a" | b"II\x2b\0" | b"MM\0\x2b"))
}

fn is_pdf(bytes: &[u8]) -> bool {
   bytes.starts_with(PDF_MAGIC)
}

fn dump_name(name: &str) -> &str {
   let name = name.rsplit(['/', '\\']).next().unwrap_or(name).trim();
   let suffix = ".bin";
   if name.len() > suffix.len() && name[name.len() - suffix.len()..].eq_ignore_ascii_case(suffix) {
      return &name[..name.len() - suffix.len()];
   }
   name
}

fn normalize_uti(name: &str) -> String {
   name
      .bytes()
      .filter(u8::is_ascii_alphanumeric)
      .map(|byte| byte.to_ascii_lowercase() as char)
      .collect()
}

fn is_state_flavor(name: &str) -> bool {
   let lower = dump_name(name).to_ascii_lowercase();
   ["com.apple.freeform.pasteboardstate.", "com_apple_freeform_pasteboardstate_"]
      .into_iter()
      .any(|prefix| {
         lower
            .strip_prefix(prefix)
            .is_some_and(|key| !key.is_empty())
      })
}

fn asset_id(name: &str) -> Option<&str> {
   let name = dump_name(name);
   let lower = name.to_ascii_lowercase();
   let prefix = ["com.apple.freeform.crlasset.", "com_apple_freeform_crlasset_"]
      .into_iter()
      .find(|prefix| lower.starts_with(*prefix))?;
   let id = &name[prefix.len()..];
   (!id.is_empty()).then_some(id)
}

fn named_invalid_render(name: &str) -> bool {
   let normalized = normalize_uti(dump_name(name));
   matches!(
      normalized.as_str(),
      "publicpng"
         | "applepngpasteboardtype"
         | "publictiff"
         | "nexttiffv40pasteboardtype"
         | "publicpdf"
         | "comadobepdf"
         | "applepdfpasteboardtype"
   )
}

#[cfg(test)]
mod tests {
   use super::*;

   const DRAWING: &[u8] = include_bytes!("../fixtures/ink-pen.drawing");
   const NATIVE: &[u8] = include_bytes!("../fixtures/real-board.crlnative");
   const TSU: &[u8] = include_bytes!("../fixtures/real-board.tsudescription");

   fn flavor(uti: &str, bytes: &[u8]) -> FreeformFlavor {
      FreeformFlavor { uti: uti.into(), bytes: bytes.into() }
   }

   #[test]
   fn classifies_only_exact_utis_and_dump_aliases() {
      assert_eq!(
         classify_blob("com_apple_freeform_CRLNativeMetadata.bin", b""),
         Some(BlobKind::CrlNativeMetadata)
      );
      assert_eq!(
         classify_blob("com.apple.freeform.pasteboardState.CRLNativeData", b"x"),
         Some(BlobKind::State)
      );
      assert_eq!(classify_blob("blob.bin", b"bplist00"), None);
      assert_eq!(classify_blob("unrelated_CRLNativeData.bin", b""), None);
   }

   #[test]
   fn validates_literal_and_sanitized_render_signatures() {
      let png = b"\x89PNG\r\n\x1a\nbody";
      let tiff = b"II\x2a\0body";
      assert_eq!(classify_blob("public.png", png), Some(BlobKind::RenderPng));
      assert_eq!(classify_blob("Apple_PNG_pasteboard_type.bin", png), Some(BlobKind::RenderPng));
      assert_eq!(classify_blob("public.tiff", tiff), Some(BlobKind::RenderTiff));
      assert_eq!(
         classify_blob("NeXT_TIFF_v4.0_pasteboard_type.bin", tiff),
         Some(BlobKind::RenderTiff)
      );
      assert_eq!(classify_blob("public.png", b"not png"), None);
   }

   #[test]
   fn preserves_tsu_only_and_render_only_content() {
      let tsu_only = decode_pasteboard(FreeformBlobs {
         flavors: vec![flavor("com.apple.freeform.TSUDescription", TSU)],
         ..FreeformBlobs::default()
      });
      assert!(matches!(tsu_only.manifest, FreeformTier::Decoded(_)));
      assert!(has_freeform_content(&FreeformBlobs {
         flavors: vec![flavor("com.apple.freeform.TSUDescription", TSU)],
         ..FreeformBlobs::default()
      }));

      let render_only = decode_pasteboard(FreeformBlobs {
         flavors: vec![flavor("public.png", b"\x89PNG\r\n\x1a\nrender")],
         ..FreeformBlobs::default()
      });
      assert_eq!(render_only.renders.len(), 1);
      assert!(has_freeform_content(&FreeformBlobs {
         flavors: vec![flavor("public.png", b"\x89PNG\r\n\x1a\nrender")],
         ..FreeformBlobs::default()
      }));
   }

   #[test]
   fn native_failure_does_not_mask_manifest_or_unknown_flavors() {
      let decoded = decode_pasteboard(FreeformBlobs {
         flavors: vec![
            flavor("com.apple.freeform.CRLNativeData", b"bad"),
            flavor("com.apple.freeform.TSUDescription", TSU),
            flavor("org.example.unknown", b"retained"),
         ],
         ..FreeformBlobs::default()
      });
      assert!(matches!(decoded.native, FreeformTier::Failed(_)));
      assert!(matches!(decoded.manifest, FreeformTier::Decoded(_)));
      assert_eq!(decoded.unknown_flavors, vec![flavor("org.example.unknown", b"retained")]);
   }

   #[test]
   fn valid_native_survives_a_bad_tsu_tier() {
      let decoded = decode_pasteboard(FreeformBlobs {
         flavors: vec![
            flavor("com.apple.freeform.CRLNativeData", NATIVE),
            flavor("com.apple.freeform.TSUDescription", b"not a bplist"),
         ],
         ..FreeformBlobs::default()
      });
      assert!(matches!(decoded.native, FreeformTier::Decoded(_)));
      assert!(matches!(decoded.manifest, FreeformTier::Failed(_)));
   }

   #[test]
   fn stale_manifest_is_not_joined_to_native() {
      let native = decode_crl_native(NATIVE).expect("fixture native should decode");
      let mut manifest = parse_tsu_description(TSU).expect("fixture manifest should decode");
      manifest.pop();
      let item_count = native.items.len();
      let mut pasteboard = FreeformPasteboard {
         native: FreeformTier::Decoded(native),
         manifest: FreeformTier::Decoded(manifest),
         ..FreeformPasteboard::default()
      };
      join_manifest(&mut pasteboard);
      assert!(
         matches!(&pasteboard.native, FreeformTier::Decoded(native) if native.items.len() == item_count)
      );
      assert!(
         pasteboard
            .diagnostics
            .iter()
            .any(|d| d.message.contains("not joined"))
      );
   }

   #[test]
   fn metadata_and_assets_are_losslessly_assembled() {
      let mut metadata = Vec::new();
      metadata.extend_from_slice(&(18u64).to_le_bytes());
      metadata.extend_from_slice(&[0x0a, 16]);
      metadata.extend([0u8; 16]);
      let decoded = decode_pasteboard(FreeformBlobs {
         flavors: vec![
            flavor("com.apple.freeform.CRLNativeMetadata", &metadata),
            flavor("com.apple.freeform.CRLAsset.first", b"first"),
            flavor("com_apple_freeform_CRLAsset_second.bin", b"second"),
            flavor("com.apple.freeform.pasteboardState.hasPremiumContent", b"\x01"),
         ],
         ..FreeformBlobs::default()
      });
      let FreeformTier::Decoded(metadata) = decoded.metadata else {
         panic!("metadata should decode");
      };
      assert_eq!(metadata.paste_id.as_deref(), Some("00000000-0000-0000-0000-000000000000"));
      assert_eq!(decoded.assets.len(), 2);
      assert_eq!(decoded.assets["first"].bytes.as_deref(), Some(&b"first"[..]));
      assert_eq!(decoded.assets["second"].bytes.as_deref(), Some(&b"second"[..]));
      assert_eq!(decoded.state["com.apple.freeform.pasteboardState.hasPremiumContent"], b"\x01");
   }

   #[test]
   fn stale_metadata_reports_but_keeps_native() {
      let mut metadata = Vec::new();
      metadata.extend_from_slice(&(18u64).to_le_bytes());
      metadata.extend_from_slice(&[0x0a, 16]);
      metadata.extend([0xff; 16]);
      let decoded = decode_pasteboard(FreeformBlobs {
         flavors: vec![
            flavor("com.apple.freeform.CRLNativeData", NATIVE),
            flavor("com.apple.freeform.CRLNativeMetadata", &metadata),
         ],
         ..FreeformBlobs::default()
      });
      assert!(matches!(decoded.native, FreeformTier::Decoded(_)));
      assert!(matches!(decoded.metadata, FreeformTier::Decoded(_)));
      assert!(
         decoded
            .diagnostics
            .iter()
            .any(|d| d.message.contains("does not match"))
      );
   }

   #[test]
   fn unordered_native_chunks_reassemble_real_capture() {
      let mut flavors: Vec<_> = NATIVE
         .chunks(1024)
         .enumerate()
         .map(|(index, bytes)| {
            let name = if index == 0 {
               "com.apple.freeform.CRLNativeData".to_owned()
            } else {
               format!("com.apple.freeform.CRLNativeData.{index}")
            };
            flavor(&name, bytes)
         })
         .collect();
      flavors.reverse();
      let blobs = FreeformBlobs { flavors: flavors.clone(), ..FreeformBlobs::default() };
      assert!(has_freeform_content(&blobs));
      let decoded = decode_pasteboard(blobs);
      assert_eq!(decoded.flavors, flavors);
      assert_eq!(decoded.native, FreeformTier::Decoded(decode_crl_native(NATIVE).unwrap()));
      assert!(decoded.unknown_flavors.is_empty());
   }

   #[test]
   fn missing_and_duplicate_chunks_fail_only_native_transport() {
      for chunks in [
         vec![flavor("com.apple.freeform.CRLNativeData.2", b"tail")],
         vec![
            flavor("com.apple.freeform.CRLNativeData.1", b"a"),
            flavor("com_apple_freeform_CRLNativeData_1.bin", b"b"),
         ],
         vec![flavor("com.apple.freeform.CRLNativeData.01", b"a")],
      ] {
         let mut flavors = vec![
            flavor("com.apple.freeform.CRLNativeData", NATIVE),
            flavor("com.apple.freeform.TSUDescription", TSU),
            flavor("public.png", b"\x89PNG\r\n\x1a\nrender"),
         ];
         flavors.extend(chunks);
         let decoded =
            decode_pasteboard(FreeformBlobs { flavors: flavors.clone(), ..Default::default() });
         assert!(matches!(decoded.native, FreeformTier::Failed(_)));
         assert!(matches!(decoded.manifest, FreeformTier::Decoded(_)));
         assert_eq!(decoded.renders.len(), 1);
         assert_eq!(decoded.flavors, flavors);
      }
   }

   #[test]
   fn metadata_chunks_use_dump_aliases_and_fail_independently() {
      let metadata = 0u64.to_le_bytes();
      let decoded = decode_pasteboard(FreeformBlobs {
         flavors: vec![
            flavor("com_apple_freeform_CRLNativeMetadata_1.bin", &metadata[4..]),
            flavor("com_apple_freeform_CRLNativeMetadata.bin", &metadata[..4]),
            flavor("com.apple.freeform.CRLNativeData", b"bad"),
         ],
         ..Default::default()
      });
      assert!(matches!(decoded.metadata, FreeformTier::Decoded(_)));
      assert!(matches!(decoded.native, FreeformTier::Failed(_)));
   }

   #[test]
   fn asset_chunks_require_exact_descriptor_identity() {
      let descriptor = FreeformAsset {
         id:             "123".into(),
         filename:       Some("image.png".into()),
         uti:            Some("public.png".into()),
         bytes:          None,
         premium:        None,
         intrinsic_size: None,
         raw_descriptor: vec![42],
      };
      let mut descriptors = BTreeMap::from([("123".into(), descriptor.clone())]);
      let flavors = vec![
         flavor("com.apple.freeform.CRLAsset.123.1", b"second"),
         flavor("com.apple.freeform.CRLAsset.123", b"first"),
      ];
      let (joined, errors) =
         assemble_transports(&flavors, Transport::Assets { descriptors: Some(&descriptors) });
      assert!(errors.is_empty());
      assert_eq!(joined.len(), 1);
      let mut diagnostics = Vec::new();
      add_asset(&mut descriptors, &joined[0], &mut diagnostics);
      assert_eq!(descriptors["123"].bytes.as_deref(), Some(&b"firstsecond"[..]));
      assert_eq!(descriptors["123"].uti.as_deref(), Some("public.png"));
      assert_eq!(descriptors["123"].raw_descriptor, vec![42]);

      let (joined, errors) = assemble_transports(&flavors, Transport::Assets { descriptors: None });
      assert!(joined.is_empty());
      assert_eq!(errors.len(), 1);

      for bad in [
         vec![
            flavor("com.apple.freeform.CRLAsset.123", b"first"),
            flavor("com.apple.freeform.CRLAsset.123.2", b"third"),
         ],
         vec![
            flavor("com.apple.freeform.CRLAsset.123", b"first"),
            flavor("com.apple.freeform.CRLAsset.123.1", b"second"),
            flavor("com_apple_freeform_CRLAsset_123_1.bin", b"duplicate"),
         ],
      ] {
         let (joined, errors) =
            assemble_transports(&bad, Transport::Assets { descriptors: Some(&descriptors) });
         assert!(joined.is_empty());
         assert_eq!(errors.len(), 1);
      }

      descriptors.insert("123.1".into(), FreeformAsset { id: "123.1".into(), ..descriptor });
      let (joined, errors) =
         assemble_transports(&flavors, Transport::Assets { descriptors: Some(&descriptors) });
      assert!(joined.is_empty());
      assert_eq!(errors.len(), 1);
   }

   #[test]
   fn numeric_asset_ids_are_not_inferred_as_chunks() {
      let decoded = decode_pasteboard(FreeformBlobs {
         flavors: vec![flavor("com.apple.freeform.CRLAsset.123", b"numeric asset")],
         ..Default::default()
      });
      assert_eq!(decoded.assets["123"].bytes.as_deref(), Some(&b"numeric asset"[..]));
      let descriptor = FreeformAsset {
         id:             "123.1".into(),
         filename:       None,
         uti:            None,
         bytes:          None,
         premium:        None,
         intrinsic_size: None,
         raw_descriptor: Vec::new(),
      };
      let descriptors = BTreeMap::from([("123.1".into(), descriptor)]);
      let flavors = vec![flavor("com.apple.freeform.CRLAsset.123.1", b"exact asset")];
      let (joined, errors) =
         assemble_transports(&flavors, Transport::Assets { descriptors: Some(&descriptors) });
      assert!(errors.is_empty());
      assert_eq!(joined[0].bytes, b"exact asset");
   }

   #[test]
   fn drawing_signature_is_not_confused_with_tsu() {
      assert_eq!(classify_blob("blob", DRAWING), Some(BlobKind::Drawing));
      assert_eq!(classify_blob("blob", TSU), None);
   }
}
