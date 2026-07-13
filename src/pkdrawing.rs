//! Decoder for Apple's `com.apple.drawing` `PencilKit` pasteboard flavor.
//!
//! The `wrd` body is protobuf-like, while each path's point records are packed
//! binary data.  This module deliberately only exposes fields whose layout is
//! established by the installed `PencilKit` format; all other bytes remain on
//! their enclosing stroke.

use super::types::{
   FreeformColor, FreeformDecodeError, FreeformDrawing, FreeformInkPoint, FreeformInkPointRole,
   FreeformInkStroke, FreeformTransform,
};

const MAGIC: &[u8; 3] = b"wrd";
/// The newest verified complete `PencilKit` point record size.
pub const PK_POINT_STRIDE: usize = 22;
const POINT_X_OFFSET: usize = 0;
const POINT_Y_OFFSET: usize = 4;
const POINT_WIDTH_OFFSET: usize = 12;
const POINT_FORCE_OFFSET: usize = 16;

#[derive(Clone, Copy)]
pub(crate) enum WireValue<'a> {
   Varint(u64),
   Fixed64(
      #[allow(
         dead_code,
         reason = "parsed as part of protobuf wire type but not read in this module"
      )]
      u64,
   ),
   Fixed32(u32),
   Bytes(&'a [u8]),
}

impl WireValue<'_> {
   pub(crate) const fn as_f32(self) -> Option<f32> {
      match self {
         Self::Fixed32(value) => Some(f32::from_bits(value)),
         _ => None,
      }
   }

   const fn f32(self) -> Option<f32> {
      self.as_f32()
   }
}

fn invalid(message: impl Into<String>) -> FreeformDecodeError {
   FreeformDecodeError::invalid(format!("com.apple.drawing: {}", message.into()))
}

fn incomplete(message: impl Into<String>) -> FreeformDecodeError {
   FreeformDecodeError::incomplete(format!("com.apple.drawing: {}", message.into()))
}

fn read_varint(buf: &[u8], mut offset: usize) -> Result<(u64, usize), FreeformDecodeError> {
   let mut value = 0u64;
   for shift in (0..64).step_by(7) {
      let byte = *buf
         .get(offset)
         .ok_or_else(|| incomplete("truncated varint"))?;
      offset += 1;
      if shift == 63 && byte > 1 {
         return Err(invalid("varint overflow"));
      }
      value |= u64::from(byte & 0x7f) << shift;
      if byte & 0x80 == 0 {
         return Ok((value, offset));
      }
   }
   Err(invalid("varint overflow"))
}

pub(crate) fn walk_message<'a>(
   buf: &'a [u8],
   visitor: &mut dyn FnMut(u64, WireValue<'a>) -> Result<(), FreeformDecodeError>,
) -> Result<(), FreeformDecodeError> {
   let mut offset = 0;
   while offset < buf.len() {
      let (tag, next) = read_varint(buf, offset)?;
      offset = next;
      let field = tag >> 3;
      if field == 0 {
         return Err(invalid("protobuf field zero"));
      }
      let value = match tag & 7 {
         0 => {
            let (value, next) = read_varint(buf, offset)?;
            offset = next;
            WireValue::Varint(value)
         },
         1 => {
            let bytes = buf
               .get(offset..offset + 8)
               .ok_or_else(|| incomplete("truncated fixed64 field"))?;
            offset += 8;
            WireValue::Fixed64(u64::from_le_bytes(bytes.try_into().unwrap()))
         },
         2 => {
            let (length, next) = read_varint(buf, offset)?;
            offset = next;
            let length = usize::try_from(length).map_err(|_| invalid("length overflow"))?;
            let end = offset
               .checked_add(length)
               .ok_or_else(|| invalid("length overflow"))?;
            let bytes = buf
               .get(offset..end)
               .ok_or_else(|| incomplete("truncated length-delimited field"))?;
            offset = end;
            WireValue::Bytes(bytes)
         },
         5 => {
            let bytes: [u8; 4] = buf
               .get(offset..offset + 4)
               .ok_or_else(|| incomplete("truncated fixed32 field"))?
               .try_into()
               .unwrap();
            offset += 4;
            WireValue::Fixed32(u32::from_le_bytes(bytes))
         },
         _ => return Err(invalid("unsupported protobuf wire type")),
      };
      visitor(field, value)?;
   }
   Ok(())
}

#[derive(Default)]
struct InkDef {
   identifier: String,
   color:      Option<FreeformColor>,
}
#[derive(Default)]
struct Path<'a> {
   count:  Option<u64>,
   points: Option<&'a [u8]>,
}
#[derive(Default)]
struct Stroke<'a> {
   ink_index: Option<u64>,
   path:      Option<Path<'a>>,
   transform: Option<FreeformTransform>,
}

fn finite(value: f32) -> Option<f64> {
   let value = f64::from(value);
   value.is_finite().then_some(value)
}

fn parse_color(buf: &[u8]) -> Result<Option<FreeformColor>, FreeformDecodeError> {
   let mut channels = [None; 4];
   walk_message(buf, &mut |field, value| {
      if (1..=4).contains(&field) {
         channels[(field - 1) as usize] = value.f32().and_then(finite);
      }
      Ok(())
   })?;
   let [Some(red), Some(green), Some(blue), Some(alpha)] = channels else {
      return Ok(None);
   };
   Ok(Some(FreeformColor {
      color_space: "sRGB".into(),
      red,
      green,
      blue,
      alpha,
      hex: to_hex(red, green, blue),
   }))
}

fn parse_ink(buf: &[u8]) -> Result<InkDef, FreeformDecodeError> {
   let mut ink = InkDef::default();
   walk_message(buf, &mut |field, value| {
      match (field, value) {
         (1, WireValue::Bytes(color)) => ink.color = parse_color(color)?,
         (2, WireValue::Bytes(bytes)) => {
            std::str::from_utf8(bytes)
               .map_err(|_| invalid("non-UTF-8 ink identifier"))?
               .clone_into(&mut ink.identifier);
         },
         _ => {},
      }
      Ok(())
   })?;
   Ok(ink)
}

fn parse_transform(buf: &[u8]) -> Result<FreeformTransform, FreeformDecodeError> {
   let mut values = [None; 6];
   walk_message(buf, &mut |field, value| {
      if (1..=6).contains(&field) {
         values[(field - 1) as usize] = value.f32().and_then(finite);
      }
      Ok(())
   })?;
   if values.iter().any(Option::is_none) {
      return Err(invalid("incomplete stroke transform"));
   }
   Ok(FreeformTransform {
      a:  values[0].unwrap(),
      b:  values[1].unwrap(),
      c:  values[2].unwrap(),
      d:  values[3].unwrap(),
      tx: values[4].unwrap(),
      ty: values[5].unwrap(),
   })
}

fn parse_path(buf: &[u8]) -> Result<Path<'_>, FreeformDecodeError> {
   let mut path = Path::default();
   walk_message(buf, &mut |field, value| {
      match (field, value) {
         (3, WireValue::Varint(count)) => path.count = Some(count),
         (7, WireValue::Bytes(points)) => path.points = Some(points),
         _ => {},
      }
      Ok(())
   })?;
   Ok(path)
}

fn parse_stroke(buf: &[u8]) -> Result<Stroke<'_>, FreeformDecodeError> {
   let mut stroke = Stroke::default();
   walk_message(buf, &mut |field, value| {
      match (field, value) {
         (4, WireValue::Varint(index)) => stroke.ink_index = Some(index),
         (5, WireValue::Bytes(path)) => stroke.path = Some(parse_path(path)?),
         (7, WireValue::Bytes(transform)) => stroke.transform = Some(parse_transform(transform)?),
         _ => {},
      }
      Ok(())
   })?;
   Ok(stroke)
}

/// Returns whether `data` has the native `PencilKit` `wrd` marker.
pub fn is_pk_drawing(data: &[u8]) -> bool {
   data.starts_with(MAGIC)
}

/// Decodes a lossless `PencilKit` drawing without applying stroke transforms to
/// local points.
pub fn decode_pk_drawing(data: &[u8]) -> Result<FreeformDrawing, FreeformDecodeError> {
   let body = data
      .strip_prefix(MAGIC)
      .ok_or_else(|| invalid("missing \"wrd\" header"))?;
   let mut ink_messages = Vec::new();
   let mut stroke_messages = Vec::new();
   walk_message(body, &mut |field, value| {
      match (field, value) {
         (4, WireValue::Bytes(ink)) => ink_messages.push(ink),
         (5, WireValue::Bytes(stroke)) => stroke_messages.push(stroke),
         _ => {},
      }
      Ok(())
   })?;
   let inks: Result<Vec<_>, _> = ink_messages.into_iter().map(parse_ink).collect();
   let inks = inks?;
   let mut strokes = Vec::with_capacity(stroke_messages.len());
   for raw_data in stroke_messages {
      let stroke = parse_stroke(raw_data)?;
      let index = stroke
         .ink_index
         .ok_or_else(|| invalid("stroke missing ink reference"))?;
      let ink = inks
         .get(usize::try_from(index).map_err(|_| invalid("ink reference overflow"))?)
         .ok_or_else(|| invalid("stroke references missing ink"))?;
      let path = stroke.path.ok_or_else(|| invalid("stroke missing path"))?;
      let points = decode_points(&path)?;
      strokes.push(FreeformInkStroke {
         ink_type: ink_type_from_identifier(&ink.identifier).to_owned(),
         ink_identifier: ink.identifier.clone(),
         color: ink.color.clone(),
         transform: stroke.transform.unwrap_or_default(),
         point_role: FreeformInkPointRole::SplineControl,
         points,
         visible_ranges: None,
         random_seed: None,
         raw_data: raw_data.to_vec(),
      });
   }
   Ok(FreeformDrawing { required_content_version: None, bounds: None, strokes })
}

fn decode_points(path: &Path<'_>) -> Result<Vec<FreeformInkPoint>, FreeformDecodeError> {
   let count = usize::try_from(
      path
         .count
         .ok_or_else(|| invalid("path missing point count"))?,
   )
   .map_err(|_| invalid("point count overflow"))?;
   let blob = path
      .points
      .ok_or_else(|| invalid("path missing point data"))?;
   if count == 0 {
      return if blob.is_empty() {
         Ok(Vec::new())
      } else {
         Err(invalid("point data with zero point count"))
      };
   }
   if blob.len() % count != 0 {
      return Err(invalid("point count does not divide packed point data"));
   }
   let stride = blob.len() / count;
   if !matches!(stride, 12 | 14 | 16 | 18 | 20 | 22) {
      return Err(FreeformDecodeError::unsupported(format!(
         "com.apple.drawing: unsupported {stride}B point stride"
      )));
   }
   let mut points = Vec::with_capacity(count);
   for record in blob.chunks_exact(stride) {
      let x = f32_at(record, POINT_X_OFFSET)?.ok_or_else(|| invalid("non-finite point x"))?;
      let y = f32_at(record, POINT_Y_OFFSET)?.ok_or_else(|| invalid("non-finite point y"))?;
      let width = if stride >= POINT_WIDTH_OFFSET + 4 {
         f32_at(record, POINT_WIDTH_OFFSET)?
      } else {
         None
      };
      let force = if stride >= POINT_FORCE_OFFSET + 2 {
         Some(
            f64::from(u16::from_le_bytes(
               record[POINT_FORCE_OFFSET..POINT_FORCE_OFFSET + 2]
                  .try_into()
                  .unwrap(),
            )) / 1000.0,
         )
      } else {
         None
      };
      points.push(FreeformInkPoint { x, y, width, force, ..Default::default() });
   }
   Ok(points)
}

fn f32_at(record: &[u8], offset: usize) -> Result<Option<f64>, FreeformDecodeError> {
   let bytes: [u8; 4] = record
      .get(offset..offset + 4)
      .ok_or_else(|| invalid("truncated point record"))?
      .try_into()
      .unwrap();
   Ok(finite(f32::from_le_bytes(bytes)))
}

fn ink_type_from_identifier(identifier: &str) -> &str {
   match identifier {
      "com.apple.ink.pen" => "pen",
      "com.apple.ink.marker" => "marker",
      "com.apple.ink.pencil" => "pencil",
      "com.apple.ink.monoline" => "monoline",
      "com.apple.ink.fountainPen" => "fountainPen",
      "com.apple.ink.watercolor" => "watercolor",
      "com.apple.ink.crayon" => "crayon",
      "com.apple.ink.eraser" => "eraser",
      "com.apple.ink.lasso" => "lasso",
      _ => identifier,
   }
}

pub(crate) fn to_hex(r: f64, g: f64, b: f64) -> String {
   let channel = |value: f64| (value.clamp(0.0, 1.0) * 255.0).round() as u32;
   format!("#{:02x}{:02x}{:02x}", channel(r), channel(g), channel(b))
}

#[cfg(test)]
mod tests {
   use super::*;
   const FIXTURE: &[u8] = include_bytes!("../fixtures/ink-pen.drawing");
   fn varint(mut value: u64) -> Vec<u8> {
      let mut out = Vec::new();
      while value >= 128 {
         out.push(value as u8 | 128);
         value >>= 7;
      }
      out.push(value as u8);
      out
   }
   fn bytes(field: u64, data: &[u8]) -> Vec<u8> {
      let mut out = varint(field << 3 | 2);
      out.extend(varint(data.len() as u64));
      out.extend(data);
      out
   }
   fn vint(field: u64, value: u64) -> Vec<u8> {
      let mut out = varint(field << 3);
      out.extend(varint(value));
      out
   }
   fn fixed(field: u64, value: f32) -> Vec<u8> {
      let mut out = varint(field << 3 | 5);
      out.extend(value.to_le_bytes());
      out
   }
   fn ink(id: &str) -> Vec<u8> {
      bytes(2, id.as_bytes())
   }
   fn point(x: f32, y: f32, width: Option<f32>, force: Option<u16>) -> Vec<u8> {
      let mut out = Vec::new();
      out.extend(x.to_le_bytes());
      out.extend(y.to_le_bytes());
      out.extend(0f32.to_le_bytes());
      if let Some(width) = width {
         out.extend(width.to_le_bytes());
         if let Some(force) = force {
            out.extend(force.to_le_bytes());
         }
      }
      out
   }
   fn transform(a: f32, b: f32, c: f32, d: f32, tx: f32, ty: f32) -> Vec<u8> {
      [a, b, c, d, tx, ty]
         .into_iter()
         .enumerate()
         .flat_map(|(index, value)| fixed(index as u64 + 1, value))
         .collect()
   }
   fn drawing(inks: &[Vec<u8>], strokes: &[Vec<u8>]) -> Vec<u8> {
      let mut out = MAGIC.to_vec();
      for ink in inks {
         out.extend(bytes(4, ink));
      }
      for stroke in strokes {
         out.extend(bytes(5, stroke));
      }
      out
   }
   fn stroke(index: u64, count: u64, points: &[u8], transform_bytes: Option<&[u8]>) -> Vec<u8> {
      let mut path = vint(3, count);
      path.extend(bytes(7, points));
      let mut out = vint(4, index);
      out.extend(bytes(5, &path));
      if let Some(transform) = transform_bytes {
         out.extend(bytes(7, transform));
      }
      out
   }

   #[test]
   fn real_fixture_decodes() {
      assert!(is_pk_drawing(FIXTURE));
      assert!(!decode_pk_drawing(FIXTURE).unwrap().strokes.is_empty());
   }
   #[test]
   fn keeps_points_local_and_transform_complete() {
      let record = point(2., 3., Some(4.), Some(500));
      let input = drawing(&[ink("com.apple.ink.pen")], &[stroke(
         0,
         1,
         &record,
         Some(&transform(0., 2., -3., 0., 10., 20.)),
      )]);
      let result = decode_pk_drawing(&input).unwrap();
      assert_eq!(
         (
            result.strokes[0].points[0].x,
            result.strokes[0].points[0].y,
            result.strokes[0].points[0].width
         ),
         (2., 3., Some(4.))
      );
      assert_eq!(result.strokes[0].transform, FreeformTransform {
         a:  0.,
         b:  2.,
         c:  -3.,
         d:  0.,
         tx: 10.,
         ty: 20.,
      });
      assert_eq!(result.strokes[0].point_role, FreeformInkPointRole::SplineControl);
   }
   #[test]
   fn maps_two_installed_inks_and_preserves_identifiers() {
      let record = point(0., 0., None, None);
      let input = drawing(&[ink("com.apple.ink.marker"), ink("com.apple.ink.fountainPen")], &[
         stroke(0, 1, &record, None),
         stroke(1, 1, &record, None),
      ]);
      let drawing = decode_pk_drawing(&input).unwrap();
      assert_eq!(
         drawing
            .strokes
            .iter()
            .map(|s| (&s.ink_type, &s.ink_identifier))
            .collect::<Vec<_>>(),
         vec![
            (&"marker".into(), &"com.apple.ink.marker".into()),
            (&"fountainPen".into(), &"com.apple.ink.fountainPen".into())
         ]
      );
   }
   #[test]
   fn maps_every_installed_ink_family_exactly() {
      for (identifier, family) in [
         ("com.apple.ink.pen", "pen"),
         ("com.apple.ink.marker", "marker"),
         ("com.apple.ink.pencil", "pencil"),
         ("com.apple.ink.monoline", "monoline"),
         ("com.apple.ink.fountainPen", "fountainPen"),
         ("com.apple.ink.watercolor", "watercolor"),
         ("com.apple.ink.crayon", "crayon"),
      ] {
         assert_eq!(ink_type_from_identifier(identifier), family);
      }
   }
   #[test]
   fn unknown_ink_is_not_normalized() {
      let record = point(0., 0., None, None);
      let drawing =
         decode_pk_drawing(&drawing(&[ink("vendor.ink")], &[stroke(0, 1, &record, None)])).unwrap();
      assert_eq!(drawing.strokes[0].ink_type, "vendor.ink");
      assert_eq!(drawing.strokes[0].ink_identifier, "vendor.ink");
   }
   #[test]
   fn missing_channels_are_absent() {
      let record = point(2., 3., None, None);
      let drawing =
         decode_pk_drawing(&drawing(&[ink("com.apple.ink.pen")], &[stroke(0, 1, &record, None)]))
            .unwrap();
      let point = drawing.strokes[0].points[0];
      assert_eq!(point.width, None);
      assert_eq!(point.force, None);
      assert_eq!(drawing.strokes[0].color, None);
      assert_eq!(drawing.required_content_version, None);
      assert_eq!(drawing.bounds, None);
   }
   #[test]
   fn rejects_bad_references_counts_and_varints() {
      let record = point(0., 0., None, None);
      assert!(
         decode_pk_drawing(&drawing(&[ink("com.apple.ink.pen")], &[stroke(1, 1, &record, None)]))
            .is_err()
      );
      assert!(
         decode_pk_drawing(&drawing(&[ink("com.apple.ink.pen")], &[stroke(0, 2, &record, None)]))
            .is_err()
      );
      let mut malformed = MAGIC.to_vec();
      malformed.extend([0x80; 10]);
      assert!(decode_pk_drawing(&malformed).is_err());
   }
}
