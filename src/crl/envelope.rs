//! Strict framing and ownership recovery for a `CRLNativeData` archive.
//!
//! The outer envelope is stable enough to validate independently of the
//! undocumented item messages.  This module therefore only exposes an object
//! record when its CRDT metadata contains one of the manifest item UUIDs at
//! the verified `ObjectMetadata` position; it never associates data by byte
//! proximity.

use std::{
   collections::{BTreeSet, HashMap},
   ops::Range,
};

use crate::{
   bplist::{Plist, bounded_plist_length, parse_bplist},
   types::{FreeformCompatibility, FreeformDecodeError},
};

const BPLIST_HEADER: &[u8; 8] = b"bplist00";
const CRDT_TAG: &[u8; 4] = b"crdt";
const NIL_UUID: [u8; 16] = [0; 16];

/// A bounded CRDT object record whose owner was established by metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeRecord<'a> {
   /// Canonical uppercase UUID of the board item that owns this record.
   pub owner_id: String,
   /// Offset of `bytes` from the start of [`NativeArchive::archive`].
   pub offset:   usize,
   /// Exact bounded protobuf payload retained for the item decoder.
   pub bytes:    &'a [u8],
}

/// Lossless, validated outer regions of a CRL native archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeArchive<'a> {
   /// Exact manifest protobuf bytes, excluding its eight-byte length prefix.
   pub manifest:      &'a [u8],
   /// Exact bounded index plist bytes.
   pub raw_index:     &'a [u8],
   /// Byte range of `raw_index` in the original input.
   pub index_range:   Range<usize>,
   /// Exact remaining CRDT object archive bytes.
   pub archive:       &'a [u8],
   /// Canonical UUID from the index's required `id` field.
   pub paste_id:      String,
   /// Canonical, unique, ordered UUIDs from the index's required `boardItems`
   /// field.
   pub item_ids:      Vec<String>,
   /// Version status derived only from typed manifest fields.
   pub compatibility: FreeformCompatibility,
   /// Records for which one unambiguous item owner was structurally
   /// established.
   pub records:       Vec<NativeRecord<'a>>,
}

impl<'a> NativeArchive<'a> {
   /// Returns every bounded record owned by `owner_id` in archive order.
   pub(crate) fn records<'b>(
      &'b self,
      owner_id: &'b str,
   ) -> impl Iterator<Item = &'b NativeRecord<'a>> + 'b {
      self
         .records
         .iter()
         .filter(move |record| record.owner_id == owner_id)
   }
}

#[derive(Debug, Clone, Copy)]
enum WireValue<'a> {
   Varint(u64),
   Bytes(&'a [u8]),
   Fixed64,
   Fixed32,
}

#[derive(Debug, Clone, Copy)]
struct Field<'a> {
   number: u64,
   value:  WireValue<'a>,
}

fn invalid(message: impl Into<String>) -> FreeformDecodeError {
   FreeformDecodeError::invalid(format!("CRLNativeData: {}", message.into()))
}

fn incomplete(message: impl Into<String>) -> FreeformDecodeError {
   FreeformDecodeError::incomplete(format!("CRLNativeData: {}", message.into()))
}

fn read_u64_le(bytes: &[u8], offset: usize) -> Option<u64> {
   Some(u64::from_le_bytes(bytes.get(offset..offset.checked_add(8)?)?.try_into().ok()?))
}

fn read_varint(bytes: &[u8], mut offset: usize) -> Result<(u64, usize), ()> {
   let mut value = 0_u64;
   for shift in 0..10 {
      let byte = *bytes.get(offset).ok_or(())?;
      offset += 1;
      if shift == 9 && byte > 1 {
         return Err(());
      }
      value |= u64::from(byte & 0x7f) << (shift * 7);
      if byte & 0x80 == 0 {
         return Ok((value, offset));
      }
   }
   Err(())
}

fn parse_fields(bytes: &[u8]) -> Result<Vec<Field<'_>>, ()> {
   let (fields, consumed) = parse_field_prefix(bytes)?;
   if consumed == bytes.len() {
      Ok(fields)
   } else {
      Err(())
   }
}

/// Parses the maximal valid protobuf field prefix.  CRDT payloads are followed
/// by another heterogeneous frame, so their protobuf message has no outer
/// length; the caller validates the remaining frame before using this prefix.
fn parse_field_prefix(bytes: &[u8]) -> Result<(Vec<Field<'_>>, usize), ()> {
   let mut fields = Vec::new();
   let mut offset = 0;
   while offset < bytes.len() {
      let start = offset;
      let Ok((tag, next)) = read_varint(bytes, offset) else {
         return if fields.is_empty() {
            Err(())
         } else {
            Ok((fields, start))
         };
      };
      offset = next;
      let number = tag >> 3;
      if number == 0 {
         return if fields.is_empty() {
            Err(())
         } else {
            Ok((fields, start))
         };
      }
      let value = match tag & 7 {
         0 => match read_varint(bytes, offset) {
            Ok((value, next)) => {
               offset = next;
               WireValue::Varint(value)
            },
            Err(()) => return Err(()),
         },
         1 => {
            if bytes
               .get(offset..offset.checked_add(8).ok_or(())?)
               .is_none()
            {
               return Err(());
            }
            offset += 8;
            WireValue::Fixed64
         },
         2 => {
            let (length, next) = read_varint(bytes, offset)?;
            let length = usize::try_from(length).map_err(|_| ())?;
            let end = next.checked_add(length).ok_or(())?;
            let value = bytes.get(next..end).ok_or(())?;
            offset = end;
            WireValue::Bytes(value)
         },
         5 => {
            if bytes
               .get(offset..offset.checked_add(4).ok_or(())?)
               .is_none()
            {
               return Err(());
            }
            offset += 4;
            WireValue::Fixed32
         },
         _ => {
            return if fields.is_empty() {
               Err(())
            } else {
               Ok((fields, start))
            };
         },
      };
      fields.push(Field { number, value });
   }
   Ok((fields, offset))
}

fn one_bytes<'a>(fields: &'a [Field<'a>], number: u64) -> Result<&'a [u8], FreeformDecodeError> {
   let mut values = fields.iter().filter_map(|field| {
      (field.number == number).then_some(match field.value {
         WireValue::Bytes(value) => value,
         _ => &[],
      })
   });
   let value = values
      .next()
      .ok_or_else(|| invalid(format!("missing protobuf field {number}")))?;
   if values.next().is_some() || value.is_empty() {
      return Err(invalid(format!("invalid protobuf field {number}")));
   }
   Ok(value)
}

fn manifest_reference(bytes: &[u8], index: usize) -> Result<[u8; 16], FreeformDecodeError> {
   let outer =
      parse_fields(bytes).map_err(|()| invalid(format!("manifest item {index} is malformed")))?;
   let nested = one_bytes(&outer, 1)?;
   let inner = parse_fields(nested)
      .map_err(|()| invalid(format!("manifest item {index} UUID is malformed")))?;
   let raw = one_bytes(&inner, 1)?;
   raw.try_into()
      .map_err(|_| invalid(format!("manifest item {index} UUID is not 16 bytes")))
}

fn uuid_from_raw(raw: [u8; 16]) -> String {
   const HEX: &[u8; 16] = b"0123456789ABCDEF";
   let mut text = [0_u8; 36];
   let mut out = 0;
   for (index, byte) in raw.into_iter().enumerate() {
      if matches!(index, 4 | 6 | 8 | 10) {
         text[out] = b'-';
         out += 1;
      }
      text[out] = HEX[usize::from(byte >> 4)];
      text[out + 1] = HEX[usize::from(byte & 0x0f)];
      out += 2;
   }
   // The source is fixed ASCII generated above.
   String::from_utf8(text.to_vec()).expect("fixed ASCII UUID")
}

fn parse_uuid(text: &str, context: &str) -> Result<String, FreeformDecodeError> {
   let bytes = text.as_bytes();
   if bytes.len() != 36 || [8, 13, 18, 23].iter().any(|&index| bytes[index] != b'-') {
      return Err(invalid(format!("{context} is not a canonical UUID")));
   }
   let mut raw = [0_u8; 16];
   let mut byte_index = 0;
   let mut high = None;
   for (index, byte) in bytes.iter().copied().enumerate() {
      if matches!(index, 8 | 13 | 18 | 23) {
         continue;
      }
      let nibble = match byte {
         b'0'..=b'9' => byte - b'0',
         b'a'..=b'f' => byte - b'a' + 10,
         b'A'..=b'F' => byte - b'A' + 10,
         _ => return Err(invalid(format!("{context} is not a canonical UUID"))),
      };
      if let Some(high) = high.take() {
         raw[byte_index] = high << 4 | nibble;
         byte_index += 1;
      } else {
         high = Some(nibble);
      }
   }
   if high.is_some() || byte_index != raw.len() {
      return Err(invalid(format!("{context} is not a canonical UUID")));
   }
   Ok(uuid_from_raw(raw))
}

fn ensure_finite(value: &Plist) -> Result<(), FreeformDecodeError> {
   match value {
      Plist::Real(value) if !value.is_finite() => {
         Err(invalid("index contains non-finite framing metadata"))
      },
      Plist::Array(values) => values.iter().try_for_each(ensure_finite),
      Plist::Dict(values) => values.values().try_for_each(ensure_finite),
      _ => Ok(()),
   }
}

fn parse_index(index: &[u8]) -> Result<(String, Vec<String>), FreeformDecodeError> {
   let plist = parse_bplist(index).ok_or_else(|| invalid("invalid index plist"))?;
   ensure_finite(&plist)?;
   let root = plist
      .as_dict()
      .ok_or_else(|| invalid("index plist is not a dictionary"))?;
   let paste_id = root
      .get("id")
      .and_then(Plist::as_str)
      .ok_or_else(|| invalid("index is missing string id"))?;
   let board_items = root
      .get("boardItems")
      .and_then(Plist::as_array)
      .ok_or_else(|| invalid("index is missing boardItems array"))?;
   if !matches!(root.get("isSmartCopyPaste"), Some(Plist::Bool(_))) {
      return Err(invalid("index is missing boolean isSmartCopyPaste"));
   }
   let mut seen = BTreeSet::new();
   let mut item_ids = Vec::with_capacity(board_items.len());
   for (index, value) in board_items.iter().enumerate() {
      let value = value
         .as_str()
         .ok_or_else(|| invalid(format!("index boardItems[{index}] is not a string")))?;
      let uuid = parse_uuid(value, &format!("index boardItems[{index}]"))?;
      if !seen.insert(uuid.clone()) {
         return Err(invalid(format!("index boardItems[{index}] duplicates an earlier UUID")));
      }
      item_ids.push(uuid);
   }
   Ok((parse_uuid(paste_id, "index id")?, item_ids))
}

fn parse_manifest(
   manifest: &[u8],
) -> Result<(Vec<[u8; 16]>, FreeformCompatibility), FreeformDecodeError> {
   let fields = parse_fields(manifest).map_err(|()| invalid("manifest protobuf is malformed"))?;
   let mut schema = None;
   let mut minimum_version = None;
   let mut references = Vec::new();
   for field in fields {
      match (field.number, field.value) {
         (1, WireValue::Varint(value)) => {
            if schema.replace(value).is_some() {
               return Err(invalid("manifest repeats schema version"));
            }
         },
         (1, _) => return Err(invalid("manifest schema version has wrong wire type")),
         (3, WireValue::Bytes(value)) => {
            references.push(manifest_reference(value, references.len())?);
         },
         (3, _) => return Err(invalid("manifest item reference has wrong wire type")),
         (6, WireValue::Varint(value)) => {
            if minimum_version.replace(value).is_some() {
               return Err(invalid("manifest repeats minimum version"));
            }
         },
         (6, _) => return Err(invalid("manifest minimum version has wrong wire type")),
         _ => {},
      }
   }
   let compatibility = match minimum_version {
      Some(minimum_version) => FreeformCompatibility::Unsupported { minimum_version },
      None => schema.map_or(FreeformCompatibility::Unknown, |version| {
         FreeformCompatibility::Supported { version }
      }),
   };
   Ok((references, compatibility))
}

fn reconcile(manifest_ids: &[[u8; 16]], index_ids: &[String]) -> Result<(), FreeformDecodeError> {
   if manifest_ids.len() != index_ids.len() {
      return Err(FreeformDecodeError::correlation(format!(
         "CRLNativeData: manifest has {} item references but index has {} boardItems",
         manifest_ids.len(),
         index_ids.len()
      )));
   }
   // Older captured fixtures use an all-zero UUID for every manifest reference.
   // It is an explicit nil reference, not evidence that any index item owns data.
   if manifest_ids.iter().all(|raw| *raw == NIL_UUID) {
      return Ok(());
   }
   for (index, (raw, index_id)) in manifest_ids.iter().zip(index_ids).enumerate() {
      if *raw == NIL_UUID || uuid_from_raw(*raw) != *index_id {
         return Err(FreeformDecodeError::correlation(format!(
            "CRLNativeData: manifest/index item UUID mismatch at {index}"
         )));
      }
   }
   Ok(())
}

fn marker_starts(archive: &[u8]) -> Vec<(usize, usize)> {
   let mut markers = Vec::new();
   let mut offset = 0;
   while offset + 8 <= archive.len() {
      if archive.get(offset..offset + CRDT_TAG.len()) == Some(CRDT_TAG) {
         let start = offset;
         if markers.last().is_none_or(|(last, _)| *last != start) {
            markers.push((start, offset));
         }
         offset += CRDT_TAG.len();
      } else {
         offset += 1;
      }
   }
   markers
}

fn record_owner(fields: &[Field<'_>], manifest_ids: &HashMap<[u8; 16], String>) -> Option<String> {
   let mut owner = None;
   for field in fields {
      if field.number != 6 {
         continue;
      }
      let WireValue::Bytes(metadata) = field.value else {
         continue;
      };
      let metadata = parse_fields(metadata).ok()?;
      for field in metadata {
         if field.number != 1 {
            continue;
         }
         let WireValue::Bytes(identifier) = field.value else {
            continue;
         };
         // CRLProto_ObjectMetadata field 6 / field 1 is a 64-byte identifier;
         // its board-item UUID occupies bytes 16..32 in the verified framing.
         let raw: [u8; 16] = identifier.get(16..32)?.try_into().ok()?;
         let candidate = manifest_ids.get(&raw)?.clone();
         if owner.replace(candidate).is_some() {
            return None;
         }
      }
   }
   owner
}

fn valid_trailing_frame(bytes: &[u8]) -> bool {
   let Some(length) = read_u64_le(bytes, 0).and_then(|length| usize::try_from(length).ok()) else {
      return false;
   };
   length.checked_add(8) == Some(bytes.len())
}

fn discover_records<'a>(archive: &'a [u8], item_ids: &[[u8; 16]]) -> Vec<NativeRecord<'a>> {
   let known = item_ids
      .iter()
      .copied()
      .map(|raw| (raw, uuid_from_raw(raw)))
      .collect::<HashMap<_, _>>();
   let markers = marker_starts(archive);
   let mut records = Vec::new();
   let mut current_owner = None;
   for (index, (_, tag)) in markers.iter().copied().enumerate() {
      let Some(payload_start) = tag.checked_add(8) else {
         continue;
      };
      let end = markers
         .get(index + 1)
         .map_or(archive.len(), |(next, _)| *next);
      if payload_start >= end {
         continue;
      }
      let payload = &archive[payload_start..end];
      let Ok((fields, consumed)) = parse_field_prefix(payload) else {
         continue;
      };
      if consumed == 0 || (consumed != payload.len() && !valid_trailing_frame(&payload[consumed..]))
      {
         continue;
      }
      // A board item's common CRDT record carries its UUID. Its immediately
      // following specific-data records deliberately carry a nil board UUID;
      // the next common record starts the next ownership group.
      if let Some(owner_id) = record_owner(&fields, &known) {
         current_owner = Some(owner_id);
      }
      let Some(owner_id) = current_owner.clone() else {
         continue;
      };
      let record_end = payload_start + consumed;
      records.push(NativeRecord {
         owner_id,
         offset: payload_start,
         bytes: &archive[payload_start..record_end],
      });
   }
   records
}

/// Parses and reconciles the strict CRL manifest, plist index, and bounded item
/// records.
pub fn parse_native_archive(data: &[u8]) -> Result<NativeArchive<'_>, FreeformDecodeError> {
   let manifest_length =
      read_u64_le(data, 0).ok_or_else(|| incomplete("truncated manifest length"))?;
   let manifest_length = usize::try_from(manifest_length)
      .map_err(|_| incomplete("manifest length exceeds address space"))?;
   let manifest_start: usize = 8;
   let manifest_end = manifest_start
      .checked_add(manifest_length)
      .ok_or_else(|| incomplete("manifest length overflow"))?;
   let manifest = data
      .get(manifest_start..manifest_end)
      .ok_or_else(|| incomplete("truncated manifest"))?;
   let index_start = manifest_end;
   if data.get(
      index_start
         ..index_start
            .checked_add(BPLIST_HEADER.len())
            .ok_or_else(|| incomplete("index offset overflow"))?,
   ) != Some(BPLIST_HEADER)
   {
      return Err(incomplete("expected bounded bplist index after manifest"));
   }
   let index_length = bounded_plist_length(data, index_start)
      .ok_or_else(|| incomplete("could not locate complete index plist trailer"))?;
   let index_end = index_start
      .checked_add(index_length)
      .ok_or_else(|| incomplete("index length overflow"))?;
   let raw_index = data
      .get(index_start..index_end)
      .ok_or_else(|| incomplete("truncated index plist"))?;
   let archive = data
      .get(index_end..)
      .ok_or_else(|| incomplete("missing object archive"))?;

   let (manifest_ids, compatibility) = parse_manifest(manifest)?;
   let (paste_id, item_ids) = parse_index(raw_index)?;
   reconcile(&manifest_ids, &item_ids)?;
   let records = discover_records(archive, &manifest_ids);
   Ok(NativeArchive {
      manifest,
      raw_index,
      index_range: index_start..index_end,
      archive,
      paste_id,
      item_ids,
      compatibility,
      records,
   })
}

#[cfg(test)]
mod tests {
   use super::*;

   const REAL: &[u8] = include_bytes!("../../fixtures/real-board.crlnative");

   #[test]
   fn parses_real_fixture_envelope() {
      let archive = parse_native_archive(REAL).expect("fixture envelope");
      assert_eq!(archive.item_ids.len(), 10);
      assert_eq!(archive.paste_id, "42753C09-5C97-4F7E-A652-F842FEFFCEAD");
      assert_eq!(&REAL[archive.index_range.clone()], archive.raw_index);
      assert!(!archive.records.is_empty(), "fixture has structurally owned CRDT records");
      assert_eq!(archive.records.len(), 21, "every fixture CRDT record has a proven owner");
   }

   #[test]
   fn rejects_u64_max_manifest_length() {
      let mut bytes = REAL.to_vec();
      bytes[..8].copy_from_slice(&u64::MAX.to_le_bytes());
      assert_eq!(
         parse_native_archive(&bytes).unwrap_err().kind,
         crate::types::FreeformDecodeErrorKind::Incomplete
      );
   }

   #[test]
   fn rejects_non_dict_and_missing_index() {
      let mut non_dict = REAL.to_vec();
      let index = 8 + usize::try_from(read_u64_le(REAL, 0).unwrap()).unwrap();
      non_dict[index + 8] = 0x51;
      assert_eq!(
         parse_native_archive(&non_dict).unwrap_err().kind,
         crate::types::FreeformDecodeErrorKind::Invalid
      );

      let mut missing = REAL.to_vec();
      let id = index
         + REAL[index..]
            .windows(2)
            .position(|window| window == b"id")
            .unwrap();
      missing[id..id + 2].copy_from_slice(b"xx");
      assert_eq!(
         parse_native_archive(&missing).unwrap_err().kind,
         crate::types::FreeformDecodeErrorKind::Invalid
      );
   }

   #[test]
   fn rejects_invalid_duplicate_and_middle_uuid() {
      let index = 8 + usize::try_from(read_u64_le(REAL, 0).unwrap()).unwrap();
      let mut invalid = REAL.to_vec();
      let first = index
         + REAL[index..]
            .windows(36)
            .position(|window| window == b"42753C09-5C97-4F7E-A652-F842FEFFCEAD")
            .unwrap();
      invalid[first] = b'G';
      assert_eq!(
         parse_native_archive(&invalid).unwrap_err().kind,
         crate::types::FreeformDecodeErrorKind::Invalid
      );

      let mut duplicate = REAL.to_vec();
      let board = index
         + REAL[index..]
            .windows(36)
            .position(|window| window == b"0D3126A7-0404-4542-9E5D-4F38F4A51EB7")
            .unwrap();
      let next = index
         + REAL[index + (board - index) + 36..]
            .windows(36)
            .position(|window| window == b"E48D4C17-FBDB-4920-B27F-EF3D88178CB9")
            .unwrap()
         + (board - index)
         + 36;
      duplicate[next..next + 36].copy_from_slice(&REAL[board..board + 36]);
      assert_eq!(
         parse_native_archive(&duplicate).unwrap_err().kind,
         crate::types::FreeformDecodeErrorKind::Invalid
      );

      let mut middle = REAL.to_vec();
      let position = index
         + REAL[index..]
            .windows(36)
            .position(|window| window == b"F339373B-91E7-4C27-BE9C-388479D3A05F")
            .unwrap();
      middle[position] = b'G';
      assert_eq!(
         parse_native_archive(&middle).unwrap_err().kind,
         crate::types::FreeformDecodeErrorKind::Invalid
      );
   }

   #[test]
   fn rejects_non_finite_index_framing_metadata() {
      let index = 8 + usize::try_from(read_u64_le(REAL, 0).unwrap()).unwrap();
      let mut bytes = REAL.to_vec();
      let uuid = b"0D3126A7-0404-4542-9E5D-4F38F4A51EB7";
      let value = index
         + REAL[index..]
            .windows(uuid.len())
            .position(|window| window == uuid)
            .unwrap();
      let marker = value - 3;
      assert_eq!(bytes[marker..value], [0x5f, 0x10, 0x24]);
      bytes[marker] = 0x23;
      bytes[marker + 1..marker + 9].copy_from_slice(&f64::NAN.to_be_bytes());
      let error = parse_native_archive(&bytes).unwrap_err();
      assert_eq!(error.kind, crate::types::FreeformDecodeErrorKind::Invalid);
      assert!(error.message.contains("non-finite"));
   }

   #[test]
   fn rejects_manifest_index_mismatch() {
      let mut bytes = REAL.to_vec();
      let manifest_id = bytes
         .windows(16)
         .position(|window| {
            window
               == [
                  0x0d, 0x31, 0x26, 0xa7, 0x04, 0x04, 0x45, 0x42, 0x9e, 0x5d, 0x4f, 0x38, 0xf4,
                  0xa5, 0x1e, 0xb7,
               ]
         })
         .unwrap();
      bytes[manifest_id] ^= 1;
      assert_eq!(
         parse_native_archive(&bytes).unwrap_err().kind,
         crate::types::FreeformDecodeErrorKind::CorrelationMismatch
      );
   }
}
