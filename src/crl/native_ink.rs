//! Record-bounded decoder for Freeform's native ink protobuf records.
//!
//! The archive has evolved several compact point encodings.  This decoder only
//! projects channels whose wire representation proves their meaning; every
//! stroke always keeps its original bounded record for replay of newer forms.

use super::envelope::NativeRecord;
use crate::{
   pkdrawing::to_hex,
   types::{
      FreeformColor, FreeformDecodeError, FreeformInkPoint, FreeformInkPointRole,
      FreeformInkStroke, FreeformTransform,
   },
};

#[derive(Clone, Copy)]
enum Value<'a> {
   Varint(u64),
   Fixed32(u32),
   Fixed64(u64),
   Bytes(&'a [u8]),
}

#[derive(Clone, Copy)]
struct Field<'a> {
   number: u32,
   value:  Value<'a>,
}

fn invalid(what: &str) -> FreeformDecodeError {
   FreeformDecodeError::invalid(format!("CRL native ink: {what}"))
}

fn varint(data: &[u8], cursor: &mut usize) -> Result<u64, FreeformDecodeError> {
   let mut value = 0u64;
   for shift in (0..64).step_by(7) {
      let byte = *data
         .get(*cursor)
         .ok_or_else(|| invalid("truncated varint"))?;
      *cursor += 1;
      if shift == 63 && byte > 1 {
         return Err(invalid("varint overflow"));
      }
      value |= u64::from(byte & 0x7f) << shift;
      if byte & 0x80 == 0 {
         return Ok(value);
      }
   }
   Err(invalid("overlong varint"))
}

fn parse_fields(data: &[u8]) -> Result<Vec<Field<'_>>, FreeformDecodeError> {
   let mut cursor = 0;
   let mut out = Vec::new();
   while cursor != data.len() {
      let key = varint(data, &mut cursor)?;
      let number = u32::try_from(key >> 3).map_err(|_| invalid("field number overflow"))?;
      if number == 0 {
         return Err(invalid("zero field number"));
      }
      let value = match key & 7 {
         0 => Value::Varint(varint(data, &mut cursor)?),
         1 => {
            let bytes: [u8; 8] = data
               .get(cursor..cursor + 8)
               .ok_or_else(|| invalid("truncated fixed64"))?
               .try_into()
               .map_err(|_| invalid("truncated fixed64"))?;
            cursor += 8;
            Value::Fixed64(u64::from_le_bytes(bytes))
         },
         2 => {
            let len = usize::try_from(varint(data, &mut cursor)?)
               .map_err(|_| invalid("length overflow"))?;
            let end = cursor
               .checked_add(len)
               .ok_or_else(|| invalid("length overflow"))?;
            let bytes = data
               .get(cursor..end)
               .ok_or_else(|| invalid("truncated bytes"))?;
            cursor = end;
            Value::Bytes(bytes)
         },
         5 => {
            let bytes: [u8; 4] = data
               .get(cursor..cursor + 4)
               .ok_or_else(|| invalid("truncated fixed32"))?
               .try_into()
               .map_err(|_| invalid("truncated fixed32"))?;
            cursor += 4;
            Value::Fixed32(u32::from_le_bytes(bytes))
         },
         _ => return Err(invalid("unsupported protobuf wire type")),
      };
      out.push(Field { number, value });
   }
   Ok(out)
}

fn number(value: Value<'_>) -> Option<f64> {
   match value {
      Value::Varint(value) => Some(value as f64),
      Value::Fixed32(value) => {
         let value = f32::from_bits(value) as f64;
         value.is_finite().then_some(value)
      },
      Value::Fixed64(value) => {
         let value = f64::from_bits(value);
         value.is_finite().then_some(value)
      },
      Value::Bytes(_) => None,
   }
}

fn direct_number(fields: &[Field<'_>], field_number: u32) -> Option<f64> {
   fields
      .iter()
      .find(|field| field.number == field_number)
      .and_then(|field| number(field.value))
}

fn utf8<'a>(fields: &'a [Field<'a>]) -> impl Iterator<Item = &'a str> + 'a {
   fields.iter().filter_map(|field| match field.value {
      Value::Bytes(bytes) => std::str::from_utf8(bytes).ok(),
      _ => None,
   })
}

fn ink_identifier<'a>(fields: &'a [Field<'a>]) -> Option<&'a str> {
   utf8(fields).find(|value| {
      value.starts_with("com.apple.ink.") || value.starts_with("com.apple.pencilkit.")
   })
}

fn ink_type(identifier: &str) -> String {
   identifier
      .rsplit('.')
      .next()
      .unwrap_or(identifier)
      .to_owned()
}

fn packed_numbers(bytes: &[u8]) -> Option<Vec<f64>> {
   if bytes.len() % 4 == 0 && !bytes.is_empty() {
      let values: Option<Vec<_>> = bytes
         .chunks_exact(4)
         .map(|chunk| {
            let bits = u32::from_le_bytes(chunk.try_into().ok()?);
            let value = f32::from_bits(bits) as f64;
            value.is_finite().then_some(value)
         })
         .collect();
      return values;
   }
   if bytes.len() % 8 == 0 && !bytes.is_empty() {
      let values: Option<Vec<_>> = bytes
         .chunks_exact(8)
         .map(|chunk| {
            let bits = u64::from_le_bytes(chunk.try_into().ok()?);
            let value = f64::from_bits(bits);
            value.is_finite().then_some(value)
         })
         .collect();
      return values;
   }
   None
}

fn decode_transform(fields: &[Field<'_>]) -> Option<FreeformTransform> {
   for field in fields {
      let Value::Bytes(bytes) = field.value else {
         continue;
      };
      if let Some(values) = packed_numbers(bytes).filter(|values| values.len() == 6) {
         return Some(FreeformTransform {
            a:  values[0],
            b:  values[1],
            c:  values[2],
            d:  values[3],
            tx: values[4],
            ty: values[5],
         });
      }
      let Ok(nested) = parse_fields(bytes) else {
         continue;
      };
      let values: Option<Vec<_>> = (1..=6)
         .map(|number| direct_number(&nested, number))
         .collect();
      if let Some(values) = values {
         return Some(FreeformTransform {
            a:  values[0],
            b:  values[1],
            c:  values[2],
            d:  values[3],
            tx: values[4],
            ty: values[5],
         });
      }
   }
   None
}

fn color_hex(red: f64, green: f64, blue: f64) -> String {
   to_hex(red, green, blue)
}

fn decode_color(fields: &[Field<'_>]) -> Option<FreeformColor> {
   for field in fields {
      let Value::Bytes(bytes) = field.value else {
         continue;
      };
      let Ok(nested) = parse_fields(bytes) else {
         continue;
      };
      let Some(red) = direct_number(&nested, 1) else {
         continue;
      };
      let Some(green) = direct_number(&nested, 2) else {
         continue;
      };
      let Some(blue) = direct_number(&nested, 3) else {
         continue;
      };
      let Some(alpha) = direct_number(&nested, 4) else {
         continue;
      };
      // The color-space is required by the public lossless model. Do not invent
      // one merely because the numerical channels look like sRGB.
      let Some(color_space) =
         utf8(&nested).find(|text| !text.starts_with("com.apple.ink.") && !text.is_empty())
      else {
         continue;
      };
      return Some(FreeformColor {
         color_space: color_space.to_owned(),
         red,
         green,
         blue,
         alpha,
         hex: color_hex(red, green, blue),
      });
   }
   None
}

fn decode_point(body: &[u8]) -> Option<FreeformInkPoint> {
   let fields = parse_fields(body).ok()?;
   let (x, y) = if let (Some(x), Some(y)) = (direct_number(&fields, 1), direct_number(&fields, 2)) {
      (x, y)
   } else {
      let pair = fields.iter().find_map(|field| match field.value {
         Value::Bytes(bytes) if field.number == 2 => {
            packed_numbers(bytes).filter(|values| values.len() == 2)
         },
         _ => None,
      })?;
      (pair[0], pair[1])
   };
   Some(FreeformInkPoint {
      x,
      y,
      time_offset: direct_number(&fields, 3),
      width: direct_number(&fields, 4),
      height: direct_number(&fields, 5),
      force: direct_number(&fields, 6),
      opacity: direct_number(&fields, 7),
      azimuth: direct_number(&fields, 8),
      altitude: direct_number(&fields, 9),
      secondary_scale: direct_number(&fields, 10),
      threshold: direct_number(&fields, 11),
   })
}

fn direct_points(fields: &[Field<'_>]) -> Vec<FreeformInkPoint> {
   let mut points = Vec::new();
   for field in fields {
      let Value::Bytes(body) = field.value else {
         continue;
      };
      // In the observed native stroke message fields 2 and 3 carry transform
      // and color respectively. Their nested numerical tags overlap point tags,
      // so dispatch those style containers before interpreting point records.
      if field.number == 2 && decode_transform(&[*field]).is_some() {
         continue;
      }
      if field.number == 3 && decode_color(&[*field]).is_some() {
         continue;
      }
      if let Some(point) = decode_point(body) {
         points.push(point);
         continue;
      }
      let Ok(container) = parse_fields(body) else {
         continue;
      };
      for child in container {
         if let Value::Bytes(point_body) = child.value {
            if let Some(point) = decode_point(point_body) {
               points.push(point);
            }
         }
      }
   }
   points
}

fn decode_stroke(data: &[u8]) -> Result<Option<FreeformInkStroke>, FreeformDecodeError> {
   let fields = parse_fields(data)?;
   let Some(identifier) = ink_identifier(&fields) else {
      return Ok(None);
   };
   let Some(transform) = decode_transform(&fields) else {
      // `FreeformInkStroke::transform` cannot represent an unknown transform.
      // Leaving this record to its owner's raw fallback avoids creating identity.
      return Ok(None);
   };
   let points = direct_points(&fields);
   Ok(Some(FreeformInkStroke {
      ink_type: ink_type(identifier),
      ink_identifier: identifier.to_owned(),
      color: decode_color(&fields),
      transform,
      point_role: FreeformInkPointRole::SplineControl,
      points,
      visible_ranges: None,
      random_seed: None,
      raw_data: data.to_vec(),
   }))
}

fn collect_strokes(
   data: &[u8],
   strokes: &mut Vec<FreeformInkStroke>,
) -> Result<(), FreeformDecodeError> {
   let fields = parse_fields(data)?;
   if let Some(stroke) = decode_stroke(data)? {
      strokes.push(stroke);
      return Ok(());
   }
   for field in fields {
      if let Value::Bytes(body) = field.value {
         // Arbitrary byte strings are not necessarily messages. They only need
         // recursive traversal when they parse as a complete protobuf message.
         if parse_fields(body).is_ok() {
            collect_strokes(body, strokes)?;
         }
      }
   }
   Ok(())
}

/// Decodes every explicitly bounded native-ink stroke in one archive record.
///
/// The parser walks protobuf tags instead of assuming a compact-record stride.
/// Unsupported compact point/mask variants remain in
/// `FreeformInkStroke::raw_data`; a record lacking a fully proven transform is
/// deliberately not projected, because the public stroke type cannot represent
/// an absent transform.
pub fn decode_native_ink(
   record: &NativeRecord<'_>,
) -> Result<Vec<FreeformInkStroke>, FreeformDecodeError> {
   let mut strokes = Vec::new();
   collect_strokes(record.bytes, &mut strokes)?;
   Ok(strokes)
}

#[cfg(test)]
mod tests {
   use super::*;

   fn varint(mut value: u64) -> Vec<u8> {
      let mut bytes = Vec::new();
      while value >= 0x80 {
         bytes.push((value as u8) | 0x80);
         value >>= 7;
      }
      bytes.push(value as u8);
      bytes
   }

   fn bytes(field: u32, value: &[u8]) -> Vec<u8> {
      let mut out = varint(u64::from(field) << 3 | 2);
      out.extend(varint(value.len() as u64));
      out.extend(value);
      out
   }

   fn fixed(field: u32, value: f32) -> Vec<u8> {
      let mut out = varint(u64::from(field) << 3 | 5);
      out.extend(value.to_le_bytes());
      out
   }

   fn transform() -> Vec<u8> {
      let mut out = Vec::new();
      for (field, value) in [(1, 2.0), (2, 0.5), (3, -0.5), (4, 3.0), (5, 17.0), (6, -9.0)] {
         out.extend(fixed(field, value));
      }
      out
   }

   fn point(x: f32, y: f32, width: f32, force: f32) -> Vec<u8> {
      let mut out = Vec::new();
      // Deliberately reordered channels exercise tag-based decoding.
      out.extend(fixed(6, force));
      out.extend(fixed(4, width));
      out.extend(fixed(2, y));
      out.extend(fixed(1, x));
      out.extend(fixed(3, 0.25));
      out.extend(fixed(5, 8.5));
      out.extend(fixed(7, 0.625));
      out.extend(fixed(8, 1.25));
      out.extend(fixed(9, 0.75));
      out
   }

   fn color() -> Vec<u8> {
      let mut out = Vec::new();
      out.extend(bytes(5, b"extended-sRGB"));
      out.extend(fixed(4, 0.375));
      out.extend(fixed(1, 0.25));
      out.extend(fixed(3, 0.75));
      out.extend(fixed(2, 0.5));
      out
   }

   fn stroke(identifier: &str, compact: bool) -> Vec<u8> {
      let mut out = Vec::new();
      out.extend(bytes(1, identifier.as_bytes()));
      out.extend(bytes(2, &transform()));
      out.extend(bytes(3, &color()));
      if compact {
         // Unknown compact bytes have no complete tagged coordinate record.
         out.extend(bytes(4, &[0x0a, 0x08, 1, 2, 3, 4, 5, 6, 7, 8]));
      } else {
         out.extend(bytes(4, &point(12.5, -3.25, 7.75, 0.875)));
         out.extend(bytes(4, &point(19.0, 4.0, 9.5, 0.375)));
      }
      out
   }

   #[test]
   fn decodes_tagged_native_ink_without_style_defaults() {
      let first = stroke("com.apple.ink.pen", false);
      let second = stroke("com.apple.ink.marker", true);
      let mut archive = bytes(9, &first);
      archive.extend(bytes(9, &second));
      let record = NativeRecord { owner_id: "ink-owner".into(), offset: 0, bytes: &archive };

      let strokes = decode_native_ink(&record).unwrap();
      assert_eq!(strokes.len(), 2);
      assert_eq!(strokes[0].ink_identifier, "com.apple.ink.pen");
      assert_eq!(strokes[0].ink_type, "pen");
      assert_eq!(strokes[0].transform.a, 2.0);
      assert_eq!(strokes[0].transform.b, 0.5);
      assert_eq!(strokes[0].transform.c, -0.5);
      assert_eq!(strokes[0].transform.d, 3.0);
      assert_eq!(strokes[0].transform.tx, 17.0);
      assert_eq!(strokes[0].transform.ty, -9.0);
      assert_eq!(strokes[0].points.len(), 2);
      assert_eq!(strokes[0].points[0].width, Some(7.75));
      assert_eq!(strokes[0].points[0].force, Some(0.875));
      assert_eq!(strokes[0].points[0].height, Some(8.5));
      assert_eq!(strokes[0].points[0].opacity, Some(0.625));
      assert_eq!(strokes[0].color.as_ref().map(|color| color.alpha), Some(0.375));
      assert_eq!(
         strokes[0]
            .color
            .as_ref()
            .map(|color| color.color_space.as_str()),
         Some("extended-sRGB")
      );
      assert!(strokes[1].points.is_empty());
      assert_eq!(strokes[1].raw_data, second);
   }
}
