//! Record-bounded decoding for native connectors and groups.
//!
//! CRL records are protobuf-like messages embedded in the native archive.  This
//! module deliberately walks only the supplied record; joining endpoint and
//! group references to the typed board-item graph belongs to the archive
//! integrator, which has that graph available.

use super::envelope::NativeRecord;
use crate::types::{
   FreeformConnectorEndpoint, FreeformDecodeError, FreeformPath, FreeformPathCommand,
   FreeformPoint, FreeformTransform,
};

#[derive(Clone, Copy)]
enum WireValue<'a> {
   Varint(u64),
   Fixed64(u64),
   Bytes(&'a [u8]),
   Fixed32(u32),
}

fn read_varint(bytes: &[u8], mut at: usize) -> Result<(u64, usize), ()> {
   let mut value = 0u64;
   for shift in 0..10 {
      let byte = *bytes.get(at).ok_or(())?;
      at += 1;
      value |= u64::from(byte & 0x7f) << (shift * 7);
      if byte & 0x80 == 0 {
         return Ok((value, at));
      }
   }
   Err(())
}

fn walk_message<'a>(
   bytes: &'a [u8],
   visit: &mut dyn FnMut(u64, WireValue<'a>) -> Result<(), ()>,
) -> Result<(), ()> {
   let mut at = 0;
   while at < bytes.len() {
      let (tag, next) = read_varint(bytes, at)?;
      at = next;
      let field = tag >> 3;
      if field == 0 {
         return Err(());
      }
      let value = match tag & 7 {
         0 => {
            let (value, next) = read_varint(bytes, at)?;
            at = next;
            WireValue::Varint(value)
         },
         1 => {
            let raw: [u8; 8] = bytes
               .get(at..at + 8)
               .ok_or(())?
               .try_into()
               .map_err(|_| ())?;
            at += 8;
            WireValue::Fixed64(u64::from_le_bytes(raw))
         },
         2 => {
            let (len, next) = read_varint(bytes, at)?;
            let len = usize::try_from(len).map_err(|_| ())?;
            let end = next.checked_add(len).ok_or(())?;
            let value = bytes.get(next..end).ok_or(())?;
            at = end;
            WireValue::Bytes(value)
         },
         5 => {
            let raw: [u8; 4] = bytes
               .get(at..at + 4)
               .ok_or(())?
               .try_into()
               .map_err(|_| ())?;
            at += 4;
            WireValue::Fixed32(u32::from_le_bytes(raw))
         },
         _ => return Err(()),
      };
      visit(field, value)?;
   }
   Ok(())
}

fn invalid_record(record: &NativeRecord<'_>, kind: &str) -> FreeformDecodeError {
   FreeformDecodeError::invalid(format!("{kind} record at {} has invalid protobuf", record.offset))
}

fn utf8(bytes: &[u8]) -> Option<String> {
   std::str::from_utf8(bytes).ok().map(str::to_owned)
}

fn uuid_from_bytes(bytes: &[u8]) -> Option<String> {
   let bytes: [u8; 16] = bytes.try_into().ok()?;
   let mut value = String::with_capacity(36);
   for (index, byte) in bytes.iter().enumerate() {
      if matches!(index, 4 | 6 | 8 | 10) {
         value.push('-');
      }
      use std::fmt::Write as _;
      write!(&mut value, "{byte:02x}").expect("writing to String cannot fail");
   }
   Some(value)
}

fn uuid_from_value(value: WireValue<'_>) -> Option<String> {
   let WireValue::Bytes(bytes) = value else {
      return None;
   };
   uuid_from_bytes(bytes).or_else(|| {
      let value = utf8(bytes)?;
      is_uuid(&value).then_some(value)
   })
}

fn is_uuid(value: &str) -> bool {
   let bytes = value.as_bytes();
   bytes.len() == 36
      && matches!(bytes[8], b'-')
      && matches!(bytes[13], b'-')
      && matches!(bytes[18], b'-')
      && matches!(bytes[23], b'-')
      && bytes
         .iter()
         .enumerate()
         .all(|(index, byte)| matches!(index, 8 | 13 | 18 | 23) || byte.is_ascii_hexdigit())
}

fn scalar(value: WireValue<'_>) -> Option<f64> {
   match value {
      WireValue::Fixed32(bits) => Some(f64::from(f32::from_bits(bits))),
      WireValue::Fixed64(bits) => Some(f64::from_bits(bits)),
      _ => None,
   }
}

fn point(bytes: &[u8]) -> Result<Option<FreeformPoint>, ()> {
   let mut x = None;
   let mut y = None;
   walk_message(bytes, &mut |field, value| {
      match field {
         1 => x = scalar(value),
         2 => y = scalar(value),
         _ => {},
      }
      Ok(())
   })?;
   Ok(match (x, y) {
      (Some(x), Some(y)) if x.is_finite() && y.is_finite() => Some(FreeformPoint { x, y }),
      _ => None,
   })
}

fn endpoint(bytes: &[u8]) -> Result<FreeformConnectorEndpoint, ()> {
   let mut item_id = None;
   let mut magnet = None;
   let mut normalized_position = None;
   let mut point_value = None;
   let mut line_end = None;
   walk_message(bytes, &mut |field, value| {
      match (field, value) {
         (1, value) => item_id = uuid_from_value(value),
         (2, WireValue::Bytes(value)) => magnet = utf8(value),
         (3, WireValue::Bytes(value)) => normalized_position = point(value)?,
         (4, WireValue::Bytes(value)) => point_value = point(value)?,
         (5, WireValue::Bytes(value)) => line_end = utf8(value),
         _ => {},
      }
      Ok(())
   })?;
   Ok(FreeformConnectorEndpoint {
      item_id,
      magnet,
      normalized_position,
      point: point_value,
      line_end,
   })
}

fn command_point(bytes: &[u8]) -> Result<Option<FreeformPoint>, ()> {
   point(bytes)
}

fn path_command(field: u64, bytes: &[u8]) -> Result<Option<FreeformPathCommand>, ()> {
   let mut points = Vec::new();
   walk_message(bytes, &mut |field, value| {
      if field == 1 {
         if let WireValue::Bytes(value) = value {
            if let Some(point) = command_point(value)? {
               points.push(point);
            }
         }
      }
      Ok(())
   })?;
   Ok(match (field, points.as_slice()) {
      (1, [point]) => Some(FreeformPathCommand::Move { point: *point }),
      (2, [point]) => Some(FreeformPathCommand::Line { point: *point }),
      (3, [control, point]) => {
         Some(FreeformPathCommand::Quadratic { control: *control, point: *point })
      },
      (4, [control_1, control_2, point]) => Some(FreeformPathCommand::Cubic {
         control_1: *control_1,
         control_2: *control_2,
         point:     *point,
      }),
      _ => None,
   })
}

fn path(bytes: &[u8]) -> Result<Option<FreeformPath>, ()> {
   let mut commands = Vec::new();
   walk_message(bytes, &mut |field, value| {
      match (field, value) {
         (1..=4, WireValue::Bytes(value)) => {
            if let Some(command) = path_command(field, value)? {
               commands.push(command);
            }
         },
         (5, WireValue::Varint(1)) => commands.push(FreeformPathCommand::Close),
         _ => {},
      }
      Ok(())
   })?;
   Ok((!commands.is_empty()).then(|| FreeformPath {
      commands,
      natural_size: None,
      raw_data: bytes.to_vec(),
   }))
}
fn transform(bytes: &[u8]) -> Result<Option<FreeformTransform>, ()> {
   let mut components = [None; 6];
   walk_message(bytes, &mut |field, value| {
      if let 1..=6 = field {
         components[(field - 1) as usize] = scalar(value);
      }
      Ok(())
   })?;
   let [Some(a), Some(b), Some(c), Some(d), Some(tx), Some(ty)] = components else {
      return Ok(None);
   };
   if [a, b, c, d, tx, ty].into_iter().all(f64::is_finite) {
      Ok(Some(FreeformTransform { a, b, c, d, tx, ty }))
   } else {
      Ok(None)
   }
}

/// Decodes one bounded `CRLConnectionLineItem` record.
///
/// The connector's field layout is record-local: tail endpoint (field 1), head
/// endpoint (field 2), routing string (field 3), explicit path (field 4), and
/// explicit horizontal/vertical flips (fields 5/6). Endpoint values retain an
/// attached target UUID, magnet and normalized position, or a free point, as
/// applicable. Values that cannot be proved from their wire shape remain absent
/// rather than receiving a synthetic default.
/// Decoded connector endpoints and attributes: (tail, head, routing string,
/// path, horizontal flip, vertical flip).
pub type DecodedConnector = (
   FreeformConnectorEndpoint,
   FreeformConnectorEndpoint,
   Option<String>,
   Option<FreeformPath>,
   Option<bool>,
   Option<bool>,
);

pub fn decode_connector(
   record: &NativeRecord<'_>,
) -> Result<DecodedConnector, FreeformDecodeError> {
   let mut tail = None;
   let mut head = None;
   let mut routing = None;
   let mut connector_path = None;
   let mut horizontal_flip = None;
   let mut vertical_flip = None;
   walk_message(record.bytes, &mut |field, value| {
      match (field, value) {
         (1, WireValue::Bytes(value)) => tail = Some(endpoint(value)?),
         (2, WireValue::Bytes(value)) => head = Some(endpoint(value)?),
         (3, WireValue::Bytes(value)) => routing = utf8(value),
         (4, WireValue::Bytes(value)) => connector_path = path(value)?,
         (5, WireValue::Varint(value @ 0..=1)) => horizontal_flip = Some(value != 0),
         (6, WireValue::Varint(value @ 0..=1)) => vertical_flip = Some(value != 0),
         _ => {},
      }
      Ok(())
   })
   .map_err(|()| invalid_record(record, "connector"))?;
   let empty = || FreeformConnectorEndpoint {
      item_id:             None,
      magnet:              None,
      normalized_position: None,
      point:               None,
      line_end:            None,
   };
   Ok((
      tail.unwrap_or_else(empty),
      head.unwrap_or_else(empty),
      routing,
      connector_path,
      horizontal_flip,
      vertical_flip,
   ))
}

/// Decodes ordered child UUID references and a counter-transform from one
/// bounded `CRLGroupItem` record.
///
/// Child references are read only from repeated field 1 entries in their source
/// order. A complete finite affine counter-transform is retained from field 2;
/// an incomplete transform remains absent. The integrator must verify each
/// returned UUID against the typed archive graph before exposing the group;
/// this record-local decoder has no access to sibling records.
pub fn decode_group(
   record: &NativeRecord<'_>,
) -> Result<(Vec<String>, Option<FreeformTransform>), FreeformDecodeError> {
   let mut children = Vec::new();
   let mut counter_transform = None;
   walk_message(record.bytes, &mut |field, value| {
      match (field, value) {
         (1, value) => {
            if let Some(id) = uuid_from_value(value) {
               children.push(id);
            }
         },
         (2, WireValue::Bytes(value)) => counter_transform = transform(value)?,
         _ => {},
      }
      Ok(())
   })
   .map_err(|()| invalid_record(record, "group"))?;
   Ok((children, counter_transform))
}

#[cfg(test)]
mod tests {
   use super::*;

   fn bytes_field(field: u8, value: &[u8]) -> Vec<u8> {
      assert!(field < 16 && value.len() < 128);
      let mut encoded = vec![(field << 3) | 2, value.len() as u8];
      encoded.extend_from_slice(value);
      encoded
   }

   fn varint_field(field: u8, value: u8) -> Vec<u8> {
      assert!(field < 16 && value < 128);
      vec![field << 3, value]
   }

   fn f32_field(field: u8, value: f32) -> Vec<u8> {
      assert!(field < 16);
      let mut encoded = vec![(field << 3) | 5];
      encoded.extend_from_slice(&value.to_le_bytes());
      encoded
   }

   fn join(parts: &[Vec<u8>]) -> Vec<u8> {
      parts.iter().flatten().copied().collect()
   }

   fn point(x: f32, y: f32) -> Vec<u8> {
      join(&[f32_field(1, x), f32_field(2, y)])
   }

   fn uuid(seed: u8) -> [u8; 16] {
      [seed; 16]
   }

   fn record(bytes: &[u8]) -> NativeRecord<'_> {
      NativeRecord { owner_id: "00000000-0000-0000-0000-000000000000".into(), offset: 11, bytes }
   }

   fn command(field: u8, points: &[Vec<u8>]) -> Vec<u8> {
      let payload = join(
         &points
            .iter()
            .map(|point| bytes_field(1, point))
            .collect::<Vec<_>>(),
      );
      bytes_field(field, &payload)
   }

   #[test]
   fn connector_preserves_attached_and_detached_ends_routing_path_and_flips() {
      let tail = join(&[
         bytes_field(1, &uuid(1)),
         bytes_field(2, b"right"),
         bytes_field(3, &point(0.75, 0.5)),
         bytes_field(5, b"circle"),
      ]);
      let head = join(&[bytes_field(4, &point(33.0, 44.0)), bytes_field(5, b"arrow")]);
      let path = join(&[
         command(1, &[point(1.0, 2.0)]),
         command(2, &[point(3.0, 4.0)]),
         command(3, &[point(5.0, 6.0), point(7.0, 8.0)]),
         command(4, &[point(9.0, 10.0), point(11.0, 12.0), point(13.0, 14.0)]),
         varint_field(5, 1),
      ]);
      let raw = join(&[
         bytes_field(1, &tail),
         bytes_field(2, &head),
         bytes_field(3, b"orthogonal"),
         bytes_field(4, &path),
         varint_field(5, 1),
         varint_field(6, 0),
      ]);

      let (tail, head, routing, decoded_path, horizontal_flip, vertical_flip) =
         decode_connector(&record(&raw)).unwrap();
      assert_eq!(tail.item_id, Some("01010101-0101-0101-0101-010101010101".into()));
      assert_eq!(tail.magnet.as_deref(), Some("right"));
      assert_eq!(tail.normalized_position, Some(FreeformPoint { x: 0.75, y: 0.5 }));
      assert_eq!(tail.point, None);
      assert_eq!(tail.line_end.as_deref(), Some("circle"));
      assert_eq!(head.item_id, None);
      assert_eq!(head.point, Some(FreeformPoint { x: 33.0, y: 44.0 }));
      assert_eq!(head.line_end.as_deref(), Some("arrow"));
      assert_eq!(routing.as_deref(), Some("orthogonal"));
      assert_eq!((horizontal_flip, vertical_flip), (Some(true), Some(false)));
      assert_eq!(decoded_path.unwrap().commands, vec![
         FreeformPathCommand::Move { point: FreeformPoint { x: 1.0, y: 2.0 } },
         FreeformPathCommand::Line { point: FreeformPoint { x: 3.0, y: 4.0 } },
         FreeformPathCommand::Quadratic {
            control: FreeformPoint { x: 5.0, y: 6.0 },
            point:   FreeformPoint { x: 7.0, y: 8.0 },
         },
         FreeformPathCommand::Cubic {
            control_1: FreeformPoint { x: 9.0, y: 10.0 },
            control_2: FreeformPoint { x: 11.0, y: 12.0 },
            point:     FreeformPoint { x: 13.0, y: 14.0 },
         },
         FreeformPathCommand::Close,
      ]);
   }

   #[test]
   fn connector_retains_each_supported_routing_name() {
      for routing in [b"straight".as_slice(), b"corner", b"curved", b"orthogonal"] {
         let raw = bytes_field(3, routing);
         assert_eq!(
            decode_connector(&record(&raw)).unwrap().2.as_deref(),
            Some(std::str::from_utf8(routing).unwrap())
         );
      }
   }

   #[test]
   fn group_preserves_child_order_and_complete_counter_transform() {
      let transform = join(&[
         f32_field(1, 2.0),
         f32_field(2, 0.0),
         f32_field(3, 0.0),
         f32_field(4, 3.0),
         f32_field(5, 20.0),
         f32_field(6, -10.0),
      ]);
      let raw =
         join(&[bytes_field(1, &uuid(3)), bytes_field(1, &uuid(2)), bytes_field(2, &transform)]);
      let (children, transform) = decode_group(&record(&raw)).unwrap();
      assert_eq!(children, vec![
         "03030303-0303-0303-0303-030303030303".to_owned(),
         "02020202-0202-0202-0202-020202020202".to_owned(),
      ]);
      assert_eq!(
         transform,
         Some(FreeformTransform { a: 2.0, b: 0.0, c: 0.0, d: 3.0, tx: 20.0, ty: -10.0 })
      );
   }

   #[test]
   fn nested_groups_retain_each_local_child_order() {
      let inner = join(&[bytes_field(1, &uuid(7)), bytes_field(1, &uuid(8))]);
      let outer = join(&[bytes_field(1, &uuid(9)), bytes_field(1, &uuid(6))]);
      assert_eq!(decode_group(&record(&inner)).unwrap().0, vec![
         "07070707-0707-0707-0707-070707070707".to_owned(),
         "08080808-0808-0808-0808-080808080808".to_owned(),
      ]);
      assert_eq!(decode_group(&record(&outer)).unwrap().0, vec![
         "09090909-0909-0909-0909-090909090909".to_owned(),
         "06060606-0606-0606-0606-060606060606".to_owned(),
      ]);
   }

   #[test]
   fn incomplete_counter_transform_stays_absent() {
      let raw = bytes_field(2, &join(&[f32_field(1, 2.0), f32_field(4, 3.0)]));
      assert_eq!(decode_group(&record(&raw)).unwrap().1, None);
   }
}
