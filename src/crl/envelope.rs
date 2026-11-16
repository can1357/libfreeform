//! Explicit Freeform object directories and their recursively serialized
//! children.
//!
//! Each object starts with an eight-byte little-endian protobuf length. Its
//! `ObjectInfo` directory names and bounds every following payload; child
//! references then define the order and ownership of subsequent objects.

use std::{cmp::Ordering, collections::BTreeSet, ops::Range};

use crate::{
   bplist::{Plist, parse_bplist},
   types::{FreeformCompatibility, FreeformDecodeError},
   wire::Varints,
};

/// `CRLProto` object framing schemas.
mod proto {
   use crate::wire::{self, Repeated, Varints};

   wire::message! {
      /// `CRLProto.ObjectInfo`: one object's kind, children, and payload
      /// directory.
      pub struct ObjectInfo<'a> {
         persistence_type: Option<u64> = 1,
         /// Minimum-version components.
         minimum_version: Varints<'a> = 2,
         children: Repeated<'a, ObjectReference<'a>> = 3,
         /// Names and lengths of the payloads following this message.
         data_directory_entries: Repeated<'a, DataDirectoryEntry<'a>> = 5,
         /// Good-enough-fidelity version components.
         fidelity_version: Varints<'a> = 7,
         persistence_subtype: Option<u64> = 9,
      }
   }

   wire::message! {
      /// A child object reference.
      pub struct ObjectReference<'a> {
         identifier: Option<Identifier<'a>> = 1,
      }
   }

   wire::message! {
      /// A child object identifier.
      pub struct Identifier<'a> {
         uuid: Option<&'a [u8]> = 1,
      }
   }

   wire::message! {
      /// One named payload region.
      pub struct DataDirectoryEntry<'a> {
         name: Option<&'a str> = 1,
         length: Option<u64> = 2,
      }
   }
}

// The captured table and premium image require 4.2 and 4.4 respectively,
// although their enclosing pasteboard root declares good-enough fidelity 2.4.
const VERIFIED_VERSION: &[u32] = &[4, 4];
const MAX_OBJECT_DEPTH: usize = 128;

/// One exact named data region, including its native format header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct NativeRegion<'a> {
   /// Key from `ObjectInfo.dataDirectoryEntries`.
   pub name:   &'a str,
   /// Offset from the beginning of `NativeArchive::archive`.
   pub offset: usize,
   /// Complete payload, never delimited by searching its contents.
   pub bytes:  &'a [u8],
}

/// A native object whose identity comes from its parent's child reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct NativeObject<'a> {
   pub id:                  String,
   pub parent_id:           Option<String>,
   pub persistence_type:    u32,
   pub persistence_subtype: Option<u32>,
   pub children:            Vec<String>,
   pub regions:             Vec<NativeRegion<'a>>,
   pub info:                &'a [u8],
}

impl<'a> NativeObject<'a> {
   /// Looks up a payload by its native directory key.
   pub fn region(&self, name: &str) -> Option<&NativeRegion<'a>> {
      self.regions.iter().find(|region| region.name == name)
   }
}

/// Validated root index and every recursively owned native object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct NativeArchive<'a> {
   pub manifest:      &'a [u8],
   pub raw_index:     &'a [u8],
   pub index_range:   Range<usize>,
   pub archive:       &'a [u8],
   pub paste_id:      String,
   pub item_ids:      Vec<String>,
   pub compatibility: FreeformCompatibility,
   pub objects:       Vec<NativeObject<'a>>,
}

struct ObjectInfo<'a> {
   bytes:               &'a [u8],
   persistence_type:    u32,
   persistence_subtype: Option<u32>,
   children:            Vec<String>,
   regions:             Vec<NativeRegion<'a>>,
   compatibility:       FreeformCompatibility,
}

struct Reader<'a> {
   input:         &'a [u8],
   offset:        usize,
   archive_start: usize,
   seen:          BTreeSet<String>,
   objects:       Vec<NativeObject<'a>>,
   compatibility: FreeformCompatibility,
}

fn invalid(message: &str) -> FreeformDecodeError {
   FreeformDecodeError::invalid(format!("CRLNativeData: {message}"))
}

impl<'a> Reader<'a> {
   fn take(&mut self, length: usize) -> Result<&'a [u8], FreeformDecodeError> {
      let (bytes, rest) = self
         .input
         .split_at_checked(length)
         .ok_or_else(|| FreeformDecodeError::incomplete("CRLNativeData: truncated object data"))?;
      self.input = rest;
      self.offset += length;
      Ok(bytes)
   }

   fn info(&mut self) -> Result<ObjectInfo<'a>, FreeformDecodeError> {
      let length = u64::from_le_bytes(*self.take(8)?.first_chunk().expect("eight bytes"));
      let length = usize::try_from(length).map_err(|_| invalid("object info length overflow"))?;
      let bytes = self.take(length)?;
      let info = proto::ObjectInfo::parse(bytes)?;
      let persistence_type = u32::try_from(info.persistence_type.unwrap_or(0))
         .map_err(|_| invalid("persistence type exceeds uint32"))?;
      let persistence_subtype = info
         .persistence_subtype
         .map(u32::try_from)
         .transpose()
         .map_err(|_| invalid("persistence subtype exceeds uint32"))?;
      let children = info
         .children
         .into_iter()
         .map(|reference| reference_id(reference?))
         .collect::<Result<_, _>>()?;
      let mut entries = Vec::new();
      let mut names = BTreeSet::new();
      for entry in info.data_directory_entries {
         let entry = entry?;
         let name = entry
            .name
            .ok_or_else(|| invalid("directory entry has no name"))?;
         if name.is_empty() || !names.insert(name) {
            return Err(invalid("empty or duplicate data directory name"));
         }
         let size = usize::try_from(entry.length.unwrap_or(0))
            .map_err(|_| invalid("data directory length overflow"))?;
         entries.push((name, size));
      }
      let mut regions = Vec::with_capacity(entries.len());
      for (name, size) in entries {
         let offset = self.offset;
         let bytes = self.take(size)?;
         regions.push(NativeRegion { name, offset, bytes });
      }
      Ok(ObjectInfo {
         bytes,
         persistence_type,
         persistence_subtype,
         children,
         regions,
         compatibility: compatibility(info)?,
      })
   }

   fn object(
      &mut self,
      id: String,
      parent_id: Option<&str>,
      depth: usize,
   ) -> Result<(), FreeformDecodeError> {
      if depth > MAX_OBJECT_DEPTH {
         return Err(invalid("object nesting exceeds the supported depth"));
      }
      if !self.seen.insert(id.clone()) {
         return Err(FreeformDecodeError::correlation(format!(
            "CRLNativeData: duplicate object {id}"
         )));
      }
      let mut info = self.info()?;
      merge_compatibility(&mut self.compatibility, &info.compatibility);
      for region in &mut info.regions {
         region.offset -= self.archive_start;
      }
      let index = self.objects.len();
      self.objects.push(NativeObject {
         id:                  id.clone(),
         parent_id:           parent_id.map(str::to_owned),
         persistence_type:    info.persistence_type,
         persistence_subtype: info.persistence_subtype,
         children:            info.children,
         regions:             info.regions,
         info:                info.bytes,
      });
      for child in 0..self.objects[index].children.len() {
         let child_id = self.objects[index].children[child].clone();
         self.object(child_id, Some(&id), depth + 1)?;
      }
      Ok(())
   }
}

fn reference_id(reference: proto::ObjectReference<'_>) -> Result<String, FreeformDecodeError> {
   let uuid = reference
      .identifier
      .ok_or_else(|| invalid("child reference has no identifier"))?
      .uuid
      .ok_or_else(|| invalid("child identifier has no UUID"))?;
   if uuid == [0; 16] {
      return Err(invalid("nil child object UUID"));
   }
   uuid_string(uuid)
}

/// Formats an exact native UUID, rejecting incorrectly sized identifiers.
pub(super) fn uuid_string(bytes: &[u8]) -> Result<String, FreeformDecodeError> {
   let raw = bytes
      .as_array::<16>()
      .ok_or_else(|| invalid("UUID is not 16 bytes"))?;
   const HEX: &[u8; 16] = b"0123456789ABCDEF";
   let mut text = [0_u8; 36];
   let mut out = 0;
   for (index, byte) in raw.iter().copied().enumerate() {
      if matches!(index, 4 | 6 | 8 | 10) {
         text[out] = b'-';
         out += 1;
      }
      text[out] = HEX[usize::from(byte >> 4)];
      text[out + 1] = HEX[usize::from(byte & 15)];
      out += 2;
   }
   Ok(String::from_utf8(text.to_vec()).expect("generated ASCII UUID"))
}

fn canonical_uuid(text: &str) -> Result<String, FreeformDecodeError> {
   let bytes = text.as_bytes();
   if bytes.len() != 36 || [8, 13, 18, 23].iter().any(|&i| bytes[i] != b'-') {
      return Err(invalid("index contains a non-canonical UUID"));
   }
   let mut raw = [0_u8; 16];
   let mut index = 0;
   let mut high = None;
   for (position, byte) in bytes.iter().copied().enumerate() {
      if matches!(position, 8 | 13 | 18 | 23) {
         continue;
      }
      let nibble = match byte {
         b'0'..=b'9' => byte - b'0',
         b'a'..=b'f' => byte - b'a' + 10,
         b'A'..=b'F' => byte - b'A' + 10,
         _ => return Err(invalid("index contains an invalid UUID")),
      };
      if let Some(first) = high.take() {
         raw[index] = first << 4 | nibble;
         index += 1;
      } else {
         high = Some(nibble);
      }
   }
   uuid_string(&raw)
}

fn parse_index(bytes: &[u8]) -> Result<(String, Vec<String>), FreeformDecodeError> {
   let plist = parse_bplist(bytes).ok_or_else(|| invalid("invalid index plist"))?;
   let root = plist
      .as_dict()
      .ok_or_else(|| invalid("index is not a dictionary"))?;
   let paste_id = root
      .get("id")
      .and_then(Plist::as_str)
      .ok_or_else(|| invalid("index has no paste UUID"))?;
   if !matches!(root.get("isSmartCopyPaste"), Some(Plist::Bool(_))) {
      return Err(invalid("index has no boolean isSmartCopyPaste"));
   }
   let items = root
      .get("boardItems")
      .and_then(Plist::as_array)
      .ok_or_else(|| invalid("index has no boardItems array"))?;
   let mut seen = BTreeSet::new();
   let mut ids = Vec::with_capacity(items.len());
   for item in items {
      let id = canonical_uuid(
         item
            .as_str()
            .ok_or_else(|| invalid("board item UUID is not a string"))?,
      )?;
      if !seen.insert(id.clone()) {
         return Err(invalid("duplicate index board item UUID"));
      }
      ids.push(id);
   }
   Ok((canonical_uuid(paste_id)?, ids))
}

fn compatibility(
   info: proto::ObjectInfo<'_>,
) -> Result<FreeformCompatibility, FreeformDecodeError> {
   let components = |version: Varints<'_>| {
      version
         .into_iter()
         .map(|component| {
            u32::try_from(component?).map_err(|_| invalid("version component exceeds uint32"))
         })
         .collect::<Result<Vec<_>, _>>()
   };
   let version = [components(info.minimum_version)?, components(info.fidelity_version)?]
      .into_iter()
      .filter(|v| !v.is_empty())
      .max_by(|a, b| compare_versions(a, b));
   Ok(match version {
      None => FreeformCompatibility::Unknown,
      Some(version) if compare_versions(&version, VERIFIED_VERSION).is_le() => {
         FreeformCompatibility::Supported { version }
      },
      Some(version) => FreeformCompatibility::Unsupported { version },
   })
}

fn compare_versions(a: &[u32], b: &[u32]) -> Ordering {
   (0..a.len().max(b.len()))
      .map(|i| a.get(i).unwrap_or(&0).cmp(b.get(i).unwrap_or(&0)))
      .find(|order| order.is_ne())
      .unwrap_or(Ordering::Equal)
}

fn declared_version(status: &FreeformCompatibility) -> Option<&[u32]> {
   match status {
      FreeformCompatibility::Supported { version }
      | FreeformCompatibility::Unsupported { version } => Some(version),
      FreeformCompatibility::Unknown => None,
   }
}

fn merge_compatibility(target: &mut FreeformCompatibility, source: &FreeformCompatibility) {
   if let Some(source_version) = declared_version(source)
      && declared_version(target)
         .is_none_or(|target| compare_versions(target, source_version).is_lt())
   {
      *target = source.clone();
   }
}

/// Parses the exact root directory and referenced object tree, rejecting
/// trailing or missing bytes.
pub(super) fn parse_native_archive(data: &[u8]) -> Result<NativeArchive<'_>, FreeformDecodeError> {
   let mut reader = Reader {
      input:         data,
      offset:        0,
      archive_start: 0,
      seen:          BTreeSet::new(),
      objects:       Vec::new(),
      compatibility: FreeformCompatibility::Unknown,
   };
   let root = reader.info()?;
   if root.persistence_type != 99 {
      return Err(invalid("root is not a pasteboard object"));
   }
   let index = root
      .regions
      .iter()
      .find(|region| region.name == "objectData")
      .ok_or_else(|| invalid("root directory has no objectData index"))?;
   let (paste_id, item_ids) = parse_index(index.bytes)?;
   if root.children != item_ids {
      return Err(FreeformDecodeError::correlation(
         "CRLNativeData: manifest/index item UUID mismatch",
      ));
   }
   reader.archive_start = reader.offset;
   reader.compatibility = root.compatibility;
   let archive = reader.input;
   for id in &item_ids {
      reader.object(id.clone(), None, 0)?;
   }
   if !reader.input.is_empty() {
      return Err(invalid("bytes follow the declared object tree"));
   }
   Ok(NativeArchive {
      manifest: root.bytes,
      raw_index: index.bytes,
      index_range: index.offset..index.offset + index.bytes.len(),
      archive,
      paste_id,
      item_ids,
      compatibility: reader.compatibility,
      objects: reader.objects,
   })
}

#[cfg(test)]
mod tests {
   use super::*;
   const REAL: &[u8] = include_bytes!("../../fixtures/real-board.crlnative");

   #[test]
   fn real_directories_include_table_capsule_and_embedded_child() {
      let archive = parse_native_archive(REAL).unwrap();
      assert_eq!(archive.compatibility, FreeformCompatibility::Supported { version: vec![4, 4] });
      let table = &archive.objects[8];
      assert_eq!(table.persistence_type, 2);
      assert_eq!(table.children, ["5AAC3C24-07E5-4429-8B8C-D0FF59B8961D"]);
      let capsule = table.region("capsuleData").unwrap();
      assert_eq!(capsule.bytes.len(), 3497);
      assert_eq!(
         capsule.bytes,
         &archive.archive[capsule.offset..capsule.offset + capsule.bytes.len()]
      );
      assert_eq!(archive.objects[9].parent_id.as_deref(), Some(table.id.as_str()));
      assert_eq!(archive.objects[10].id, *archive.item_ids.last().unwrap());
   }

   #[test]
   fn rejects_truncated_or_trailing_object_data() {
      for end in [8, 255, 810, 1024, REAL.len() - 1] {
         assert!(parse_native_archive(&REAL[..end]).is_err());
      }
      let mut trailing = REAL.to_vec();
      trailing.push(0);
      assert_eq!(
         parse_native_archive(&trailing).unwrap_err().kind,
         crate::types::FreeformDecodeErrorKind::Invalid
      );
   }

   #[test]
   fn compatibility_follows_the_newest_declared_version() {
      let compatibility = |bytes: &[u8]| compatibility(proto::ObjectInfo::parse(bytes)?);
      assert_eq!(
         compatibility(&[0x12, 1, 1, 0x3a, 2, 4, 5]).unwrap(),
         FreeformCompatibility::Unsupported { version: vec![4, 5] }
      );
      assert_eq!(compatibility(&[0x3a, 3, 2, 4, 0]).unwrap(), FreeformCompatibility::Supported {
         version: vec![2, 4, 0],
      });
      assert!(compatibility(&[0x15, 0, 0, 0, 0]).is_err());
   }
}
