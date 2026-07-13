//! Decoder for `com.apple.freeform.CRLNativeData` (+ the `TSUDescription`
//! manifest) — Freeform's full native object graph.
//!
//! The format is reverse-engineered (see `docs/FORMAT.md` §3.4 in the
//! repository) and OS-version fragile.
//!
//! The top-level `CRLNative` manifest is well-formed protobuf and is decoded
//! with the strict wire walker from `pkdrawing`. The object archive is
//! CRDT-interleaved, so the load-bearing data for text, colors, geometry,
//! and native paths is recovered by byte offset: `scan_texts`/`scan_colors`/
//! `scan_geo_points`/`scan_vertex_pattern` and `decode_item_geometry` are
//! byte-offset heuristics.
//!
//! Verified payloads recover board-item classes/order, per-item geometry
//! (frame), fills, and TSWP text. Geometry comes from a TSD-style
//! `GeometryArchive` whose position/size are consecutive
//! `22 0a 0d <x> 15 <y>` Points.

use std::collections::HashMap;

use super::{
   bplist::{Plist, bounded_plist_length, parse_bplist},
   pkdrawing::{WireValue, to_hex, walk_message},
   types::{
      FreeformBoardItem, FreeformDecodeError, FreeformFrame, FreeformInkPoint, FreeformInkStroke,
      FreeformNative,
   },
};

/// The three regions of a `CRLNativeData` blob (`docs/FORMAT.md` §3.4 A/B/C).
pub struct Sections<'a> {
   pub manifest:  &'a [u8],
   pub index:     HashMap<String, Plist>,
   pub section_c: &'a [u8],
}

/// One `TSUDescription` board-item entry: class plus cheap routing hints.
/// Hint values are stringified plist scalars (`true`/`false`, decimal ints,
/// strings as-is; null/data/array/dict collapse to `""`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
   feature = "serde",
   derive(serde::Serialize, serde::Deserialize),
   serde(rename_all = "camelCase")
)]
pub struct TsuEntry {
   pub class_name: String,
   pub hints:      HashMap<String, String>,
}

const BPLIST_HEADER: &[u8; 8] = b"bplist00";

fn read_u64_le(b: &[u8], offset: usize) -> Option<u64> {
   let bytes: [u8; 8] = b.get(offset..offset + 8)?.try_into().ok()?;
   Some(u64::from_le_bytes(bytes))
}

/// Lenient LEB128 read: stops at the buffer end, returns (value, next
/// offset). Wraps on absurd varints — a garbage-only path; real records
/// never encode them.
fn read_varint(b: &[u8], offset: usize) -> (u64, usize) {
   let mut value: u64 = 0;
   let mut scale: u64 = 1;
   let mut i = offset;
   while i < b.len() {
      let c = b[i];
      i += 1;
      value = value.wrapping_add(((c & 0x7f) as u64).wrapping_mul(scale));
      if c & 0x80 == 0 {
         return (value, i);
      }
      scale = scale.wrapping_mul(128);
   }
   (value, i)
}

fn f32_at(b: &[u8], offset: usize) -> Option<f32> {
   let bytes: [u8; 4] = b.get(offset..offset + 4)?.try_into().ok()?;
   Some(f32::from_le_bytes(bytes))
}

/// Split a `CRLNativeData` blob into its manifest, index plist, and object
/// archive.
pub fn split_sections(data: &[u8]) -> Result<Sections<'_>, FreeformDecodeError> {
   let manifest_len = read_u64_le(data, 0)
      .ok_or_else(|| FreeformDecodeError("CRLNativeData: truncated manifest length".into()))?;
   let manifest_end = (8u128 + manifest_len as u128).min(data.len() as u128) as usize;
   let manifest = &data[8.min(data.len())..manifest_end];
   let p0 = (8u128 + manifest_len as u128).min(usize::MAX as u128) as usize;
   if data.get(p0..p0 + BPLIST_HEADER.len()) != Some(BPLIST_HEADER.as_slice()) {
      return Err(FreeformDecodeError(format!("CRLNativeData: expected bplist at {p0}")));
   }
   let plen = bounded_plist_length(data, p0)
      .ok_or_else(|| FreeformDecodeError("bplist: could not locate trailer".into()))?;
   let index = parse_bplist(&data[p0..p0 + plen])
      .ok_or_else(|| FreeformDecodeError("CRLNativeData: invalid index plist".into()))?;
   Ok(Sections { manifest, index: as_record(index), section_c: &data[p0 + plen..] })
}

fn as_record(value: Plist) -> HashMap<String, Plist> {
   match value {
      Plist::Dict(d) => d,
      _ => HashMap::new(),
   }
}

/// Count top-level board items from the manifest (repeated field 3). Parse
/// damage returns 0.
pub fn manifest_item_count(manifest: &[u8]) -> usize {
   let mut count = 0usize;
   let ok = walk_message(manifest, &mut |field, value| {
      if field == 3 {
         match value {
            WireValue::Bytes(_) => count += 1,
            // `repeated bytes` only accepts length-delimited records;
            // any other wire type is structural damage.
            _ => return Err(()),
         }
      }
      Ok(())
   });
   if ok.is_ok() { count } else { 0 }
}

// TSWP-shaped strings that are structural bookkeeping, not user text.
const NOISE: &[&str] = &[
   "commonCRDTData",
   "specificCRDTData",
   "capsuleData",
   "crdt",
   "bcrdt",
   "dcrdt",
   "fcrdt",
   "thumbnail", // asset reference, not text
   "image",     // asset reference marker; real text is carried by TSWP text storage
];

// Asset filenames (`<uuid>.jpg`), shape presets (`Parallelogram_950`), font
// ids, and bare UUIDs are graph identifiers, not user text.
const ASSET_EXTENSIONS: &[&str] = &[
   "jpg", "jpeg", "png", "heic", "heif", "gif", "webp", "mov", "mp4", "m4v", "pdf", "tif", "tiff",
];

fn is_asset_file(text: &str) -> bool {
   let Some(dot) = text.rfind('.') else {
      return false;
   };
   let ext = &text[dot + 1..];
   ASSET_EXTENSIONS.iter().any(|e| ext.eq_ignore_ascii_case(e))
}

/// `^[A-Za-z][A-Za-z0-9]*_\d+$`
fn is_preset_name(text: &str) -> bool {
   let Some(under) = text.find('_') else {
      return false;
   };
   let (head, tail) = (&text[..under], &text[under + 1..]);
   let mut head_bytes = head.bytes();
   let Some(first) = head_bytes.next() else {
      return false;
   };
   first.is_ascii_alphabetic()
      && head_bytes.all(|b| b.is_ascii_alphanumeric())
      && !tail.is_empty()
      && tail.bytes().all(|b| b.is_ascii_digit())
}

/// `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-`i
fn is_uuid_like(text: &str) -> bool {
   let b = text.as_bytes();
   if b.len() < 19 {
      return false;
   }
   let hex = |r: std::ops::Range<usize>| b[r].iter().all(|c| c.is_ascii_hexdigit());
   hex(0..8) && b[8] == b'-' && hex(9..13) && b[13] == b'-' && hex(14..18) && b[18] == b'-'
}

fn is_graph_identifier(text: &str) -> bool {
   NOISE.contains(&text)
      || text.starts_with("com.apple")
      || is_asset_file(text)
      || is_preset_name(text)
      || is_uuid_like(text)
      || !text.bytes().any(|b| b.is_ascii_alphanumeric())
}

struct TextHit {
   offset: usize,
   text:   String,
}

/// Heuristic TSWP text-storage extraction: a `0x0a <len> <utf8>` run followed
/// by a plausible field tag.
///
/// Catches cell/label/note/text-box content; filters CRDT markers and
/// single-byte style-run false positives.
pub fn tswp_strings(data: &[u8]) -> Vec<String> {
   scan_texts(data).into_iter().map(|t| t.text).collect()
}

fn scan_texts(data: &[u8]) -> Vec<TextHit> {
   let mut found = Vec::new();
   for m in 0..data.len() {
      if data[m] != 0x0a {
         continue;
      }
      let (len, j) = read_varint(data, m + 1);
      if !(2..=4096).contains(&len) {
         continue;
      }
      let len = len as usize;
      let Some(slice) = data.get(j..j + len) else {
         continue;
      };
      let next = data.get(j + len).copied().unwrap_or(0);
      let printable = slice.iter().all(|&c| (32..127).contains(&c) || c >= 0x80);
      if !printable || !matches!(next & 7, 0 | 2 | 5) {
         continue;
      }
      let Ok(text) = std::str::from_utf8(slice) else {
         continue;
      };
      if is_graph_identifier(text) {
         continue;
      }
      found.push(TextHit { offset: m, text: text.to_owned() });
   }
   found
}

struct ColorHit {
   offset: usize,
   hex:    String,
}

/// Every sRGB color: three consecutive `#15 fixed32` channels (tag byte 0x7d)
/// within 12 bytes.
pub fn srgb_colors(data: &[u8]) -> Vec<String> {
   scan_colors(data).into_iter().map(|c| c.hex).collect()
}

fn scan_colors(data: &[u8]) -> Vec<ColorHit> {
   let mut channels: Vec<(usize, f64)> = Vec::new();
   for m in 0..data.len() {
      if data[m] != 0x7d {
         continue;
      }
      let Some(v) = f32_at(data, m + 1) else {
         continue;
      };
      let v = v as f64;
      if (-0.01..=1.01).contains(&v) {
         channels.push((m, (v * 10000.0).round() / 10000.0));
      }
   }
   let mut colors = Vec::new();
   let mut group: Vec<(usize, f64)> = Vec::new();
   let flush = |group: &[(usize, f64)], colors: &mut Vec<ColorHit>| {
      if group.len() >= 3 {
         colors.push(ColorHit {
            offset: group[0].0,
            hex:    to_hex(group[0].1, group[1].1, group[2].1),
         });
      }
   };
   for channel in channels {
      match group.last() {
         Some(prev) if channel.0 - prev.0 <= 12 => group.push(channel),
         _ => {
            flush(&group, &mut colors);
            group = vec![channel];
         },
      }
   }
   flush(&group, &mut colors);
   colors
}

fn hint_string(value: &Plist) -> String {
   match value {
      Plist::Bool(true) => "true".into(),
      Plist::Bool(false) => "false".into(),
      Plist::Int(i) => i.to_string(),
      Plist::Real(r) => r.to_string(),
      Plist::String(s) => s.clone(),
      Plist::Null | Plist::Data(_) | Plist::Array(_) | Plist::Dict(_) => String::new(),
   }
}

/// Parse the `TSUDescription` manifest into per-item classes + routing hints.
/// Errors on an unparseable bplist.
pub fn parse_tsu_description(data: &[u8]) -> Result<Vec<TsuEntry>, FreeformDecodeError> {
   let plist = parse_bplist(data)
      .ok_or_else(|| FreeformDecodeError("TSUDescription: invalid bplist".into()))?;
   let Plist::Dict(dict) = plist else {
      return Ok(Vec::new());
   };
   let Some(board_items) = dict.get("boardItems").and_then(Plist::as_array) else {
      return Ok(Vec::new());
   };
   Ok(board_items
      .iter()
      .map(|item| {
         let Some(record) = item.as_dict() else {
            return TsuEntry { class_name: "?".into(), hints: HashMap::new() };
         };
         let class_name = record
            .get("class")
            .and_then(Plist::as_str)
            .map_or_else(|| "?".into(), |c| c.replacen("Freeform.", "", 1));
         let hints = record
            .iter()
            .filter(|(key, _)| key.as_str() != "class")
            .map(|(key, value)| (key.clone(), hint_string(value)))
            .collect();
         TsuEntry { class_name, hints }
      })
      .collect())
}

#[derive(Clone, Copy)]
struct GeoPoint {
   offset: usize,
   x:      f64,
   y:      f64,
}

// A GeometryArchive Point is `22 0a 0d <x:f32> 15 <y:f32>` (field4 ->
// Point{x,y}), 12 bytes wide. Position and size are two such Points back to
// back (stride 14).
const GEO_POINT_STRIDE: usize = 14;

fn is_geo_point_at(data: &[u8], offset: usize) -> bool {
   data.get(offset) == Some(&0x22)
      && data.get(offset + 1) == Some(&0x0a)
      && data.get(offset + 2) == Some(&0x0d)
      && data.get(offset + 7) == Some(&0x15)
}

fn scan_geo_points(data: &[u8]) -> Vec<GeoPoint> {
   let mut points = Vec::new();
   let mut i = 0usize;
   while i + 12 <= data.len() {
      if is_geo_point_at(data, i) {
         points.push(GeoPoint {
            offset: i,
            x:      f32_at(data, i + 3).unwrap_or(0.0) as f64,
            y:      f32_at(data, i + 8).unwrap_or(0.0) as f64,
         });
         i += 12;
      } else {
         i += 1;
      }
   }
   points
}

#[derive(Clone, Copy)]
struct OutlineHit {
   offset: usize,
   x:      f64,
   y:      f64,
}

// Native shape paths store the first vertex as `0a 0a 12 08 <x><y>` and
// subsequent vertices as `0a 0c 08 01 12 08 <x><y>` in the captured CRL graph.
const FIRST_VERTEX: &[u8] = &[0x0a, 0x0a, 0x12, 0x08];
const NEXT_VERTEX: &[u8] = &[0x0a, 0x0c, 0x08, 0x01, 0x12, 0x08];

// Native freehand records store stroke-local samples plus a per-stroke
// geometry origin. PKDrawing stays the canonical source when present; this
// path covers Freeform selections that omit `com.apple.drawing`.
const INK_MARKER: &[u8] = b"com.apple.ink.";
const NATIVE_INK_FIRST_POINT: &[u8] = &[0x0a, 0x0a, 0x12, 0x08];
const DEFAULT_NATIVE_INK_WIDTH: f64 = 2.0;

struct NativeInkItem {
   frame:   FreeformFrame,
   strokes: Vec<FreeformInkStroke>,
}

struct NativeInkStrokeDraft {
   ink_type: String,
   points:   Vec<FreeformInkPoint>,
}

struct NativeStrokeGeometry {
   origin_x: f64,
   origin_y: f64,
   next:     usize,
}

struct NativePointContainer {
   start: usize,
   end:   usize,
}

fn decode_native_ink_items(data: &[u8], uuids: &[String]) -> Vec<Option<NativeInkItem>> {
   let mut records: Vec<(usize, usize)> = Vec::new(); // (item_index, offset)
   for (item_index, uuid) in uuids.iter().enumerate() {
      let needle = uuid_to_bytes(uuid);
      let mut offset = 0usize;
      while offset < data.len() {
         let Some(found) = index_of_bytes_from(data, &needle, offset) else {
            break;
         };
         records.push((item_index, found));
         offset = found + needle.len();
      }
   }
   records.sort_by_key(|r| r.1);

   let mut drafts_by_item: Vec<Vec<NativeInkStrokeDraft>> =
      uuids.iter().map(|_| Vec::new()).collect();
   for i in 0..records.len() {
      let (item_index, record_offset) = records[i];
      let end = records.get(i + 1).map_or(data.len(), |r| r.1);
      let Some(marker) = index_of_bytes_from(data, INK_MARKER, record_offset) else {
         continue;
      };
      if marker >= end {
         continue;
      }
      let Some(geometry) = find_native_stroke_geometry(data, marker, end) else {
         continue;
      };
      let Some(container) = find_native_point_container(data, geometry.next, end) else {
         continue;
      };
      let points = parse_native_ink_points(data, &container, &geometry);
      if points.len() < 2 {
         continue;
      }
      drafts_by_item[item_index]
         .push(NativeInkStrokeDraft { ink_type: native_ink_type_at(data, marker), points });
   }

   drafts_by_item
      .into_iter()
      .map(native_ink_item_from_drafts)
      .collect()
}

fn native_ink_item_from_drafts(drafts: Vec<NativeInkStrokeDraft>) -> Option<NativeInkItem> {
   if drafts.is_empty() {
      return None;
   }
   let mut min_x = f64::INFINITY;
   let mut min_y = f64::INFINITY;
   let mut max_x = f64::NEG_INFINITY;
   let mut max_y = f64::NEG_INFINITY;
   for draft in &drafts {
      for point in &draft.points {
         min_x = min_x.min(point.x);
         min_y = min_y.min(point.y);
         max_x = max_x.max(point.x);
         max_y = max_y.max(point.y);
      }
   }
   if !min_x.is_finite() {
      return None;
   }

   Some(NativeInkItem {
      frame:   FreeformFrame {
         x:        min_x,
         y:        min_y,
         w:        max_x - min_x,
         h:        max_y - min_y,
         rotation: 0.0,
      },
      strokes: drafts
         .into_iter()
         .map(|draft| FreeformInkStroke {
            ink_type: draft.ink_type,
            color:    "#000000".into(),
            opacity:  1.0,
            points:   draft
               .points
               .into_iter()
               .map(|point| FreeformInkPoint {
                  x:     point.x - min_x,
                  y:     point.y - min_y,
                  force: point.force,
                  width: point.width,
               })
               .collect(),
         })
         .collect(),
   })
}

fn native_ink_type_at(data: &[u8], marker: usize) -> String {
   let start = marker + INK_MARKER.len();
   let mut end = start;
   while let Some(&c) = data.get(end) {
      if !c.is_ascii_lowercase() {
         break;
      }
      end += 1;
   }
   let suffix = String::from_utf8_lossy(&data[start.min(data.len())..end]).into_owned();
   match suffix.as_str() {
      "marker" | "pencil" | "pen" => suffix,
      "" => "pen".into(),
      _ => suffix,
   }
}

fn find_native_stroke_geometry(
   data: &[u8],
   start: usize,
   end: usize,
) -> Option<NativeStrokeGeometry> {
   let mut i = start;
   while i + GEO_POINT_STRIDE + 12 <= end {
      if !is_geo_point_at(data, i) || !is_geo_point_at(data, i + GEO_POINT_STRIDE) {
         i += 1;
         continue;
      }
      let width = f32_at(data, i + GEO_POINT_STRIDE + 3).unwrap_or(f32::NAN) as f64;
      let height = f32_at(data, i + GEO_POINT_STRIDE + 8).unwrap_or(f32::NAN) as f64;
      if !width.is_finite()
         || !height.is_finite()
         || !(0.0..=10000.0).contains(&width)
         || !(0.0..=10000.0).contains(&height)
      {
         i += 1;
         continue;
      }
      return Some(NativeStrokeGeometry {
         origin_x: f32_at(data, i + 3).unwrap_or(0.0) as f64,
         origin_y: f32_at(data, i + 8).unwrap_or(0.0) as f64,
         next:     i + GEO_POINT_STRIDE + 12,
      });
   }
   None
}

fn find_native_point_container(
   data: &[u8],
   start: usize,
   end: usize,
) -> Option<NativePointContainer> {
   for i in start..end.min(data.len()) {
      if data[i] != 0x22 {
         continue;
      }
      let (length, payload_start) = read_varint(data, i + 1);
      let payload_end = (payload_start as u128 + length as u128).min(usize::MAX as u128) as usize;
      if payload_start <= i + 1 || payload_end > end || payload_end <= payload_start {
         continue;
      }
      if data.get(payload_start..payload_start + NATIVE_INK_FIRST_POINT.len())
         != Some(NATIVE_INK_FIRST_POINT)
      {
         continue;
      }
      return Some(NativePointContainer { start: payload_start, end: payload_end });
   }
   None
}

fn parse_native_ink_points(
   data: &[u8],
   container: &NativePointContainer,
   geometry: &NativeStrokeGeometry,
) -> Vec<FreeformInkPoint> {
   let mut points = Vec::new();
   let mut offset = container.start;
   while offset < container.end {
      let (tag, tag_end) = read_varint(data, offset);
      if tag != 0x0a {
         break;
      }
      let (length, body_start) = read_varint(data, tag_end);
      let body_end = (body_start as u128 + length as u128).min(usize::MAX as u128) as usize;
      if body_end > container.end || body_end <= body_start {
         break;
      }
      push_native_ink_point(&mut points, data, body_start, length, geometry);
      offset = body_end;
   }
   points
}

fn push_native_ink_point(
   points: &mut Vec<FreeformInkPoint>,
   data: &[u8],
   body_start: usize,
   length: u64,
   geometry: &NativeStrokeGeometry,
) {
   let at = |i: usize| data.get(body_start + i).copied();
   let point_offset = if length == 10 && at(0) == Some(0x12) && at(1) == Some(0x08) {
      body_start + 2
   } else if length == 24
      && at(0) == Some(0x08)
      && at(1) == Some(0x02)
      && at(2) == Some(0x12)
      && at(3) == Some(0x08)
   {
      body_start + 4
   } else {
      return;
   };
   if point_offset + 8 > data.len() {
      return;
   }
   let local_x = f32_at(data, point_offset).unwrap_or(f32::NAN) as f64;
   let local_y = f32_at(data, point_offset + 4).unwrap_or(f32::NAN) as f64;
   if !local_x.is_finite() || !local_y.is_finite() {
      return;
   }
   if !(-10000.0..=10000.0).contains(&local_x) || !(-10000.0..=10000.0).contains(&local_y) {
      return;
   }
   points.push(FreeformInkPoint {
      x:     geometry.origin_x + local_x,
      y:     geometry.origin_y + local_y,
      force: 0.5,
      width: DEFAULT_NATIVE_INK_WIDTH,
   });
}

fn scan_outline_hits(data: &[u8]) -> Vec<OutlineHit> {
   let mut hits = Vec::new();
   scan_vertex_pattern(data, FIRST_VERTEX, &mut hits);
   scan_vertex_pattern(data, NEXT_VERTEX, &mut hits);
   hits.sort_by_key(|h| h.offset);
   hits
}

fn scan_vertex_pattern(data: &[u8], pattern: &[u8], out: &mut Vec<OutlineHit>) {
   if data.len() < pattern.len() + 8 {
      return;
   }
   for i in 0..=data.len() - pattern.len() - 8 {
      if &data[i..i + pattern.len()] != pattern {
         continue;
      }
      let x = f32_at(data, i + pattern.len()).unwrap_or(f32::NAN) as f64;
      let y = f32_at(data, i + pattern.len() + 4).unwrap_or(f32::NAN) as f64;
      if (-10.0..=5000.0).contains(&x) && (-10.0..=5000.0).contains(&y) {
         out.push(OutlineHit { offset: i, x, y });
      }
   }
}

fn dedupe_outline(points: &[(f64, f64)]) -> Vec<(f64, f64)> {
   let mut seen = std::collections::HashSet::new();
   let mut out = Vec::new();
   for &(x, y) in points {
      let key = ((x * 1000.0).round() as i64, (y * 1000.0).round() as i64);
      if seen.insert(key) {
         out.push((x, y));
      }
   }
   out
}

struct GeoBlock {
   offset: usize,
   frame:  FreeformFrame,
}

const DEG_TO_RAD: f64 = std::f64::consts::PI / 180.0;

/// A geometry block = position Point + size Point + optional rotation f32.
fn build_geo_blocks(data: &[u8], points: &[GeoPoint]) -> Vec<GeoBlock> {
   let mut blocks = Vec::new();
   let mut k = 0usize;
   while k < points.len() {
      let pos = points[k];
      let Some(size) = points.get(k + 1) else {
         break;
      };
      if size.offset == pos.offset + GEO_POINT_STRIDE {
         let rot_at = size.offset + 12;
         // A rotation marker whose f32 is truncated reads as 0.
         let rotation = if data.get(rot_at) == Some(&0x12)
            && data.get(rot_at + 1) == Some(&0x05)
            && data.get(rot_at + 2) == Some(&0x7d)
         {
            -(f32_at(data, rot_at + 3).unwrap_or(0.0) as f64) * DEG_TO_RAD
         } else {
            0.0
         };
         blocks.push(GeoBlock {
            offset: pos.offset,
            frame:  FreeformFrame { x: pos.x, y: pos.y, w: size.x, h: size.y, rotation },
         });
         k += 1; // consume the size Point so it cannot start another block
      }
      k += 1;
   }
   blocks
}

/// Two hex digits per byte, dashes skipped; invalid digits read as 0.
fn uuid_to_bytes(uuid: &str) -> [u8; 16] {
   let hex: Vec<u8> = uuid.bytes().filter(|&b| b != b'-').collect();
   let digit = |i: usize| hex.get(i).and_then(|&b| (b as char).to_digit(16));
   let mut bytes = [0u8; 16];
   for (i, byte) in bytes.iter_mut().enumerate() {
      *byte = match (digit(i * 2), digit(i * 2 + 1)) {
         (Some(hi), Some(lo)) => (hi * 16 + lo) as u8,
         (Some(hi), None) => hi as u8,
         _ => 0,
      };
   }
   bytes
}

fn index_of_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
   index_of_bytes_from(haystack, needle, 0)
}

fn index_of_bytes_from(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
   if needle.is_empty() || haystack.len() < needle.len() {
      return if from <= haystack.len() && needle.is_empty() {
         Some(from)
      } else {
         None
      };
   }
   (from..=haystack.len() - needle.len()).find(|&i| &haystack[i..i + needle.len()] == needle)
}

struct ItemGeometry {
   frame:   Option<FreeformFrame>,
   fill:    Option<String>,
   text:    Option<String>,
   outline: Option<Vec<(f64, f64)>>,
}

/// Resolve per-item geometry, fill, and text from the object archive.
///  - Frame: each item owns the geometry block with the greatest offset before
///    its UUID marker (assigned in item order), which skips a container's
///    nested child geometry (e.g. a table's embedded shapes).
///  - Fill/text: associated to the nearest item UUID by byte offset.
fn decode_item_geometry(section_c: &[u8], uuids: &[String]) -> Vec<ItemGeometry> {
   let uuid_offsets: Vec<Option<usize>> = uuids
      .iter()
      .map(|u| index_of_bytes(section_c, &uuid_to_bytes(u)))
      .collect();
   let points = scan_geo_points(section_c);
   let blocks = build_geo_blocks(section_c, &points);
   let mut point_offsets_in_blocks = std::collections::HashSet::new();
   for block in &blocks {
      point_offsets_in_blocks.insert(block.offset);
      point_offsets_in_blocks.insert(block.offset + GEO_POINT_STRIDE);
   }
   let standalone_points: Vec<GeoPoint> = points
      .iter()
      .filter(|p| !point_offsets_in_blocks.contains(&p.offset))
      .copied()
      .collect();
   let mut used_standalone = vec![false; standalone_points.len()];
   let mut used = vec![false; blocks.len()];
   let mut frames: Vec<Option<FreeformFrame>> = vec![None; uuids.len()];

   for i in 0..uuids.len() {
      let Some(u) = uuid_offsets[i] else {
         continue;
      };
      let mut best_k: Option<usize> = None;
      let mut best_offset: Option<usize> = None;
      for (k, block) in blocks.iter().enumerate() {
         if used[k] || block.offset >= u || best_offset.is_some_and(|b| block.offset <= b) {
            continue;
         }
         best_offset = Some(block.offset);
         best_k = Some(k);
      }
      if let Some(k) = best_k {
         used[k] = true;
         frames[i] = Some(blocks[k].frame);
      }
   }

   // Empty text boxes often store only a placement point, not a size block.
   // Give them a conservative default frame so the placeholder/import remains
   // editable.
   for i in 0..uuids.len() {
      if frames[i].is_some() {
         continue;
      }
      let Some(u) = uuid_offsets[i] else {
         continue;
      };
      let mut best_k: Option<usize> = None;
      let mut best_offset: Option<usize> = None;
      for (k, point) in standalone_points.iter().enumerate() {
         if used_standalone[k]
            || point.offset >= u
            || best_offset.is_some_and(|b| point.offset <= b)
         {
            continue;
         }
         best_offset = Some(point.offset);
         best_k = Some(k);
      }
      if let Some(k) = best_k {
         used_standalone[k] = true;
         let point = standalone_points[k];
         frames[i] = Some(FreeformFrame {
            x:        point.x,
            y:        point.y,
            w:        160.0,
            h:        40.0,
            rotation: 0.0,
         });
      }
   }

   let nearest_item = |offset: usize| -> Option<usize> {
      let mut best: Option<usize> = None;
      let mut best_dist = u64::MAX;
      for (j, u) in uuid_offsets.iter().enumerate() {
         let Some(u) = u else {
            continue;
         };
         let dist = offset.abs_diff(*u) as u64;
         if dist < best_dist {
            best_dist = dist;
            best = Some(j);
         }
      }
      best
   };

   let mut fills_by_item: Vec<Vec<String>> = uuids.iter().map(|_| Vec::new()).collect();
   for color in scan_colors(section_c) {
      if let Some(j) = nearest_item(color.offset) {
         fills_by_item[j].push(color.hex);
      }
   }
   let mut texts_by_item: Vec<Vec<String>> = uuids.iter().map(|_| Vec::new()).collect();
   for hit in scan_texts(section_c) {
      if let Some(j) = nearest_item(hit.offset) {
         texts_by_item[j].push(hit.text);
      }
   }
   let mut outlines_by_item: Vec<Vec<(f64, f64)>> = uuids.iter().map(|_| Vec::new()).collect();
   for hit in scan_outline_hits(section_c) {
      if let Some(j) = nearest_item(hit.offset) {
         outlines_by_item[j].push((hit.x, hit.y));
      }
   }

   (0..uuids.len())
      .map(|i| {
         let fills = &fills_by_item[i];
         let texts = &texts_by_item[i];
         let outline = dedupe_outline(&outlines_by_item[i]);
         ItemGeometry {
            frame:   frames[i],
            fill:    fills
               .iter()
               .find(|hex| hex.as_str() != "#000000")
               .or_else(|| fills.first())
               .cloned(),
            text:    if texts.is_empty() {
               None
            } else {
               Some(texts.join("\n"))
            },
            outline: if outline.len() >= 3 {
               Some(outline)
            } else {
               None
            },
         }
      })
      .collect()
}

/// Place native freehand strokes on the board. Strokes are decoded in
/// item-local space, while the item's `GeometryArchive` frame carries its board
/// origin, so the local stroke bounds are translated onto that origin.
/// Without this offset every drawing collapses toward the paste origin
/// (PKDrawing-less selections only).
fn native_ink_frame(
   native_ink: Option<&NativeInkItem>,
   geo_frame: Option<FreeformFrame>,
) -> Option<FreeformFrame> {
   let Some(native_ink) = native_ink else {
      return geo_frame;
   };
   let Some(geo_frame) = geo_frame else {
      return Some(native_ink.frame);
   };
   Some(FreeformFrame {
      x:        native_ink.frame.x + geo_frame.x,
      y:        native_ink.frame.y + geo_frame.y,
      w:        native_ink.frame.w,
      h:        native_ink.frame.h,
      rotation: native_ink.frame.rotation,
   })
}

/// Decode `com.apple.freeform.CRLNativeData`, optionally joined with the
/// `TSUDescription` blob to attach a class + hints to each ordered board item.
pub fn decode_crl_native(
   data: &[u8],
   tsu: Option<&[u8]>,
) -> Result<FreeformNative, FreeformDecodeError> {
   let sections = split_sections(data)?;
   let paste_id = sections
      .index
      .get("id")
      .and_then(Plist::as_str)
      .map(str::to_owned);
   let uuids: Vec<String> = sections
      .index
      .get("boardItems")
      .and_then(Plist::as_array)
      .map(|items| {
         items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect()
      })
      .unwrap_or_default();
   let classes = match tsu {
      Some(tsu) => parse_tsu_description(tsu)?,
      None => Vec::new(),
   };

   let geometry = decode_item_geometry(sections.section_c, &uuids);
   let native_ink_items = decode_native_ink_items(sections.section_c, &uuids);
   let items: Vec<FreeformBoardItem> = uuids
      .iter()
      .enumerate()
      .zip(geometry)
      .zip(&native_ink_items)
      .map(|(((i, uuid), geo), native_ink)| FreeformBoardItem {
         index:          i,
         uuid:           Some(uuid.clone()),
         outline:        geo.outline,
         class_name:     classes
            .get(i)
            .map_or_else(|| "?".into(), |e| e.class_name.clone()),
         hints:          classes.get(i).map(|e| e.hints.clone()).unwrap_or_default(),
         frame:          native_ink_frame(native_ink.as_ref(), geo.frame),
         fill:           geo.fill,
         text:           geo.text,
         native_strokes: native_ink.as_ref().map(|ink| ink.strokes.clone()),
      })
      .collect();

   Ok(FreeformNative {
      paste_id,
      items,
      texts: tswp_strings(sections.section_c),
      colors: srgb_colors(sections.section_c),
   })
}

#[cfg(test)]
mod tests {
   use serde::Deserialize;

   use super::*;

   const NATIVE: &[u8] = include_bytes!("../fixtures/native-mixed.crlnative");
   const TSU: &[u8] = include_bytes!("../fixtures/native-mixed.tsudescription");
   const EXPECTED_JSON: &[u8] = include_bytes!("../fixtures/native-mixed.expected.json");

   /// native-mixed.expected.json uses camelCase field names.
   #[derive(Deserialize)]
   #[serde(rename_all = "camelCase")]
   struct Expected {
      paste_id:      String,
      uuids:         Vec<String>,
      classes:       Vec<String>,
      manifest_refs: usize,
      colors:        Vec<String>,
   }

   fn expected() -> Expected {
      serde_json::from_slice(EXPECTED_JSON).expect("expected.json parses")
   }

   fn near(value: f64, expected_value: f64) -> bool {
      (value - expected_value).abs() <= 0.02
   }

   // Test-only base64 decoder for the embedded minimal index plist.
   fn b64(value: &str) -> Vec<u8> {
      const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
      let mut out = Vec::new();
      let mut acc: u32 = 0;
      let mut bits: u32 = 0;
      for c in value.bytes() {
         if c == b'=' {
            break;
         }
         let v = TABLE.iter().position(|&t| t == c).expect("valid base64") as u32;
         acc = (acc << 6) | v;
         bits += 6;
         if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
         }
      }
      out
   }

   const MINIMAL_INDEX_B64: &str = "YnBsaXN0MDDTAQIDBAYHWmJvYXJkSXRlbXNSaWRfEBBpc1NtYXJ0Q29weVBhc3RloQVfECRFOEM0NjU0MC0wMDExLTRDNTQtOUQ0Qy01OEY4NUIzQjhEMTlfECQxMTExMTExMS0yMjIyLTMzMzMtNDQ0NC01NTU1NTU1NTU1NTUICA8aHTAyWYAAAAAAAAABAQAAAAAAAAAIAAAAAAAAAAAAAAAAAAAAgQ==";

   const MINIMAL_UUID: [u8; 16] = [
      0xe8, 0xc4, 0x65, 0x40, 0x00, 0x11, 0x4c, 0x54, 0x9d, 0x4c, 0x58, 0xf8, 0x5b, 0x3b, 0x8d,
      0x19,
   ];

   fn f32le(value: f32) -> Vec<u8> {
      value.to_le_bytes().to_vec()
   }

   fn minimal_ink_points() -> Vec<u8> {
      let mut points = vec![0x0a, 0x0a, 0x12, 0x08];
      points.extend(f32le(2.0));
      points.extend(f32le(3.0));
      points.extend([0x0a, 0x18, 0x08, 0x02, 0x12, 0x08]);
      points.extend(f32le(5.0));
      points.extend(f32le(7.0));
      points.push(0x1a);
      points.push(0x04);
      points.extend(f32le(2.0));
      points.push(0x22);
      points.push(0x04);
      points.extend(f32le(0.5));
      points
   }

   fn minimal_stroke_section() -> Vec<u8> {
      let points = minimal_ink_points();
      let mut section = MINIMAL_UUID.to_vec();
      section.extend(b"com.apple.ink.pen");
      section.extend([0x12, 0x0c, 0x22, 0x0a, 0x0d]);
      section.extend(f32le(10.0));
      section.push(0x15);
      section.extend(f32le(20.0));
      section.extend([0x12, 0x0c, 0x22, 0x0a, 0x0d]);
      section.extend(f32le(5.0));
      section.push(0x15);
      section.extend(f32le(7.0));
      section.push(0x22);
      section.push(points.len() as u8);
      section.extend(points);
      section
   }

   fn assemble(section_c: &[u8]) -> Vec<u8> {
      let index = b64(MINIMAL_INDEX_B64);
      let mut out = vec![0u8; 8];
      out.extend(index);
      out.extend_from_slice(section_c);
      out
   }

   fn minimal_native_ink_blob() -> Vec<u8> {
      assemble(&minimal_stroke_section())
   }

   // Like minimal_native_ink_blob, but with a GeometryArchive frame (position
   // + size Points at GEO stride) placed before the item UUID, so
   // decode_item_geometry binds it as the item's board origin. The ink
   // samples remain item-local.
   fn native_ink_blob_with_routing_frame() -> Vec<u8> {
      // position Point (100,200) + 2-byte gap + size Point (50,60) = one GeoBlock.
      let mut section = vec![0x22, 0x0a, 0x0d];
      section.extend(f32le(100.0));
      section.push(0x15);
      section.extend(f32le(200.0));
      section.extend([0x00, 0x00, 0x22, 0x0a, 0x0d]);
      section.extend(f32le(50.0));
      section.push(0x15);
      section.extend(f32le(60.0));
      section.extend(minimal_stroke_section());
      assemble(&section)
   }

   #[test]
   fn splits_manifest_bounded_index_plist_and_object_archive() {
      let sections = split_sections(NATIVE).unwrap();
      let want = expected();
      assert_eq!(manifest_item_count(sections.manifest), want.manifest_refs);
      assert_eq!(sections.index.get("id").and_then(Plist::as_str), Some(want.paste_id.as_str()));
      let board_items: Vec<&str> = sections
         .index
         .get("boardItems")
         .and_then(Plist::as_array)
         .unwrap()
         .iter()
         .filter_map(Plist::as_str)
         .collect();
      assert_eq!(board_items, want.uuids);
      // Index plist is bounded — section C must follow it, not be swallowed.
      assert!(!sections.section_c.is_empty());
   }

   #[test]
   fn joins_index_uuids_with_tsu_classes_in_order() {
      let result = decode_crl_native(NATIVE, Some(TSU)).unwrap();
      let want = expected();
      assert_eq!(result.paste_id.as_deref(), Some(want.paste_id.as_str()));
      let uuids: Vec<&str> = result
         .items
         .iter()
         .filter_map(|i| i.uuid.as_deref())
         .collect();
      assert_eq!(uuids, want.uuids);
      let classes: Vec<&str> = result.items.iter().map(|i| i.class_name.as_str()).collect();
      assert_eq!(classes, want.classes);
   }

   #[test]
   fn attaches_tsu_routing_hints() {
      let items = decode_crl_native(NATIVE, Some(TSU)).unwrap().items;
      assert_eq!(items[1].hints.get("textbox").map(String::as_str), Some("true"));
      assert_eq!(items[0].class_name, "CRLWPStickyNoteItem");
   }

   #[test]
   fn recovers_tswp_text_and_drops_crdt_bookkeeping() {
      let texts = decode_crl_native(NATIVE, Some(TSU)).unwrap().texts;
      assert!(texts.iter().any(|t| t == "hi"));
      assert!(texts.iter().any(|t| t == "aaa"));
      assert!(!texts.iter().any(|t| t == "commonCRDTData"));
   }

   #[test]
   fn recovers_srgb_fill_colors() {
      let colors = decode_crl_native(NATIVE, Some(TSU)).unwrap().colors;
      for hex in expected().colors {
         assert!(colors.contains(&hex), "missing {hex} in {colors:?}");
      }
   }

   #[test]
   fn recovers_native_freehand_strokes_embedded_in_crlnativedata() {
      let native = decode_crl_native(&minimal_native_ink_blob(), None).unwrap();
      let item = &native.items[0];
      assert_eq!(
         item.frame,
         Some(FreeformFrame {
            x:        12.0,
            y:        23.0,
            w:        3.0,
            h:        4.0,
            rotation: 0.0,
         })
      );
      let stroke = &item.native_strokes.as_ref().unwrap()[0];
      assert_eq!(stroke.ink_type, "pen");
      let points: Vec<(f64, f64, f64, f64)> = stroke
         .points
         .iter()
         .map(|p| (p.x, p.y, p.width, p.force))
         .collect();
      assert_eq!(points, vec![(0.0, 0.0, 2.0, 0.5), (3.0, 4.0, 2.0, 0.5)]);
   }

   #[test]
   fn offsets_native_freehand_strokes_by_the_item_geometry_frame() {
      let native = decode_crl_native(&native_ink_blob_with_routing_frame(), None).unwrap();
      let item = &native.items[0];
      // Strokes decode in item-local space (bbox at 12,23); the routing frame
      // (100,200) carries the board origin, so the placed frame is their sum.
      assert_eq!(
         item.frame,
         Some(FreeformFrame {
            x:        112.0,
            y:        223.0,
            w:        3.0,
            h:        4.0,
            rotation: 0.0,
         })
      );
      let points: Vec<(f64, f64)> = item.native_strokes.as_ref().unwrap()[0]
         .points
         .iter()
         .map(|p| (p.x, p.y))
         .collect();
      assert_eq!(points, vec![(0.0, 0.0), (3.0, 4.0)]);
   }

   #[test]
   fn decodes_without_a_tsu_join() {
      let result = decode_crl_native(NATIVE, None).unwrap();
      let uuids: Vec<&str> = result
         .items
         .iter()
         .filter_map(|i| i.uuid.as_deref())
         .collect();
      assert_eq!(uuids, expected().uuids);
      assert!(result.items.iter().all(|i| i.class_name == "?"));
   }

   #[test]
   fn tsu_parsing_strips_the_freeform_prefix_and_separates_hints() {
      let entries = parse_tsu_description(TSU).unwrap();
      let classes: Vec<&str> = entries.iter().map(|e| e.class_name.as_str()).collect();
      assert_eq!(classes, expected().classes);
      assert_eq!(entries[1].hints.get("textbox").map(String::as_str), Some("true"));
      assert!(!entries[1].hints.contains_key("class"));
   }

   // Frame a string as a TSWP text-storage run: `0x0a <len> <utf8> 0x12`.
   fn tswp(s: &str) -> Vec<u8> {
      let body = s.as_bytes();
      let mut out = vec![0x0a, body.len() as u8];
      out.extend_from_slice(body);
      out.push(0x12);
      out
   }

   fn scan(parts: &[&str]) -> Vec<String> {
      let bytes: Vec<u8> = parts.iter().flat_map(|p| tswp(p)).collect();
      tswp_strings(&bytes)
   }

   #[test]
   fn keeps_user_text_and_drops_graph_identifiers() {
      assert_eq!(
         scan(&[
            "hello world",                              // user text -> kept
            "commonCRDTData",                           // CRDT bookkeeping -> dropped
            "thumbnail",                                // asset reference -> dropped
            "AB12CD34-1111-2222-3333-444455556666.jpg", // asset filename -> dropped
            "Parallelogram_950",                        // shape preset -> dropped
            "com.apple.Freeform.system.font.regular",   // font id -> dropped
            "image",                                    // asset marker -> dropped
         ]),
         vec!["hello world".to_string()]
      );
   }

   #[test]
   fn ignores_non_framed_bytes() {
      assert!(tswp_strings(b"commonCRDTData").is_empty());
   }

   #[test]
   fn srgb_colors_groups_three_consecutive_fixed32_channels() {
      let sections = split_sections(NATIVE).unwrap();
      assert!(srgb_colors(sections.section_c).len() >= 2);
   }

   const REAL_BOARD_NATIVE: &[u8] = include_bytes!("../fixtures/real-board.crlnative");
   const REAL_BOARD_TSU: &[u8] = include_bytes!("../fixtures/real-board.tsudescription");

   /// Exercises the full heuristic stack — geometry frames, rotation, fills,
   /// text, and class join — against a captured 10-item real board.
   #[test]
   fn decodes_the_captured_real_board() {
      let (native, tsu) = (REAL_BOARD_NATIVE, REAL_BOARD_TSU);
      let result = decode_crl_native(native, Some(tsu)).unwrap();
      assert_eq!(result.items.len(), 10);

      let frame = |i: usize| result.items[i].frame.expect("frame");
      let line = &result.items[1];
      assert_eq!(line.fill.as_deref(), Some("#000000"));
      assert!(near(frame(1).x, 320.4769));
      assert!(near(frame(1).w, 212.132));
      assert!(near(frame(1).rotation, -std::f64::consts::FRAC_PI_4));

      assert_eq!(result.items[3].fill.as_deref(), Some("#5ac4f6")); // circle
      assert!(near(frame(3).w, 150.0) && near(frame(3).h, 150.0));
      assert_eq!(result.items[4].fill.as_deref(), Some("#5ac4f6")); // triangle
      assert!(near(frame(4).w, 150.0) && near(frame(4).h, 150.0));
      assert!(near(frame(5).w, 180.5283) && near(frame(5).h, 85.6663)); // parallelogram
      assert!(near(frame(6).w, 151.25)); // connector

      let sticky = &result.items[7];
      assert_eq!(sticky.fill.as_deref(), Some("#ffe16c"));
      assert_eq!(sticky.text.as_deref(), Some("hi"));
      assert!(near(frame(7).x, 1159.0) && near(frame(7).y, 513.5));
      assert!(near(frame(7).w, 200.0) && near(frame(7).h, 200.0));

      let table = &result.items[8];
      assert_eq!(table.fill.as_deref(), Some("#bfbfbf"));
      assert_eq!(table.text.as_deref(), Some("aaa\naaa\naaa"));
      assert!(near(frame(8).x, 244.0) && near(frame(8).y, 1339.5));
      assert!(near(frame(8).w, 688.0) && near(frame(8).h, 611.8125));

      let image = &result.items[9];
      assert_eq!(image.class_name, "CRLImageItem");
      assert_eq!(image.text, None);
      assert!(near(frame(9).w, 250.0) && near(frame(9).h, 187.5));
   }

   #[test]
   fn every_fixture_prefix_decodes_or_errors_without_panicking() {
      for cut in 0..=NATIVE.len() {
         let _ = decode_crl_native(&NATIVE[..cut], Some(TSU));
         let _ = decode_crl_native(&NATIVE[..cut], None);
      }
      for cut in 0..=TSU.len() {
         let _ = parse_tsu_description(&TSU[..cut]);
      }
      let ink = minimal_native_ink_blob();
      for cut in 0..=ink.len() {
         let _ = decode_crl_native(&ink[..cut], None);
      }
   }
}
