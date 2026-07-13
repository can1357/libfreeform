//! Record-bounded decoder for native geometric shapes.
//!
//! The CRL object graph contains many protobuf-like CRDT envelopes.  This
//! module only assigns a semantic value when its complete enclosing message is
//! present in the current [`NativeRecord`]; it deliberately does not scan
//! neighbouring records or group floating-point values by byte distance.

use super::envelope::NativeRecord;
use crate::types::{
   FreeformColor, FreeformDecodeError, FreeformFrame, FreeformGeometry, FreeformGradientStop,
   FreeformPaint, FreeformPath, FreeformPathCommand, FreeformPoint, FreeformShadow, FreeformSize,
   FreeformStrokeStyle, FreeformStyle, FreeformTransform,
};

#[derive(Clone, Copy)]
enum Wire<'a> {
   Varint(u64),
   Fixed32(f32),
   Fixed64(f64),
   Bytes(&'a [u8]),
}

#[derive(Clone, Copy)]
struct Field<'a> {
   number: u32,
   value:  Wire<'a>,
}

/// Decodes a single archive-bounded native shape record.
///
/// A record that has no currently-proven shape fields is still successful: the
/// caller retains `record.bytes` as the board item's raw data.  Truncated wire
/// values are reported rather than being padded with zeroes.
pub fn decode_shape(
   record: &NativeRecord<'_>,
) -> Result<
   (Option<String>, Option<FreeformPath>, FreeformGeometry, FreeformStyle),
   FreeformDecodeError,
> {
   let fields = fields(record.bytes)?;
   let preset = find_preset(&fields);
   let geometry = find_geometry(record.bytes, &fields);
   let path = if is_ink_record(record.bytes) {
      None
   } else {
      find_path(&fields)
   };
   let style = find_style(&fields);
   Ok((preset, path, geometry, style))
}

fn fields(data: &[u8]) -> Result<Vec<Field<'_>>, FreeformDecodeError> {
   let mut offset = 0;
   let mut result = Vec::new();
   while offset < data.len() {
      let (key, next) = varint(data, offset)?;
      offset = next;
      let number = u32::try_from(key >> 3)
         .ok()
         .filter(|number| *number != 0)
         .ok_or_else(|| FreeformDecodeError::invalid("shape record: invalid protobuf field"))?;
      let value = match key & 7 {
         0 => {
            let (value, next) = varint(data, offset)?;
            offset = next;
            Wire::Varint(value)
         },
         1 => {
            let bytes: [u8; 8] = data
               .get(offset..offset + 8)
               .ok_or_else(|| FreeformDecodeError::incomplete("shape record: truncated fixed64"))?
               .try_into()
               .map_err(|_| FreeformDecodeError::incomplete("shape record: truncated fixed64"))?;
            offset += 8;
            Wire::Fixed64(f64::from_le_bytes(bytes))
         },
         2 => {
            let (length, next) = varint(data, offset)?;
            offset = next;
            let length = usize::try_from(length)
               .map_err(|_| FreeformDecodeError::invalid("shape record: oversized message"))?;
            let end = offset
               .checked_add(length)
               .filter(|end| *end <= data.len())
               .ok_or_else(|| FreeformDecodeError::incomplete("shape record: truncated message"))?;
            let bytes = &data[offset..end];
            offset = end;
            Wire::Bytes(bytes)
         },
         5 => {
            let bytes: [u8; 4] = data
               .get(offset..offset + 4)
               .ok_or_else(|| FreeformDecodeError::incomplete("shape record: truncated fixed32"))?
               .try_into()
               .map_err(|_| FreeformDecodeError::incomplete("shape record: truncated fixed32"))?;
            offset += 4;
            Wire::Fixed32(f32::from_le_bytes(bytes))
         },
         _ => {
            return Err(FreeformDecodeError::invalid(
               "shape record: unsupported protobuf wire type",
            ));
         },
      };
      result.push(Field { number, value });
   }
   Ok(result)
}

fn varint(data: &[u8], mut offset: usize) -> Result<(u64, usize), FreeformDecodeError> {
   let mut value = 0u64;
   for shift in (0..64).step_by(7) {
      let byte = *data
         .get(offset)
         .ok_or_else(|| FreeformDecodeError::incomplete("shape record: truncated varint"))?;
      offset += 1;
      value |= u64::from(byte & 0x7f) << shift;
      if byte & 0x80 == 0 {
         return Ok((value, offset));
      }
   }
   Err(FreeformDecodeError::invalid("shape record: overlong varint"))
}

fn nested(field: Field<'_>) -> Option<Vec<Field<'_>>> {
   let Wire::Bytes(bytes) = field.value else {
      return None;
   };
   fields(bytes).ok()
}

fn finite(value: f64) -> Option<f64> {
   value.is_finite().then_some(value)
}

fn scalar(field: Field<'_>) -> Option<f64> {
   match field.value {
      Wire::Fixed32(value) => finite(f64::from(value)),
      Wire::Fixed64(value) => finite(value),
      Wire::Varint(value) => Some(value as f64),
      Wire::Bytes(_) => None,
   }
}

const fn bool_scalar(field: Field<'_>) -> Option<bool> {
   match field.value {
      Wire::Varint(0) => Some(false),
      Wire::Varint(1) => Some(true),
      _ => None,
   }
}

fn point(fields: &[Field<'_>]) -> Option<FreeformPoint> {
   let x = fields
      .iter()
      .find(|f| f.number == 1)
      .copied()
      .and_then(scalar)?;
   let y = fields
      .iter()
      .find(|f| f.number == 2)
      .copied()
      .and_then(scalar)?;
   Some(FreeformPoint { x, y })
}

fn point_field(field: Field<'_>) -> Option<FreeformPoint> {
   nested(field).as_deref().and_then(point)
}

fn find_preset(record_fields: &[Field<'_>]) -> Option<String> {
   for field in record_fields {
      let Wire::Bytes(bytes) = field.value else {
         continue;
      };
      if let Ok(value) = std::str::from_utf8(bytes) {
         if preset_name(value) {
            return Some(value.to_owned());
         }
      }
      if let Ok(children) = fields(bytes) {
         if let Some(value) = find_preset(&children) {
            return Some(value);
         }
      }
   }
   None
}

fn preset_name(value: &str) -> bool {
   let Some((name, revision)) = value.rsplit_once('_') else {
      return false;
   };
   !name.is_empty()
      && revision.as_bytes().iter().all(u8::is_ascii_digit)
      && name
         .bytes()
         .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn find_geometry(bytes: &[u8], fields: &[Field<'_>]) -> FreeformGeometry {
   let mut result = geometry_from_fields(fields);
   if result.frame.is_none() {
      result.frame = legacy_frame(bytes);
   }
   result
}

fn geometry_from_fields(fields: &[Field<'_>]) -> FreeformGeometry {
   let mut result = FreeformGeometry::default();
   for field in fields {
      let Some(children) = nested(*field) else {
         continue;
      };
      let position = children
         .iter()
         .find(|f| f.number == 1)
         .copied()
         .and_then(point_field);
      let size = children
         .iter()
         .find(|f| f.number == 2)
         .copied()
         .and_then(point_field);
      // A pair of points alone is ambiguous: the same wire shape describes a
      // linear-gradient vector.  A geometry archive carries at least one
      // geometry-only companion (angle, flip/validity, anchor, or affine).
      let geometry_trait = children.iter().any(|field| {
         (field.number == 3 && scalar(*field).is_some()) || (4..=9).contains(&field.number)
      });
      if result.frame.is_none() && geometry_trait {
         if let (Some(position), Some(size)) = (position, size) {
            let rotation = children
               .iter()
               .find(|f| f.number == 3)
               .copied()
               .and_then(scalar)
               .unwrap_or(0.0);
            result.frame =
               Some(FreeformFrame { x: position.x, y: position.y, w: size.x, h: size.y, rotation });
            result.horizontal_flip = children
               .iter()
               .find(|f| f.number == 4)
               .copied()
               .and_then(bool_scalar);
            result.vertical_flip = children
               .iter()
               .find(|f| f.number == 5)
               .copied()
               .and_then(bool_scalar);
            result.width_valid = children
               .iter()
               .find(|f| f.number == 6)
               .copied()
               .and_then(bool_scalar);
            result.height_valid = children
               .iter()
               .find(|f| f.number == 7)
               .copied()
               .and_then(bool_scalar);
            result.anchor = children
               .iter()
               .find(|f| f.number == 8)
               .copied()
               .and_then(point_field);
            result.transform = children
               .iter()
               .find(|f| f.number == 9)
               .copied()
               .and_then(transform_field);
            return result;
         }
      }
      let child = geometry_from_fields(&children);
      merge_geometry(&mut result, child);
   }
   result
}

const fn merge_geometry(destination: &mut FreeformGeometry, source: FreeformGeometry) {
   if destination.frame.is_none() {
      destination.frame = source.frame;
   }
   if destination.transform.is_none() {
      destination.transform = source.transform;
   }
   if destination.anchor.is_none() {
      destination.anchor = source.anchor;
   }
   if destination.horizontal_flip.is_none() {
      destination.horizontal_flip = source.horizontal_flip;
   }
   if destination.vertical_flip.is_none() {
      destination.vertical_flip = source.vertical_flip;
   }
   if destination.width_valid.is_none() {
      destination.width_valid = source.width_valid;
   }
   if destination.height_valid.is_none() {
      destination.height_valid = source.height_valid;
   }
}

fn transform_field(field: Field<'_>) -> Option<FreeformTransform> {
   let children = nested(field)?;
   let value = |number| {
      children
         .iter()
         .find(|f| f.number == number)
         .copied()
         .and_then(scalar)
   };
   Some(FreeformTransform {
      a:  value(1)?,
      b:  value(2)?,
      c:  value(3)?,
      d:  value(4)?,
      tx: value(5)?,
      ty: value(6)?,
   })
}

// Captured GeometryArchive positions use two adjacent point messages.  This
// accepts the complete grammar, not merely nearby floats, and is intentionally
// a frame-only fallback: the other geometry properties are not present there.
fn legacy_frame(bytes: &[u8]) -> Option<FreeformFrame> {
   let mut start = 0;
   while start + 26 <= bytes.len() {
      let Some(first) = geometry_point_at(bytes, start) else {
         start += 1;
         continue;
      };
      let second_at = start + 14;
      let Some(second) = geometry_point_at(bytes, second_at) else {
         start += 1;
         continue;
      };
      let rotation = match bytes.get(second_at + 12..second_at + 19) {
         Some([0x12, 0x05, 0x7d, a, b, c, d]) => {
            finite(-f64::from(f32::from_le_bytes([*a, *b, *c, *d])) * std::f64::consts::PI / 180.0)
               .unwrap_or(0.0)
         },
         _ => 0.0,
      };
      return Some(FreeformFrame { x: first.x, y: first.y, w: second.x, h: second.y, rotation });
   }
   None
}

fn geometry_point_at(bytes: &[u8], offset: usize) -> Option<FreeformPoint> {
   let point = bytes.get(offset..offset + 12)?;
   if point[0..3] != [0x22, 0x0a, 0x0d] || point[7] != 0x15 {
      return None;
   }
   Some(FreeformPoint {
      x: finite(f64::from(f32::from_le_bytes(point[3..7].try_into().ok()?)))?,
      y: finite(f64::from(f32::from_le_bytes(point[8..12].try_into().ok()?)))?,
   })
}

fn is_ink_record(bytes: &[u8]) -> bool {
   bytes
      .windows(b"com.apple.ink.".len())
      .any(|part| part == b"com.apple.ink.")
      || bytes
         .windows(b"PKStroke".len())
         .any(|part| part == b"PKStroke")
}

fn find_path(record_fields: &[Field<'_>]) -> Option<FreeformPath> {
   for field in record_fields {
      let Some(bytes) = (match field.value {
         Wire::Bytes(bytes) => Some(bytes),
         _ => None,
      }) else {
         continue;
      };
      if let Some(path) = path_message(bytes) {
         return Some(FreeformPath {
            commands:     path,
            natural_size: natural_size(bytes),
            raw_data:     bytes.to_vec(),
         });
      }
      if let Ok(children) = fields(bytes) {
         if let Some(path) = find_path(&children) {
            return Some(path);
         }
      }
   }
   None
}

fn path_message(bytes: &[u8]) -> Option<Vec<FreeformPathCommand>> {
   let nodes = fields(bytes).ok()?;
   let mut commands = Vec::new();
   for node in nodes {
      let Some(node) = nested(node) else { continue };
      let kind = node
         .iter()
         .find(|field| field.number == 1)
         .and_then(|field| match field.value {
            Wire::Varint(value) => Some(value),
            _ => None,
         });
      let points: Vec<_> = node
         .iter()
         .filter(|field| field.number == 2)
         .copied()
         .filter_map(point_field)
         .collect();
      let command = match (kind, points.as_slice()) {
         (Some(0), [point]) => FreeformPathCommand::Move { point: *point },
         (Some(1), [point]) => FreeformPathCommand::Line { point: *point },
         (Some(2), [control, point]) => {
            FreeformPathCommand::Quadratic { control: *control, point: *point }
         },
         (Some(3), [control_1, control_2, point]) => FreeformPathCommand::Cubic {
            control_1: *control_1,
            control_2: *control_2,
            point:     *point,
         },
         (Some(4), []) => FreeformPathCommand::Close,
         _ => continue,
      };
      commands.push(command);
   }
   (!commands.is_empty()
      && commands
         .iter()
         .any(|command| matches!(command, FreeformPathCommand::Move { .. })))
   .then_some(commands)
}

fn natural_size(bytes: &[u8]) -> Option<FreeformSize> {
   let fields = fields(bytes).ok()?;
   for field in fields {
      let Some(child) = nested(field) else { continue };
      if let Some(point) = point(&child) {
         return Some(FreeformSize { width: point.x, height: point.y });
      }
   }
   None
}

fn find_style(fields: &[Field<'_>]) -> FreeformStyle {
   let mut style = FreeformStyle::default();
   for field in fields {
      let Some(child) = nested(*field) else {
         continue;
      };
      // Only a complete visual-style envelope establishes that field 3 is
      // opacity; a geometry record also has an unrelated scalar at field 3.
      let fill = child
         .iter()
         .find(|f| f.number == 1)
         .copied()
         .and_then(paint_field);
      let stroke = child
         .iter()
         .find(|f| f.number == 2)
         .copied()
         .and_then(stroke_field);
      let shadows = child
         .iter()
         .filter(|f| f.number == 4)
         .copied()
         .filter_map(shadow_field)
         .collect::<Vec<_>>();
      if fill.is_some() || stroke.is_some() || !shadows.is_empty() {
         if style.fill.is_none() {
            style.fill = fill;
         }
         if style.stroke.is_none() {
            style.stroke = stroke;
         }
         if style.opacity.is_none() {
            style.opacity = child
               .iter()
               .find(|f| f.number == 3)
               .copied()
               .and_then(scalar)
               .filter(|value| (0.0..=1.0).contains(value));
         }
         if style.shadows.is_empty() {
            style.shadows = shadows;
         }
      }
      let nested_style = find_style(&child);
      if style.fill.is_none() {
         style.fill = nested_style.fill;
      }
      if style.stroke.is_none() {
         style.stroke = nested_style.stroke;
      }
      if style.opacity.is_none() {
         style.opacity = nested_style.opacity;
      }
      if style.shadows.is_empty() {
         style.shadows = nested_style.shadows;
      }
   }
   style
}

fn paint_field(field: Field<'_>) -> Option<FreeformPaint> {
   let Wire::Bytes(bytes) = field.value else {
      return None;
   };
   let fields = fields(bytes).ok()?;
   if let Some(color) = color(&fields) {
      return Some(FreeformPaint::Solid { color });
   }
   if let Some(asset_id) = fields.iter().find(|f| f.number == 5).and_then(string_field) {
      let technique = fields.iter().find(|f| f.number == 6).and_then(string_field);
      return Some(FreeformPaint::Image { asset_id, technique });
   }
   let stops = fields
      .iter()
      .filter(|f| f.number == 3)
      .copied()
      .filter_map(stop_field)
      .collect::<Vec<_>>();
   if !stops.is_empty() {
      if let Some(center) = fields
         .iter()
         .find(|f| f.number == 4)
         .copied()
         .and_then(point_field)
      {
         let radius = fields
            .iter()
            .find(|f| f.number == 5)
            .copied()
            .and_then(scalar)?;
         return Some(FreeformPaint::RadialGradient { center, radius, stops });
      }
      let start = fields
         .iter()
         .find(|f| f.number == 1)
         .copied()
         .and_then(point_field)?;
      let end = fields
         .iter()
         .find(|f| f.number == 2)
         .copied()
         .and_then(point_field)?;
      return Some(FreeformPaint::LinearGradient { start, end, stops });
   }
   None
}

fn color(fields: &[Field<'_>]) -> Option<FreeformColor> {
   let value = |number| {
      fields
         .iter()
         .find(|field| field.number == number)
         .copied()
         .and_then(scalar)
         .filter(|value| (0.0..=1.0).contains(value))
   };
   let red = value(1)?;
   let green = value(2)?;
   let blue = value(3)?;
   let alpha = value(4).unwrap_or(1.0);
   let color_space = fields
      .iter()
      .find(|field| field.number == 5)
      .and_then(string_field)
      .unwrap_or_else(|| "sRGB".into());
   Some(FreeformColor { color_space, red, green, blue, alpha, hex: hex(red, green, blue) })
}

fn hex(red: f64, green: f64, blue: f64) -> String {
   format!(
      "#{:02x}{:02x}{:02x}",
      (red * 255.0).round() as u8,
      (green * 255.0).round() as u8,
      (blue * 255.0).round() as u8
   )
}

fn stop_field(field: Field<'_>) -> Option<FreeformGradientStop> {
   let fields = nested(field)?;
   Some(FreeformGradientStop {
      fraction:   fields
         .iter()
         .find(|f| f.number == 1)
         .copied()
         .and_then(scalar)
         .filter(|value| (0.0..=1.0).contains(value))?,
      inflection: fields
         .iter()
         .find(|f| f.number == 2)
         .copied()
         .and_then(scalar),
      color:      fields
         .iter()
         .find(|f| f.number == 3)
         .copied()
         .and_then(paint_field)
         .and_then(|paint| match paint {
            FreeformPaint::Solid { color } => Some(color),
            _ => None,
         })?,
   })
}

fn stroke_field(field: Field<'_>) -> Option<FreeformStrokeStyle> {
   let fields = nested(field)?;
   Some(FreeformStrokeStyle {
      paint:       fields
         .iter()
         .find(|f| f.number == 1)
         .copied()
         .and_then(paint_field)?,
      width:       fields
         .iter()
         .find(|f| f.number == 2)
         .copied()
         .and_then(scalar)?,
      dash:        fields
         .iter()
         .filter(|f| f.number == 3)
         .copied()
         .filter_map(scalar)
         .collect(),
      dash_offset: fields
         .iter()
         .find(|f| f.number == 4)
         .copied()
         .and_then(scalar),
      cap:         fields.iter().find(|f| f.number == 5).and_then(string_field),
      join:        fields.iter().find(|f| f.number == 6).and_then(string_field),
      miter_limit: fields
         .iter()
         .find(|f| f.number == 7)
         .copied()
         .and_then(scalar),
      tail_end:    fields.iter().find(|f| f.number == 8).and_then(string_field),
      head_end:    fields.iter().find(|f| f.number == 9).and_then(string_field),
   })
}

fn shadow_field(field: Field<'_>) -> Option<FreeformShadow> {
   let fields = nested(field)?;
   Some(FreeformShadow {
      kind:     fields
         .iter()
         .find(|f| f.number == 1)
         .and_then(string_field)?,
      color:    fields
         .iter()
         .find(|f| f.number == 2)
         .copied()
         .and_then(paint_field)
         .and_then(|paint| match paint {
            FreeformPaint::Solid { color } => Some(color),
            _ => None,
         })?,
      offset_x: fields
         .iter()
         .find(|f| f.number == 3)
         .copied()
         .and_then(scalar)?,
      offset_y: fields
         .iter()
         .find(|f| f.number == 4)
         .copied()
         .and_then(scalar)?,
      radius:   fields
         .iter()
         .find(|f| f.number == 5)
         .copied()
         .and_then(scalar)?,
      opacity:  fields
         .iter()
         .find(|f| f.number == 6)
         .copied()
         .and_then(scalar)
         .filter(|value| (0.0..=1.0).contains(value)),
      angle:    fields
         .iter()
         .find(|f| f.number == 7)
         .copied()
         .and_then(scalar),
   })
}

fn string_field(field: &Field<'_>) -> Option<String> {
   let Wire::Bytes(bytes) = field.value else {
      return None;
   };
   std::str::from_utf8(bytes).ok().map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
   use super::*;

   fn varint(mut value: u64) -> Vec<u8> {
      let mut bytes = Vec::new();
      loop {
         let mut byte = (value & 0x7f) as u8;
         value >>= 7;
         if value != 0 {
            byte |= 0x80;
         }
         bytes.push(byte);
         if value == 0 {
            return bytes;
         }
      }
   }

   fn field(number: u32, wire: u8, value: &[u8]) -> Vec<u8> {
      let mut bytes = varint(u64::from(number << 3 | u32::from(wire)));
      if wire == 2 {
         bytes.extend(varint(value.len() as u64));
      }
      bytes.extend(value);
      bytes
   }

   fn message(number: u32, value: Vec<u8>) -> Vec<u8> {
      field(number, 2, &value)
   }

   fn float(number: u32, value: f32) -> Vec<u8> {
      field(number, 5, &value.to_le_bytes())
   }

   fn point(x: f32, y: f32) -> Vec<u8> {
      let mut bytes = float(1, x);
      bytes.extend(float(2, y));
      bytes
   }

   fn color(red: f32, green: f32, blue: f32, alpha: f32) -> Vec<u8> {
      let mut bytes = float(1, red);
      bytes.extend(float(2, green));
      bytes.extend(float(3, blue));
      bytes.extend(float(4, alpha));
      bytes
   }

   fn record(bytes: &[u8]) -> NativeRecord<'_> {
      NativeRecord { owner_id: "00000000-0000-0000-0000-000000000000".into(), offset: 0, bytes }
   }

   #[test]
   fn retains_preset_identifiers() {
      for preset in ["Circle_950", "Triangle_950", "Parallelogram_950"] {
         let bytes = message(15, preset.as_bytes().to_vec());
         let (found, ..) = decode_shape(&record(&bytes)).unwrap();
         assert_eq!(found.as_deref(), Some(preset));
      }
   }

   #[test]
   fn preserves_cubic_commands_and_subpaths() {
      let mut path = Vec::new();
      for (kind, points) in [
         (0, vec![point(1.0, 2.0)]),
         (3, vec![point(3.0, 4.0), point(5.0, 6.0), point(7.0, 8.0)]),
         (4, vec![]),
         (0, vec![point(9.0, 10.0)]),
         (1, vec![point(11.0, 12.0)]),
      ] {
         let mut node = field(1, 0, &varint(kind));
         for control in points {
            node.extend(message(2, control));
         }
         path.extend(message(1, node));
      }
      let bytes = message(14, path);
      let (_, found, ..) = decode_shape(&record(&bytes)).unwrap();
      let path = found.unwrap();
      assert_eq!(path.commands.len(), 5);
      assert!(matches!(path.commands[1], FreeformPathCommand::Cubic {
         control_1: FreeformPoint { x: 3.0, y: 4.0 },
         control_2: FreeformPoint { x: 5.0, y: 6.0 },
         point:     FreeformPoint { x: 7.0, y: 8.0 },
      }));
      assert!(matches!(path.commands[2], FreeformPathCommand::Close));
      assert!(matches!(path.commands[3], FreeformPathCommand::Move { .. }));
   }

   #[test]
   fn preserves_geometry_flips_and_affine_transform() {
      let mut affine = float(1, 1.0);
      affine.extend(float(2, 2.0));
      affine.extend(float(3, 3.0));
      affine.extend(float(4, 4.0));
      affine.extend(float(5, 5.0));
      affine.extend(float(6, 6.0));
      let mut geometry = message(1, point(10.0, 20.0));
      geometry.extend(message(2, point(30.0, 40.0)));
      geometry.extend(float(3, 0.5));
      geometry.extend(field(4, 0, &[1]));
      geometry.extend(field(5, 0, &[0]));
      geometry.extend(field(6, 0, &[1]));
      geometry.extend(field(7, 0, &[0]));
      geometry.extend(message(8, point(2.0, 3.0)));
      geometry.extend(message(9, affine));
      let bytes = message(1, geometry);
      let (_, _, actual, _) = decode_shape(&record(&bytes)).unwrap();
      assert_eq!(actual.frame.unwrap(), FreeformFrame {
         x:        10.0,
         y:        20.0,
         w:        30.0,
         h:        40.0,
         rotation: 0.5,
      });
      assert_eq!(actual.horizontal_flip, Some(true));
      assert_eq!(actual.vertical_flip, Some(false));
      assert_eq!(actual.width_valid, Some(true));
      assert_eq!(actual.height_valid, Some(false));
      assert_eq!(actual.anchor, Some(FreeformPoint { x: 2.0, y: 3.0 }));
      assert_eq!(actual.transform.unwrap().ty, 6.0);
   }

   #[test]
   fn retains_fill_alpha_and_gradient_stops() {
      let mut stop = float(1, 0.25);
      stop.extend(float(2, 0.4));
      stop.extend(message(3, color(0.1, 0.2, 0.3, 0.4)));
      let mut gradient = message(1, point(0.0, 0.0));
      gradient.extend(message(2, point(100.0, 0.0)));
      gradient.extend(message(3, stop));
      let style = message(1, gradient);
      let bytes = message(1, style);
      let (_, _, _, actual) = decode_shape(&record(&bytes)).unwrap();
      let Some(FreeformPaint::LinearGradient { stops, .. }) = actual.fill else {
         panic!("missing gradient")
      };
      assert!((stops[0].color.alpha - 0.4).abs() < 1e-6);
      assert!(matches!(stops[0].inflection, Some(value) if (value - 0.4).abs() < 1e-6));
   }

   #[test]
   fn keeps_fill_and_stroke_colors_separate_with_line_style_and_shadow() {
      let fill = color(1.0, 0.0, 0.0, 0.75);
      let mut stroke = message(1, color(0.0, 0.0, 1.0, 0.5));
      stroke.extend(float(2, 3.0));
      stroke.extend(float(3, 4.0));
      stroke.extend(float(3, 5.0));
      stroke.extend(float(4, 2.0));
      stroke.extend(message(5, b"round".to_vec()));
      stroke.extend(message(6, b"bevel".to_vec()));
      stroke.extend(float(7, 8.0));
      stroke.extend(message(8, b"diamond".to_vec()));
      stroke.extend(message(9, b"arrow".to_vec()));
      let mut shadow = message(1, b"drop".to_vec());
      shadow.extend(message(2, color(0.0, 1.0, 0.0, 0.25)));
      shadow.extend(float(3, 1.0));
      shadow.extend(float(4, 2.0));
      shadow.extend(float(5, 3.0));
      shadow.extend(float(6, 0.6));
      let mut style = message(1, fill);
      style.extend(message(2, stroke));
      style.extend(float(3, 0.9));
      style.extend(message(4, shadow));
      let bytes = message(1, style);
      let (_, _, _, actual) = decode_shape(&record(&bytes)).unwrap();
      let Some(FreeformPaint::Solid { color: fill }) = actual.fill else {
         panic!("missing fill")
      };
      let stroke = actual.stroke.unwrap();
      let FreeformPaint::Solid { color: outline } = stroke.paint else {
         panic!("missing stroke")
      };
      assert_eq!((fill.red, fill.blue), (1.0, 0.0));
      assert_eq!((outline.red, outline.blue), (0.0, 1.0));
      assert_eq!(stroke.dash, [4.0, 5.0]);
      assert_eq!(stroke.tail_end.as_deref(), Some("diamond"));
      assert_eq!(stroke.head_end.as_deref(), Some("arrow"));
      assert_eq!(actual.shadows[0].color.green, 1.0);
   }

   #[test]
   fn never_reads_ink_nodes_as_shape_outline() {
      let mut node = field(1, 0, &[0]);
      node.extend(message(2, point(4.0, 5.0)));
      let mut bytes = b"com.apple.ink.pen".to_vec();
      bytes.extend(message(14, message(1, node)));
      assert!(decode_shape(&record(&bytes)).is_err(), "non-protobuf ink prefix is invalid");

      let mut marked = Vec::new();
      marked.extend(message(1, b"com.apple.ink.pen".to_vec()));
      let mut ink_node = field(1, 0, &[0]);
      ink_node.extend(message(2, point(4.0, 5.0)));
      marked.extend(message(14, message(1, ink_node)));
      assert!(decode_shape(&record(&marked)).unwrap().1.is_none());
   }
}
