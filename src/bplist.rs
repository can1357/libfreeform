//! Minimal Apple binary property list (`bplist00`) reader.
//!
//! Scoped to the object types that appear in Freeform's `TSUDescription`
//! manifest and the index plist embedded in `CRLNativeData`: dictionaries,
//! arrays, strings, integers, reals, booleans, data, and dates. It is not a
//! general keyed-archive unarchiver. Reads are deliberately lenient —
//! out-of-bounds bytes read as 0, structural damage returns `None` — because
//! real blobs arrive truncated (e.g. partial Universal Clipboard transfers)
//! and a plist parse must never panic.

use std::collections::HashMap;

/// Plain plist value tree. Integers and UIDs parse as `Int`; reals and
/// dates as `Real`.
#[derive(Debug, Clone, PartialEq)]
pub enum Plist {
   Null,
   Bool(bool),
   Int(u64),
   Real(f64),
   String(String),
   Data(Vec<u8>),
   Array(Vec<Self>),
   Dict(HashMap<String, Self>),
}

impl Plist {
   pub fn as_str(&self) -> Option<&str> {
      match self {
         Self::String(s) => Some(s),
         _ => None,
      }
   }

   pub fn as_array(&self) -> Option<&[Self]> {
      match self {
         Self::Array(a) => Some(a),
         _ => None,
      }
   }

   pub const fn as_dict(&self) -> Option<&HashMap<String, Self>> {
      match self {
         Self::Dict(d) => Some(d),
         _ => None,
      }
   }
}

const HEADER: &[u8; 8] = b"bplist00";

/// Total object-parse budget: garbage offset tables (including cyclic
/// object references) abort the parse instead of hanging.
const PARSE_BUDGET: u64 = 250_000;
/// Recursion cap: hostile nesting aborts instead of overflowing the stack.
const MAX_DEPTH: u32 = 512;

/// `b[i]` tolerating any index: out-of-range (including negative) is None.
fn byte_at(b: &[u8], i: i64) -> Option<u8> {
   if i < 0 {
      None
   } else {
      b.get(i as usize).copied()
   }
}

/// Read an unsigned big-endian integer of `size` bytes at `offset`, reading
/// out-of-bounds bytes as 0. Widths beyond 8 bytes wrap — a garbage-only
/// path; well-formed plists never emit them.
fn read_uint(b: &[u8], offset: i64, size: u64) -> u64 {
   let mut value: u64 = 0;
   for i in 0..size.min(u32::MAX as u64) {
      let byte = offset
         .checked_add(i as i64)
         .and_then(|at| byte_at(b, at))
         .unwrap_or(0);
      value = value.wrapping_mul(256).wrapping_add(byte as u64);
   }
   value
}

fn read_f32_be(b: &[u8], offset: usize) -> Option<f64> {
   let bytes: [u8; 4] = b.get(offset..offset + 4)?.try_into().ok()?;
   Some(f32::from_be_bytes(bytes) as f64)
}

fn read_f64_be(b: &[u8], offset: usize) -> Option<f64> {
   let bytes: [u8; 8] = b.get(offset..offset + 8)?.try_into().ok()?;
   Some(f64::from_be_bytes(bytes))
}

/// Locate a bounded `bplist00`'s length by its 32-byte trailer.
///
/// The index plist inside `CRLNativeData` does NOT run to EOF, so the trailer
/// must be found rather than read from the end of the blob. `p0` is the
/// offset of the `bplist00` header in `data`. `None` when no plausible
/// trailer exists.
pub fn bounded_plist_length(data: &[u8], p0: usize) -> Option<usize> {
   if data.len() < 32 {
      return None;
   }
   let mut p = p0.checked_add(8)?;
   while p <= data.len() - 32 {
      if data[p..p + 5].iter().any(|&c| c != 0) {
         p += 1;
         continue;
      }
      let offset_int_size = data[p + 6];
      let object_ref_size = data[p + 7];
      if !(1..=8).contains(&offset_int_size) || !(1..=8).contains(&object_ref_size) {
         p += 1;
         continue;
      }
      let num_objects = read_uint(data, (p + 8) as i64, 8);
      let top_object = read_uint(data, (p + 16) as i64, 8);
      let offset_table_offset = read_uint(data, (p + 24) as i64, 8);
      let length = (p + 32 - p0) as u64;
      if num_objects > 0
         && num_objects < 1_000_000
         && top_object < num_objects
         && offset_table_offset >= 8
         && offset_table_offset < length
      {
         return Some(length as usize);
      }
      p += 1;
   }
   None
}

struct Parser<'a> {
   b:                   &'a [u8],
   object_ref_size:     u64,
   offset_int_size:     u64,
   offset_table_offset: u64,
   num_objects:         u64,
   budget:              u64,
}

impl Parser<'_> {
   /// Offset-table entry for `index`; `None` when `index` is beyond the
   /// table.
   fn offset_at(&self, index: u64) -> Option<i64> {
      if index >= self.num_objects {
         return None;
      }
      let at = (self.offset_table_offset as u128 + index as u128 * self.offset_int_size as u128)
         .min(i64::MAX as u128) as i64;
      Some(read_uint(self.b, at, self.offset_int_size).min(i64::MAX as u64) as i64)
   }

   /// Object count, resolving the `0x_F` escape to a following int object.
   fn count_at(&self, pos: i64, info: u8) -> (u64, i64) {
      if info != 0x0f {
         return (info as u64, pos + 1);
      }
      // Int marker 0x1_, low nibble = log2(bytes); the high nibble is not
      // verified — real encoders always emit an int object here.
      let size_pow = byte_at(self.b, pos + 1).unwrap_or(0);
      let int_bytes = 1u64 << (size_pow & 0x0f);
      (read_uint(self.b, pos + 2, int_bytes), pos.saturating_add(2 + int_bytes as i64))
   }

   /// Parse the object at table `index`. `Err(())` aborts the whole parse
   /// (float read past the buffer, exhausted budget or depth).
   fn object(&mut self, index: u64, depth: u32) -> Result<Plist, ()> {
      if depth > MAX_DEPTH || self.budget == 0 {
         return Err(());
      }
      self.budget -= 1;
      let Some(start) = self.offset_at(index) else {
         return Ok(Plist::Null);
      };
      let marker = byte_at(self.b, start).unwrap_or(0);
      let type_nibble = marker & 0xf0;
      let info = marker & 0x0f;
      let len = self.b.len() as i64;

      Ok(match type_nibble {
         0x00 => match marker {
            0x08 => Plist::Bool(false),
            0x09 => Plist::Bool(true),
            _ => Plist::Null,
         },
         0x10 => Plist::Int(read_uint(self.b, start + 1, 1u64 << info)),
         0x20 => {
            // Float object of `1 << info` bytes; one that runs past the
            // buffer or is under 4 bytes aborts the parse.
            let size = 1i64 << info;
            if start + 1 + size > len {
               return Err(());
            }
            if info == 3 {
               Plist::Real(read_f64_be(self.b, (start + 1) as usize).ok_or(())?)
            } else if size >= 4 {
               Plist::Real(read_f32_be(self.b, (start + 1) as usize).ok_or(())?)
            } else {
               return Err(());
            }
         },
         0x30 => Plist::Real(read_f64_be(self.b, (start + 1) as usize).ok_or(())?),
         0x40 => {
            let (count, data_start) = self.count_at(start, info);
            let from = data_start.clamp(0, len) as usize;
            let to = data_start
               .saturating_add(count.min(i64::MAX as u64) as i64)
               .clamp(0, len) as usize;
            Plist::Data(self.b[from..to.max(from)].to_vec())
         },
         0x50 => {
            // ASCII (really Latin-1: each byte maps to the same code
            // point). Bytes past the buffer end the string.
            let (count, data_start) = self.count_at(start, info);
            let mut s = String::new();
            for i in 0..count {
               let Some(c) = byte_at(self.b, data_start.saturating_add(i as i64)) else {
                  break;
               };
               s.push(c as char);
            }
            Plist::String(s)
         },
         0x60 => {
            // UTF-16BE code units; invalid surrogates become U+FFFD.
            let (count, data_start) = self.count_at(start, info);
            let mut units: Vec<u16> = Vec::new();
            for i in 0..count {
               let at = data_start.saturating_add(i as i64 * 2);
               if byte_at(self.b, at).is_none() {
                  break;
               }
               units.push(read_uint(self.b, at, 2) as u16);
            }
            Plist::String(String::from_utf16_lossy(&units))
         },
         0x80 => Plist::Int(read_uint(self.b, start + 1, info as u64 + 1)),
         0xa0 => {
            let (count, data_start) = self.count_at(start, info);
            let mut out: Vec<Plist> = Vec::new();
            for i in 0..count {
               let at = data_start.saturating_add((i * self.object_ref_size) as i64);
               let child = self.object(read_uint(self.b, at, self.object_ref_size), depth + 1)?;
               out.push(child);
            }
            Plist::Array(out)
         },
         0xd0 => {
            let (count, data_start) = self.count_at(start, info);
            let value_base = data_start
               .saturating_add((count.min(i64::MAX as u64 / 256) * self.object_ref_size) as i64);
            let mut out: HashMap<String, Plist> = HashMap::new();
            for i in 0..count {
               let key_at = data_start.saturating_add((i * self.object_ref_size) as i64);
               let value_at = value_base.saturating_add((i * self.object_ref_size) as i64);
               let key = self.object(read_uint(self.b, key_at, self.object_ref_size), depth + 1)?;
               let value =
                  self.object(read_uint(self.b, value_at, self.object_ref_size), depth + 1)?;
               if let Plist::String(key) = key {
                  out.insert(key, value);
               }
            }
            Plist::Dict(out)
         },
         _ => Plist::Null,
      })
   }
}

/// Parse a `bplist00` byte slice (offset 0 must be the header) into plain
/// values. `None` on a bad header or structural damage (malformed floats,
/// exhausted parse budget).
pub fn parse_bplist(b: &[u8]) -> Option<Plist> {
   if b.len() < HEADER.len() || &b[..HEADER.len()] != HEADER {
      return None;
   }
   let trailer = b.len() as i64 - 32;
   let offset_int_size = byte_at(b, trailer + 6).unwrap_or(1) as u64;
   let object_ref_size = byte_at(b, trailer + 7).unwrap_or(1) as u64;
   let num_objects = read_uint(b, trailer + 8, 8);
   let top_object = read_uint(b, trailer + 16, 8);
   let offset_table_offset = read_uint(b, trailer + 24, 8);

   let mut parser = Parser {
      b,
      object_ref_size,
      offset_int_size,
      offset_table_offset,
      num_objects,
      budget: PARSE_BUDGET,
   };
   parser.object(top_object, 0).ok()
}

#[cfg(test)]
mod tests {
   use super::*;

   const TSU: &[u8] = include_bytes!("../fixtures/native-mixed.tsudescription");
   const CRL: &[u8] = include_bytes!("../fixtures/native-mixed.crlnative");

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
   fn rejects_a_bad_header() {
      assert_eq!(parse_bplist(b"notaplist00"), None);
      assert_eq!(parse_bplist(b"bpl"), None);
   }

   #[test]
   fn locates_a_bounded_plist_trailer() {
      // The crlnative fixture embeds a bounded index plist after the 8-byte
      // manifest-length prefix + manifest.
      let manifest_len = u64::from_le_bytes(CRL[..8].try_into().unwrap()) as usize;
      let p0 = 8 + manifest_len;
      assert_eq!(&CRL[p0..p0 + 8], b"bplist00");
      let len = bounded_plist_length(CRL, p0).expect("trailer found");
      assert!(len >= 40 && p0 + len < CRL.len());
      assert!(parse_bplist(&CRL[p0..p0 + len]).is_some());
   }

   #[test]
   fn truncated_inputs_never_panic() {
      for cut in 0..TSU.len() {
         let _ = parse_bplist(&TSU[..cut]);
      }
   }
}
