//! Decoder for Apple's `com.apple.drawing` pasteboard flavor — the
//! `PencilKit` `PKDrawing` that Freeform flattens all freehand ink into.
//!
//! The on-disk format is undocumented; the layout was verified byte-for-byte
//! against a real `PKDrawing.dataRepresentation()` (see
//! `fixtures/ink-pen.drawing`). The protobuf body is hand-walked with a
//! minimal strict wire reader; the message tree is documented below.
//!
//! Container:  "wrd" magic (3 bytes) + protobuf body.
//! Body fields:  #4 (repeated) ink definitions; #5 (repeated) strokes.
//!   Ink:    #1 color { #1..#4 = f32 r,g,b,a }, #2 = ink id string, ...
//!   Stroke: #4 = ink index (into the #4 ink list), #5 = path, #7 = transform
//!           { #1..#6 = f32 a,b,c,d,tx,ty }.
//!   Path:   #3 = point count, #6 = width/default metadata, #7 = packed points.
//!   Point:  variable-size struct. Recent `PencilKit` writes 12/14/16/18/20/22-
//!           byte records depending on which per-point channels are present.
//!           The stable prefix is f32 x, f32 y, f32 timeOffset; width is f32
//!           at +12 when the stride includes it, force is u16*1000 at +16.

use super::types::{FreeformDecodeError, FreeformDrawing, FreeformInkPoint, FreeformInkStroke};

const MAGIC: &[u8; 3] = b"wrd";
/// Legacy full per-point record size; newer drawings may use smaller strides.
pub const PK_POINT_STRIDE: usize = 22;
const POINT_X_OFFSET: usize = 0;
const POINT_Y_OFFSET: usize = 4;
const POINT_WIDTH_OFFSET: usize = 12;
const POINT_FORCE_OFFSET: usize = 16;

// ---------------------------------------------------------------------------
// Minimal strict protobuf wire walker. Structural damage (truncation, bad
// wire type, field 0) is an Err, which decode_pk_drawing maps to a typed
// "invalid protobuf body" error.

#[derive(Debug, Clone, Copy)]
pub(crate) enum WireValue<'a> {
   Varint(u64),
   /// Consumed for skipping only — no Freeform message reads a fixed64 field.
   Fixed64,
   Bytes(&'a [u8]),
   Fixed32(u32),
}

impl WireValue<'_> {
   const fn as_f32(&self) -> Option<f32> {
      match self {
         WireValue::Fixed32(bits) => Some(f32::from_bits(*bits)),
         _ => None,
      }
   }
}

fn read_varint(b: &[u8], mut i: usize) -> Result<(u64, usize), ()> {
   let mut value: u64 = 0;
   for k in 0..10 {
      let c = *b.get(i).ok_or(())?;
      i += 1;
      value |= ((c & 0x7f) as u64).wrapping_shl(7 * k);
      if c & 0x80 == 0 {
         return Ok((value, i));
      }
   }
   Err(())
}

/// Walk one message's fields, calling `visit(field_number, value)` for each.
pub(crate) fn walk_message<'a>(
   buf: &'a [u8],
   visit: &mut dyn FnMut(u64, WireValue<'a>) -> Result<(), ()>,
) -> Result<(), ()> {
   let mut i = 0;
   while i < buf.len() {
      let (tag, next) = read_varint(buf, i)?;
      i = next;
      let field = tag >> 3;
      if field == 0 {
         return Err(());
      }
      let value = match tag & 7 {
         0 => {
            let (v, next) = read_varint(buf, i)?;
            i = next;
            WireValue::Varint(v)
         },
         1 => {
            buf.get(i..i + 8).ok_or(())?;
            i += 8;
            WireValue::Fixed64
         },
         2 => {
            let (len, next) = read_varint(buf, i)?;
            let len = usize::try_from(len).map_err(|_| ())?;
            let end = next.checked_add(len).ok_or(())?;
            let bytes = buf.get(next..end).ok_or(())?;
            i = end;
            WireValue::Bytes(bytes)
         },
         5 => {
            let bytes: [u8; 4] = buf.get(i..i + 4).ok_or(())?.try_into().map_err(|_| ())?;
            i += 4;
            WireValue::Fixed32(u32::from_le_bytes(bytes))
         },
         _ => return Err(()), // groups / reserved wire types
      };
      visit(field, value)?;
   }
   Ok(())
}

// ---------------------------------------------------------------------------
// Message shapes, merged with proto3 semantics (a repeated scalar keeps the
// last value; missing sub-messages stay None).

#[derive(Debug, Clone)]
struct InkDef {
   ink_type: String,
   color:    String,
   opacity:  f64,
}

fn default_ink() -> InkDef {
   InkDef { ink_type: "pen".into(), color: "#000000".into(), opacity: 1.0 }
}

#[derive(Default)]
struct PkColor {
   r: f32,
   g: f32,
   b: f32,
   a: f32,
}

#[derive(Default)]
struct PkInk {
   color:      Option<PkColor>,
   identifier: String,
}

#[derive(Default, Clone, Copy)]
struct PkTransform {
   a:  f32,
   b:  f32,
   c:  f32,
   d:  f32,
   tx: f32,
   ty: f32,
}

#[derive(Default)]
struct PkPath<'a> {
   point_count: u32,
   width_meta:  &'a [u8],
   points:      &'a [u8],
}

#[derive(Default)]
struct PkStroke<'a> {
   ink_index: u32,
   path:      Option<PkPath<'a>>,
   transform: Option<PkTransform>,
}

fn parse_color_into(buf: &[u8], color: &mut PkColor) -> Result<(), ()> {
   walk_message(buf, &mut |field, value| {
      if let Some(v) = value.as_f32() {
         match field {
            1 => color.r = v,
            2 => color.g = v,
            3 => color.b = v,
            4 => color.a = v,
            _ => {},
         }
      }
      Ok(())
   })
}

fn parse_ink(buf: &[u8]) -> Result<PkInk, ()> {
   let mut ink = PkInk::default();
   walk_message(buf, &mut |field, value| {
      match (field, value) {
         (1, WireValue::Bytes(b)) => {
            let color = ink.color.get_or_insert_with(PkColor::default);
            parse_color_into(b, color)?;
         },
         (2, WireValue::Bytes(b)) => ink.identifier = String::from_utf8_lossy(b).into_owned(),
         _ => {},
      }
      Ok(())
   })?;
   Ok(ink)
}

fn parse_transform_into(buf: &[u8], t: &mut PkTransform) -> Result<(), ()> {
   walk_message(buf, &mut |field, value| {
      if let Some(v) = value.as_f32() {
         match field {
            1 => t.a = v,
            2 => t.b = v,
            3 => t.c = v,
            4 => t.d = v,
            5 => t.tx = v,
            6 => t.ty = v,
            _ => {},
         }
      }
      Ok(())
   })
}

fn parse_path_into<'a>(buf: &'a [u8], path: &mut PkPath<'a>) -> Result<(), ()> {
   walk_message(buf, &mut |field, value| {
      match (field, value) {
         (3, WireValue::Varint(v)) => path.point_count = v as u32,
         (6, WireValue::Bytes(b)) => path.width_meta = b,
         (7, WireValue::Bytes(b)) => path.points = b,
         _ => {},
      }
      Ok(())
   })
}

fn parse_stroke(buf: &[u8]) -> Result<PkStroke<'_>, ()> {
   let mut stroke = PkStroke::default();
   walk_message(buf, &mut |field, value| {
      match (field, value) {
         (4, WireValue::Varint(v)) => stroke.ink_index = v as u32,
         (5, WireValue::Bytes(b)) => {
            let path = stroke.path.get_or_insert_with(PkPath::default);
            parse_path_into(b, path)?;
         },
         (7, WireValue::Bytes(b)) => {
            let t = stroke.transform.get_or_insert_with(PkTransform::default);
            parse_transform_into(b, t)?;
         },
         _ => {},
      }
      Ok(())
   })?;
   Ok(stroke)
}

// ---------------------------------------------------------------------------

/// True when `data` starts with the `PKDrawing` `"wrd"` magic.
pub fn is_pk_drawing(data: &[u8]) -> bool {
   data.len() >= MAGIC.len() && &data[..MAGIC.len()] == MAGIC
}

/// Decode a `com.apple.drawing` `PKDrawing` blob into normalized strokes.
///
/// Errors when the header/version is unrecognized or a stroke's packed point
/// blob does not match the expected raw layout (e.g. a future-version
/// compression or schema change).
pub fn decode_pk_drawing(data: &[u8]) -> Result<FreeformDrawing, FreeformDecodeError> {
   if !is_pk_drawing(data) {
      return Err(FreeformDecodeError("com.apple.drawing: missing \"wrd\" header".into()));
   }

   let body = &data[MAGIC.len()..];
   let mut ink_msgs: Vec<&[u8]> = Vec::new();
   let mut stroke_msgs: Vec<&[u8]> = Vec::new();
   let structure = walk_message(body, &mut |field, value| {
      match (field, value) {
         (4, WireValue::Bytes(b)) => ink_msgs.push(b),
         (5, WireValue::Bytes(b)) => stroke_msgs.push(b),
         _ => {},
      }
      Ok(())
   });
   let invalid = || FreeformDecodeError("com.apple.drawing: invalid protobuf body".into());
   structure.map_err(|()| invalid())?;

   let mut inks: Vec<InkDef> = Vec::with_capacity(ink_msgs.len());
   for msg in ink_msgs {
      inks.push(ink_def(parse_ink(msg).map_err(|()| invalid())?));
   }

   let mut strokes: Vec<FreeformInkStroke> = Vec::new();
   for msg in stroke_msgs {
      let stroke = parse_stroke(msg).map_err(|()| invalid())?;
      if let Some(stroke) = convert_stroke(&stroke, &inks)? {
         strokes.push(stroke);
      }
   }

   Ok(FreeformDrawing { strokes })
}

fn ink_def(ink: PkInk) -> InkDef {
   let ink_type = ink_type_from_identifier(&ink.identifier);
   match ink.color {
      None => InkDef { ink_type, ..default_ink() },
      Some(c) => InkDef {
         ink_type,
         color: to_hex(c.r as f64, c.g as f64, c.b as f64),
         opacity: clamp(c.a as f64, 0.0, 1.0),
      },
   }
}

const IDENTITY: [f64; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

fn convert_stroke(
   stroke: &PkStroke<'_>,
   inks: &[InkDef],
) -> Result<Option<FreeformInkStroke>, FreeformDecodeError> {
   let Some(path) = &stroke.path else {
      return Ok(None);
   };
   let transform = match stroke.transform {
      None => IDENTITY,
      Some(t) => [t.a as f64, t.b as f64, t.c as f64, t.d as f64, t.tx as f64, t.ty as f64],
   };
   let points = decode_points(path, &transform)?;
   if points.is_empty() {
      return Ok(None);
   }
   let ink = inks
      .get(stroke.ink_index as usize)
      .or_else(|| inks.first())
      .cloned()
      .unwrap_or_else(default_ink);
   Ok(Some(FreeformInkStroke {
      ink_type: ink.ink_type,
      color: ink.color,
      opacity: ink.opacity,
      points,
   }))
}

fn read_f32_le(bytes: &[u8]) -> f32 {
   f32::from_le_bytes(bytes[..4].try_into().unwrap_or([0; 4]))
}

#[allow(clippy::many_single_char_names, reason = "canonical CGAffineTransform component notation")]
fn decode_points(
   path: &PkPath<'_>,
   transform: &[f64; 6],
) -> Result<Vec<FreeformInkPoint>, FreeformDecodeError> {
   let count = path.point_count as usize;
   let blob = path.points;
   let fallback_width = width_hint(path.width_meta, 1.0);
   if count == 0 || blob.is_empty() {
      return Ok(Vec::new());
   }
   if blob.len() % count != 0 {
      return Err(FreeformDecodeError(format!(
         "com.apple.drawing: stroke point blob is {}B for {} points. Unsupported PKDrawing \
          version: point packing.",
         blob.len(),
         count
      )));
   }
   let stride = blob.len() / count;
   if !is_supported_point_stride(stride) {
      return Err(FreeformDecodeError(format!(
         "com.apple.drawing: unsupported {stride}B point stride. Unsupported PKDrawing version: \
          point packing."
      )));
   }
   let [a, b, c, d, tx, ty] = *transform;
   let mut points = Vec::with_capacity(count);
   for k in 0..count {
      let o = k * stride;
      let lx = read_f32_le(&blob[o + POINT_X_OFFSET..]) as f64;
      let ly = read_f32_le(&blob[o + POINT_Y_OFFSET..]) as f64;
      let width = if stride >= POINT_WIDTH_OFFSET + 4 {
         finite_positive(read_f32_le(&blob[o + POINT_WIDTH_OFFSET..]) as f64, fallback_width)
      } else {
         fallback_width
      };
      let force = if stride >= POINT_FORCE_OFFSET + 2 {
         let raw =
            u16::from_le_bytes([blob[o + POINT_FORCE_OFFSET], blob[o + POINT_FORCE_OFFSET + 1]]);
         clamp(raw as f64 / 1000.0, 0.0, 1.0)
      } else {
         1.0
      };
      points.push(FreeformInkPoint {
         x: a * lx + c * ly + tx,
         y: b * lx + d * ly + ty,
         force,
         width,
      });
   }
   Ok(points)
}

const fn is_supported_point_stride(stride: usize) -> bool {
   matches!(stride, 12 | 14 | 16 | 18 | 20 | 22)
}

fn width_hint(bytes: &[u8], fallback: f64) -> f64 {
   if bytes.len() < 4 {
      return fallback;
   }
   finite_positive(read_f32_le(bytes) as f64, fallback)
}

fn finite_positive(value: f64, fallback: f64) -> f64 {
   if value.is_finite() && value > 0.0 {
      value
   } else {
      fallback
   }
}

fn ink_type_from_identifier(id: &str) -> String {
   if id.contains("marker") {
      return "marker".into();
   }
   if id.contains("pencil") {
      return "pencil".into();
   }
   if id.contains("pen") {
      return "pen".into();
   }
   if id.is_empty() {
      "pen".into()
   } else {
      id.into()
   }
}

pub(crate) fn to_hex(r: f64, g: f64, b: f64) -> String {
   let channel = |v: f64| (clamp(v, 0.0, 1.0) * 255.0).round() as u32;
   format!("#{:02x}{:02x}{:02x}", channel(r), channel(g), channel(b))
}

const fn clamp(value: f64, min: f64, max: f64) -> f64 {
   value.max(min).min(max)
}

#[cfg(test)]
mod tests {
   use super::*;

   const FIXTURE: &[u8] = include_bytes!("../fixtures/ink-pen.drawing");

   // fixtures/ink-pen.drawing is a real PencilKit `PKDrawing.dataRepresentation()`
   // (the exact serializer Freeform uses for `com.apple.drawing`). Ground truth
   // from the generator: a red `.pen` ink with two strokes, transform
   // (1,0,0,1,100,50):
   //   stroke 0: local (0,0)f.2 (10,5)f.5 (20,0)f.8 (30,10)f.4 -> page
   // (100,50)..(130,60)   stroke 1: local (0,40)f.3 (40,40)f.9
   // -> page (100,90),(140,90)

   fn near(a: f64, b: f64) -> bool {
      (a - b).abs() <= 0.01
   }

   // Test-only protobuf builders for synthesizing PKDrawing bodies.
   fn varint(mut value: u64) -> Vec<u8> {
      let mut bytes = Vec::new();
      while value >= 0x80 {
         bytes.push((value & 0x7f) as u8 | 0x80);
         value >>= 7;
      }
      bytes.push(value as u8);
      bytes
   }

   fn field_varint(field: u64, value: u64) -> Vec<u8> {
      let mut out = varint(field << 3);
      out.extend(varint(value));
      out
   }

   fn field_bytes(field: u64, bytes: &[u8]) -> Vec<u8> {
      let mut out = varint((field << 3) | 2);
      out.extend(varint(bytes.len() as u64));
      out.extend_from_slice(bytes);
      out
   }

   fn f32le(value: f32) -> Vec<u8> {
      value.to_le_bytes().to_vec()
   }

   fn u16le(value: u16) -> Vec<u8> {
      value.to_le_bytes().to_vec()
   }

   fn drawing_with_paths(paths: &[Vec<u8>]) -> Vec<u8> {
      let mut out = MAGIC.to_vec();
      for path in paths {
         out.extend(field_bytes(5, &field_bytes(5, path)));
      }
      out
   }

   fn path_with_point_blob(count: u64, blob: &[u8], width_hint: Option<f32>) -> Vec<u8> {
      let mut out = field_varint(3, count);
      if let Some(w) = width_hint {
         out.extend(field_bytes(6, &f32le(w)));
      }
      out.extend(field_bytes(7, blob));
      out
   }

   fn point18(x: f32, y: f32, width: f32, force: u16) -> Vec<u8> {
      let mut out = f32le(x);
      out.extend(f32le(y));
      out.extend(f32le(0.0));
      out.extend(f32le(width));
      out.extend(u16le(force));
      out
   }

   fn point20(x: f32, y: f32, width: f32, force: u16) -> Vec<u8> {
      let mut out = point18(x, y, width, force);
      out.extend([0x34, 0x12]);
      out
   }

   fn point12(x: f32, y: f32) -> Vec<u8> {
      let mut out = f32le(x);
      out.extend(f32le(y));
      out.extend(f32le(0.0));
      out
   }

   #[test]
   fn recognizes_the_pkdrawing_wrd_header() {
      assert!(is_pk_drawing(FIXTURE));
      assert!(!is_pk_drawing(&[0x62, 0x70, 0x6c]));
   }

   #[test]
   fn decodes_every_stroke_with_transformed_page_space_points() {
      let FreeformDrawing { strokes } = decode_pk_drawing(FIXTURE).unwrap();
      assert_eq!(strokes.iter().map(|s| s.points.len()).collect::<Vec<_>>(), vec![4, 2]);

      let p0 = strokes[0].points[0];
      let p3 = strokes[0].points[3];
      assert!(near(p0.x, 100.0) && near(p0.y, 50.0));
      assert!(near(p3.x, 130.0) && near(p3.y, 60.0));

      let rounded: Vec<[i64; 2]> = strokes[1]
         .points
         .iter()
         .map(|p| [p.x.round() as i64, p.y.round() as i64])
         .collect();
      assert_eq!(rounded, vec![[100, 90], [140, 90]]);
   }

   #[test]
   fn recovers_per_point_pressure() {
      let drawing = decode_pk_drawing(FIXTURE).unwrap();
      let forces: Vec<f64> = drawing.strokes[0]
         .points
         .iter()
         .map(|p| (p.force * 100.0).round() / 100.0)
         .collect();
      assert_eq!(forces, vec![0.2, 0.5, 0.8, 0.4]);
   }

   #[test]
   fn recovers_the_srgb_ink_color_opacity_and_family() {
      let drawing = decode_pk_drawing(FIXTURE).unwrap();
      let s0 = &drawing.strokes[0];
      // PencilKit stored systemRed as sRGB (1, 0.258824, 0.270588, 1) -> #ff4245.
      assert_eq!(s0.color, "#ff4245");
      assert_eq!(s0.opacity, 1.0);
      assert_eq!(s0.ink_type, "pen");
   }

   #[test]
   fn decodes_current_pencilkit_variable_width_point_records() {
      let drawing_bytes = drawing_with_paths(&[
         path_with_point_blob(1, &point18(10.0, 20.0, 2.5, 500), None),
         path_with_point_blob(1, &point20(30.0, 40.0, 3.5, 250), None),
         path_with_point_blob(1, &point12(50.0, 60.0), Some(4.5)),
      ]);
      let FreeformDrawing { strokes } = decode_pk_drawing(&drawing_bytes).unwrap();
      assert_eq!(strokes.iter().map(|s| s.points.len()).collect::<Vec<_>>(), vec![1, 1, 1]);
      let first = |s: &FreeformInkStroke| s.points[0];
      assert_eq!(strokes.iter().map(|s| first(s).x).collect::<Vec<_>>(), vec![10.0, 30.0, 50.0]);
      assert_eq!(strokes.iter().map(|s| first(s).y).collect::<Vec<_>>(), vec![20.0, 40.0, 60.0]);
      assert_eq!(strokes.iter().map(|s| first(s).width).collect::<Vec<_>>(), vec![2.5, 3.5, 4.5]);
      assert_eq!(strokes.iter().map(|s| first(s).force).collect::<Vec<_>>(), vec![0.5, 0.25, 1.0]);
   }

   #[test]
   fn errors_on_an_unrecognized_header() {
      assert!(decode_pk_drawing(&[1, 2, 3, 4]).is_err());
   }

   #[test]
   fn errors_when_a_point_blob_does_not_match_the_raw_stride() {
      // Hand-built "wrd" + body #5 stroke -> #5 path { #3 count=3, #7 blob=1 byte }.
      let path = [0x18, 0x03, 0x3a, 0x01, 0x00];
      let mut stroke = vec![0x2a, path.len() as u8];
      stroke.extend(path);
      let mut body = vec![0x2a, stroke.len() as u8];
      body.extend(stroke);
      let mut blob = MAGIC.to_vec();
      blob.extend(body);
      let err = decode_pk_drawing(&blob).unwrap_err();
      assert!(err.0.contains("Unsupported PKDrawing version"), "{}", err.0);
   }

   #[test]
   fn exposes_the_verified_point_stride() {
      assert_eq!(PK_POINT_STRIDE, 22);
   }

   #[test]
   fn every_fixture_prefix_decodes_or_errors_without_panicking() {
      for cut in 0..=FIXTURE.len() {
         let _ = decode_pk_drawing(&FIXTURE[..cut]);
      }
   }
}
