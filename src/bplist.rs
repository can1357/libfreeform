//! Strict Apple binary property list (`bplist00`) reader.
//!
//! The reader supports the property-list types used by Freeform archives and
//! rejects malformed or unsupported objects rather than inventing a value for
//! them. Every span is constrained to the declared object area so embedded
//! archive bytes cannot be mistaken for plist payload.

use std::{collections::HashMap, mem::size_of, ops::Range};

/// Plain binary-plist value tree.
#[derive(Debug, Clone, PartialEq)]
pub enum Plist {
   /// The plist null object.
   Null,
   /// A boolean object.
   Bool(bool),
   /// A signed integer object.
   Int(i64),
   /// A floating-point number or date (seconds from the Apple reference date).
   Real(f64),
   /// An ASCII or UTF-16 string.
   String(String),
   /// An opaque byte string.
   Data(Vec<u8>),
   /// An ordered array.
   Array(Vec<Self>),
   /// An ordered set.
   OrderedSet(Vec<Self>),
   /// An unordered set.
   Set(Vec<Self>),
   /// A dictionary with string keys.
   Dict(HashMap<String, Self>),
   /// A binary-plist UID, which is not an integer value.
   Uid(u64),
}

impl Plist {
   /// Returns the value as a string when it is one.
   pub fn as_str(&self) -> Option<&str> {
      match self {
         Self::String(s) => Some(s),
         _ => None,
      }
   }

   /// Returns the value as an array when it is one.
   pub fn as_array(&self) -> Option<&[Self]> {
      match self {
         Self::Array(a) => Some(a),
         _ => None,
      }
   }

   /// Returns the value as a dictionary when it is one.
   pub const fn as_dict(&self) -> Option<&HashMap<String, Self>> {
      match self {
         Self::Dict(d) => Some(d),
         _ => None,
      }
   }
}

const HEADER: &[u8; 8] = b"bplist00";
const TRAILER_SIZE: usize = 32;
const MAX_OBJECTS: usize = 100_000;
const MAX_EXPANDED_OBJECTS: usize = 250_000;
const MAX_DEPTH: u32 = 128;
const MAX_COLLECTION_ENTRIES: usize = 100_000;
const MAX_MATERIALIZED_BYTES: usize = 16 * 1024 * 1024;
const MAX_TRAILER_CANDIDATES: usize = 4_096;

#[derive(Clone, Copy)]
struct Trailer {
   offset_int_size:     usize,
   object_ref_size:     usize,
   num_objects:         usize,
   top_object:          usize,
   offset_table_offset: usize,
}

/// Reads an exact, unsigned, big-endian integer with a supported plist width.
fn read_uint(b: &[u8], offset: usize, width: usize) -> Option<u64> {
   if !(1..=8).contains(&width) {
      return None;
   }
   let bytes = b.get(offset..offset.checked_add(width)?)?;
   Some(
      bytes
         .iter()
         .fold(0_u64, |value, &byte| (value << 8) | byte as u64),
   )
}

/// Reads an exact signed, big-endian integer with a supported plist width.
fn read_int(b: &[u8], offset: usize, width: usize) -> Option<i64> {
   let value = read_uint(b, offset, width)?;
   let shift = 64_u32.checked_sub((width as u32).checked_mul(8)?)?;
   Some(((value << shift) as i64) >> shift)
}

/// Reads and validates a binary plist trailer at `trailer_start`.
fn trailer_at(b: &[u8], trailer_start: usize) -> Option<Trailer> {
   let trailer = b.get(trailer_start..trailer_start.checked_add(TRAILER_SIZE)?)?;
   if trailer[..6].iter().any(|&byte| byte != 0) {
      return None;
   }

   let offset_int_size = trailer[6] as usize;
   let object_ref_size = trailer[7] as usize;
   if !(1..=8).contains(&offset_int_size) || !(1..=8).contains(&object_ref_size) {
      return None;
   }

   let num_objects = usize::try_from(read_uint(trailer, 8, 8)?).ok()?;
   let top_object = usize::try_from(read_uint(trailer, 16, 8)?).ok()?;
   let offset_table_offset = usize::try_from(read_uint(trailer, 24, 8)?).ok()?;
   if num_objects == 0 || num_objects > MAX_OBJECTS || top_object >= num_objects {
      return None;
   }
   if offset_table_offset < HEADER.len() {
      return None;
   }
   let offset_table_len = num_objects.checked_mul(offset_int_size)?;
   if offset_table_offset.checked_add(offset_table_len)? != trailer_start {
      return None;
   }

   Some(Trailer { offset_int_size, object_ref_size, num_objects, top_object, offset_table_offset })
}

/// Locate a complete bounded `bplist00` inside `data`.
///
/// The index plist inside `CRLNativeData` does not run to EOF. Candidate
/// trailers are fully parsed before acceptance, so trailer-shaped bytes in an
/// object payload do not hide the later, real trailer.
pub fn bounded_plist_length(data: &[u8], p0: usize) -> Option<usize> {
   if data.get(p0..p0.checked_add(HEADER.len())?)? != HEADER {
      return None;
   }
   let first = p0.checked_add(HEADER.len())?;
   let last = data.len().checked_sub(TRAILER_SIZE)?;
   if first > last {
      return None;
   }

   let mut candidates = 0_usize;
   for trailer_start in first..=last {
      let end = trailer_start.checked_add(TRAILER_SIZE)?;
      let candidate = data.get(p0..end)?;
      let local_trailer_start = trailer_start.checked_sub(p0)?;
      if trailer_at(candidate, local_trailer_start).is_none() {
         continue;
      }
      candidates = candidates.checked_add(1)?;
      if candidates > MAX_TRAILER_CANDIDATES {
         return None;
      }
      if parse_bplist(candidate).is_some() {
         return end.checked_sub(p0);
      }
   }
   None
}

struct Parser<'a> {
   b:                   &'a [u8],
   object_ref_size:     usize,
   offset_table_offset: usize,
   object_offsets:      Vec<usize>,
   expanded_objects:    usize,
   materialized_bytes:  usize,
}

impl<'a> Parser<'a> {
   fn new(b: &'a [u8], trailer: Trailer) -> Result<Self, ()> {
      let mut object_offsets = Vec::new();
      object_offsets
         .try_reserve_exact(trailer.num_objects)
         .map_err(|_| ())?;
      for index in 0..trailer.num_objects {
         let entry = trailer
            .offset_table_offset
            .checked_add(index.checked_mul(trailer.offset_int_size).ok_or(())?)
            .ok_or(())?;
         let offset = usize::try_from(read_uint(b, entry, trailer.offset_int_size).ok_or(())?)
            .map_err(|_| ())?;
         if !(HEADER.len()..trailer.offset_table_offset).contains(&offset) {
            return Err(());
         }
         object_offsets.push(offset);
      }
      Ok(Self {
         b,
         object_ref_size: trailer.object_ref_size,
         offset_table_offset: trailer.offset_table_offset,
         object_offsets,
         expanded_objects: 0,
         materialized_bytes: 0,
      })
   }

   fn object_span(&self, start: usize, len: usize) -> Result<Range<usize>, ()> {
      let end = start.checked_add(len).ok_or(())?;
      if start < HEADER.len() || end > self.offset_table_offset {
         return Err(());
      }
      Ok(start..end)
   }

   fn charge_bytes(&mut self, len: usize) -> Result<(), ()> {
      self.materialized_bytes = self.materialized_bytes.checked_add(len).ok_or(())?;
      if self.materialized_bytes > MAX_MATERIALIZED_BYTES {
         return Err(());
      }
      Ok(())
   }

   /// Returns the object count and byte offset after a length encoding.
   fn count_at(&self, start: usize, info: u8) -> Result<(usize, usize), ()> {
      if info != 0x0f {
         return Ok((info as usize, start.checked_add(1).ok_or(())?));
      }
      let count_marker_at = start.checked_add(1).ok_or(())?;
      let count_marker = *self.b.get(count_marker_at).ok_or(())?;
      if count_marker & 0xf0 != 0x10 {
         return Err(());
      }
      let width = 1_usize
         .checked_shl((count_marker & 0x0f) as u32)
         .ok_or(())?;
      if width > 8 {
         return Err(());
      }
      let count_data_at = count_marker_at.checked_add(1).ok_or(())?;
      self.object_span(count_marker_at, 1_usize.checked_add(width).ok_or(())?)?;
      let count = read_int(self.b, count_data_at, width).ok_or(())?;
      if count < 0 {
         return Err(());
      }
      Ok((usize::try_from(count).map_err(|_| ())?, count_data_at.checked_add(width).ok_or(())?))
   }

   fn reference_at(&self, at: usize) -> Result<usize, ()> {
      let reference =
         usize::try_from(read_uint(self.b, at, self.object_ref_size).ok_or(())?).map_err(|_| ())?;
      if reference >= self.object_offsets.len() {
         return Err(());
      }
      Ok(reference)
   }

   fn collection(&mut self, refs_start: usize, count: usize, depth: u32) -> Result<Vec<Plist>, ()> {
      if count > MAX_COLLECTION_ENTRIES {
         return Err(());
      }
      let refs_len = count.checked_mul(self.object_ref_size).ok_or(())?;
      self.object_span(refs_start, refs_len)?;
      self.charge_bytes(count.checked_mul(size_of::<Plist>()).ok_or(())?)?;
      let mut values = Vec::new();
      values.try_reserve_exact(count).map_err(|_| ())?;
      for index in 0..count {
         let at = refs_start
            .checked_add(index.checked_mul(self.object_ref_size).ok_or(())?)
            .ok_or(())?;
         values.push(self.object(self.reference_at(at)?, depth.checked_add(1).ok_or(())?)?);
      }
      Ok(values)
   }

   fn dictionary(
      &mut self,
      refs_start: usize,
      count: usize,
      depth: u32,
   ) -> Result<HashMap<String, Plist>, ()> {
      if count > MAX_COLLECTION_ENTRIES {
         return Err(());
      }
      let refs_len = count
         .checked_mul(self.object_ref_size)
         .and_then(|len| len.checked_mul(2))
         .ok_or(())?;
      self.object_span(refs_start, refs_len)?;
      // Covers the hash-table entries and limits intentionally repeated keys.
      self.charge_bytes(count.checked_mul(128).ok_or(())?)?;
      let value_base = refs_start
         .checked_add(count.checked_mul(self.object_ref_size).ok_or(())?)
         .ok_or(())?;
      let mut values = HashMap::new();
      values.try_reserve(count).map_err(|_| ())?;
      for index in 0..count {
         let key_at = refs_start
            .checked_add(index.checked_mul(self.object_ref_size).ok_or(())?)
            .ok_or(())?;
         let value_at = value_base
            .checked_add(index.checked_mul(self.object_ref_size).ok_or(())?)
            .ok_or(())?;
         let key = self.object(self.reference_at(key_at)?, depth.checked_add(1).ok_or(())?)?;
         let Plist::String(key) = key else {
            return Err(());
         };
         if values.contains_key(&key) {
            return Err(());
         }
         let value = self.object(self.reference_at(value_at)?, depth.checked_add(1).ok_or(())?)?;
         values.insert(key, value);
      }
      Ok(values)
   }

   fn object(&mut self, index: usize, depth: u32) -> Result<Plist, ()> {
      if depth > MAX_DEPTH || self.expanded_objects >= MAX_EXPANDED_OBJECTS {
         return Err(());
      }
      self.expanded_objects += 1;
      let start = *self.object_offsets.get(index).ok_or(())?;
      let marker = *self.b.get(start).ok_or(())?;
      let object_type = marker & 0xf0;
      let info = marker & 0x0f;

      match object_type {
         0x00 => match marker {
            0x00 => Ok(Plist::Null),
            0x08 => Ok(Plist::Bool(false)),
            0x09 => Ok(Plist::Bool(true)),
            _ => Err(()),
         },
         0x10 => {
            let width = 1_usize.checked_shl(info as u32).ok_or(())?;
            if width > 8 {
               return Err(());
            }
            let value_at = self
               .object_span(start, 1_usize.checked_add(width).ok_or(())?)?
               .start
               + 1;
            Ok(Plist::Int(read_int(self.b, value_at, width).ok_or(())?))
         },
         0x20 => {
            let width = match info {
               2 => 4,
               3 => 8,
               _ => return Err(()),
            };
            let value_at = self.object_span(start, 1 + width)?.start + 1;
            let value = match width {
               4 => f32::from_be_bytes(self.b[value_at..value_at + 4].try_into().map_err(|_| ())?)
                  as f64,
               8 => f64::from_be_bytes(self.b[value_at..value_at + 8].try_into().map_err(|_| ())?),
               _ => return Err(()),
            };
            Ok(Plist::Real(value))
         },
         0x30 => {
            if info != 3 {
               return Err(());
            }
            let value_at = self.object_span(start, 9)?.start + 1;
            Ok(Plist::Real(f64::from_be_bytes(
               self.b[value_at..value_at + 8].try_into().map_err(|_| ())?,
            )))
         },
         0x40 => {
            let (count, data_at) = self.count_at(start, info)?;
            let span = self.object_span(data_at, count)?;
            self.charge_bytes(count)?;
            Ok(Plist::Data(self.b[span].to_vec()))
         },
         0x50 => {
            let (count, string_at) = self.count_at(start, info)?;
            let span = self.object_span(string_at, count)?;
            let text = std::str::from_utf8(&self.b[span]).map_err(|_| ())?;
            if !text.is_ascii() {
               return Err(());
            }
            self.charge_bytes(count)?;
            Ok(Plist::String(text.to_owned()))
         },
         0x60 => {
            let (count, string_at) = self.count_at(start, info)?;
            let byte_count = count.checked_mul(2).ok_or(())?;
            let span = self.object_span(string_at, byte_count)?;
            // Every UTF-16 code unit could encode to three UTF-8 bytes.
            self.charge_bytes(count.checked_mul(3).ok_or(())?)?;
            let mut units = Vec::new();
            units.try_reserve_exact(count).map_err(|_| ())?;
            for bytes in self.b[span].chunks_exact(2) {
               units.push(u16::from_be_bytes(bytes.try_into().map_err(|_| ())?));
            }
            Ok(Plist::String(String::from_utf16(&units).map_err(|_| ())?))
         },
         0x80 => {
            let width = info as usize + 1;
            if width > 8 {
               return Err(());
            }
            let value_at = self
               .object_span(start, 1_usize.checked_add(width).ok_or(())?)?
               .start
               + 1;
            Ok(Plist::Uid(read_uint(self.b, value_at, width).ok_or(())?))
         },
         0xa0 | 0xb0 | 0xc0 => {
            let (count, refs_start) = self.count_at(start, info)?;
            let values = self.collection(refs_start, count, depth)?;
            Ok(match object_type {
               0xa0 => Plist::Array(values),
               0xb0 => Plist::OrderedSet(values),
               0xc0 => Plist::Set(values),
               _ => return Err(()),
            })
         },
         0xd0 => {
            let (count, refs_start) = self.count_at(start, info)?;
            Ok(Plist::Dict(self.dictionary(refs_start, count, depth)?))
         },
         _ => Err(()),
      }
   }
}

/// Parse a complete `bplist00` byte slice into a lossless supported value tree.
///
/// The input must contain a valid header, 32-byte trailer, offset table, and
/// bounded objects. Malformed, oversized, cyclic, or unsupported input returns
/// `None` without partially decoding it.
pub fn parse_bplist(b: &[u8]) -> Option<Plist> {
   if b.get(..HEADER.len())? != HEADER {
      return None;
   }
   let trailer_start = b.len().checked_sub(TRAILER_SIZE)?;
   if trailer_start < HEADER.len() {
      return None;
   }
   let trailer = trailer_at(b, trailer_start)?;
   let mut parser = Parser::new(b, trailer).ok()?;
   parser.object(trailer.top_object, 0).ok()
}

#[cfg(test)]
mod tests {
   use super::*;

   const TSU: &[u8] = include_bytes!("../fixtures/native-mixed.tsudescription");
   const CRL: &[u8] = include_bytes!("../fixtures/native-mixed.crlnative");

   fn plist(objects: &[&[u8]], top_object: usize) -> Vec<u8> {
      assert!(u8::try_from(objects.len()).is_ok());
      let mut output = HEADER.to_vec();
      let mut offsets = Vec::new();
      for object in objects {
         offsets.push(u8::try_from(output.len()).unwrap());
         output.extend_from_slice(object);
      }
      let table_offset = output.len();
      output.extend(offsets);
      output.extend([0; 6]);
      output.extend([1, 1]);
      output.extend_from_slice(&(objects.len() as u64).to_be_bytes());
      output.extend_from_slice(&(top_object as u64).to_be_bytes());
      output.extend_from_slice(&(table_offset as u64).to_be_bytes());
      output
   }

   #[test]
   fn parses_the_tsu_manifest_dict() {
      let plist = parse_bplist(TSU).expect("tsu fixture parses");
      let dict = plist.as_dict().expect("top object is a dict");
      let items = dict
         .get("boardItems")
         .and_then(Plist::as_array)
         .expect("boardItems array");
      assert_eq!(items.len(), 3);
      let first = items[0].as_dict().expect("item dict");
      assert_eq!(first.get("class").and_then(Plist::as_str), Some("Freeform.CRLWPStickyNoteItem"));
   }

   #[test]
   fn locates_a_bounded_plist_trailer() {
      let manifest_len = u64::from_le_bytes(CRL[..8].try_into().unwrap()) as usize;
      let p0 = 8 + manifest_len;
      assert_eq!(&CRL[p0..p0 + 8], HEADER);
      let len = bounded_plist_length(CRL, p0).expect("trailer found");
      assert!(len >= 40 && p0 + len < CRL.len());
      assert!(parse_bplist(&CRL[p0..p0 + len]).is_some());
   }

   #[test]
   fn rejects_incomplete_and_bad_headers() {
      assert_eq!(parse_bplist(b"notaplist00"), None);
      assert_eq!(parse_bplist(b"bpl"), None);
      assert_eq!(parse_bplist(b"bplist00"), None);
   }

   #[test]
   fn retains_negative_integers_and_distinct_uids() {
      assert_eq!(parse_bplist(&plist(&[&[0x10, 0xff]], 0)), Some(Plist::Int(-1)));
      assert_eq!(parse_bplist(&plist(&[&[0x80, 0xff]], 0)), Some(Plist::Uid(255)));
   }

   #[test]
   fn skips_a_trailer_shaped_invalid_prefix() {
      let mut embedded = HEADER.to_vec();
      embedded.push(7); // An invalid offset for the apparent one-object prefix.
      embedded.extend([0; 6]);
      embedded.extend([1, 1]);
      embedded.extend_from_slice(&1_u64.to_be_bytes());
      embedded.extend_from_slice(&0_u64.to_be_bytes());
      embedded.extend_from_slice(&8_u64.to_be_bytes());
      embedded.push(0x08);
      let offset_table = embedded.len();
      embedded.push(41);
      embedded.extend([0; 6]);
      embedded.extend([1, 1]);
      embedded.extend_from_slice(&1_u64.to_be_bytes());
      embedded.extend_from_slice(&0_u64.to_be_bytes());
      embedded.extend_from_slice(&(offset_table as u64).to_be_bytes());
      assert_eq!(bounded_plist_length(&embedded, 0), Some(embedded.len()));
      assert_eq!(parse_bplist(&embedded), Some(Plist::Bool(false)));
   }

   #[test]
   fn rejects_invalid_widths_offsets_and_unknown_markers() {
      let mut invalid_width = plist(&[&[0x08]], 0);
      let trailer = invalid_width.len() - TRAILER_SIZE;
      invalid_width[trailer + 6] = 0;
      assert_eq!(parse_bplist(&invalid_width), None);

      let mut invalid_offset = plist(&[&[0x08]], 0);
      let offset_entry = invalid_offset.len() - TRAILER_SIZE - 1;
      invalid_offset[offset_entry] = 7;
      assert_eq!(parse_bplist(&invalid_offset), None);
      assert_eq!(parse_bplist(&plist(&[&[0x70]], 0)), None);
   }

   #[test]
   fn parses_sets_without_coercing_them_to_arrays() {
      let objects = [&[0xb1, 2][..], &[0xc1, 2][..], &[0x09][..]];
      assert_eq!(
         parse_bplist(&plist(&objects, 0)),
         Some(Plist::OrderedSet(vec![Plist::Bool(true)]))
      );
      assert_eq!(parse_bplist(&plist(&objects, 1)), Some(Plist::Set(vec![Plist::Bool(true)])));
   }

   #[test]
   fn rejects_excessive_shared_reference_expansion() {
      let mut objects = Vec::new();
      for index in 0..18_u8 {
         objects.push(vec![0xa2, index + 1, index + 1]);
      }
      objects.push(vec![0x09]);
      let refs: Vec<&[u8]> = objects.iter().map(Vec::as_slice).collect();
      assert_eq!(parse_bplist(&plist(&refs, 0)), None);
   }

   #[test]
   fn truncated_inputs_never_panic() {
      for cut in 0..TSU.len() {
         let _ = parse_bplist(&TSU[..cut]);
      }
   }
}
