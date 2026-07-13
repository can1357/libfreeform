//! Record-bounded decoders for TSWP text and CRL table payloads.
//!
//! The native archive uses protobuf-shaped messages, but its field schema is
//! versioned.  This module therefore only assigns a value when its wire field
//! and representation prove the mapping; unrecognised nested data stays in the
//! owning `NativeRecord` bytes rather than becoming a guessed public value.

use super::envelope::NativeRecord;
use crate::types::{
   FreeformColor, FreeformDecodeError, FreeformFrame, FreeformPaint, FreeformStyle,
   FreeformTableCell, FreeformText, FreeformTextRun,
};

#[derive(Clone, Copy)]
enum Wire<'a> {
   Varint(u64),
   Fixed32(u32),
   Fixed64(u64),
   Bytes(&'a [u8]),
}
type ParsedFields<'a> = Vec<(u32, Wire<'a>)>;

/// Parses one bounded protobuf-shaped message without reading beyond `data`.
fn fields(data: &[u8]) -> Result<ParsedFields<'_>, FreeformDecodeError> {
   let mut out = Vec::new();
   let mut at = 0;
   while at < data.len() {
      let key = varint(data, &mut at)?;
      let field = (key >> 3) as u32;
      if field == 0 {
         return Err(FreeformDecodeError::invalid("CRL record contains field zero"));
      }
      let value = match key & 7 {
         0 => Wire::Varint(varint(data, &mut at)?),
         1 => {
            let b = take(data, &mut at, 8)?;
            Wire::Fixed64(u64::from_le_bytes(b.try_into().expect("fixed-width slice")))
         },
         2 => {
            let len = usize::try_from(varint(data, &mut at)?)
               .map_err(|_| FreeformDecodeError::invalid("CRL length exceeds address space"))?;
            Wire::Bytes(take(data, &mut at, len)?)
         },
         5 => {
            let b = take(data, &mut at, 4)?;
            Wire::Fixed32(u32::from_le_bytes(b.try_into().expect("fixed-width slice")))
         },
         wire => {
            return Err(FreeformDecodeError::invalid(format!(
               "CRL record has unsupported wire type {wire}"
            )));
         },
      };
      out.push((field, value));
   }
   Ok(out)
}

fn take<'a>(data: &'a [u8], at: &mut usize, len: usize) -> Result<&'a [u8], FreeformDecodeError> {
   let end = at
      .checked_add(len)
      .ok_or_else(|| FreeformDecodeError::incomplete("CRL length overflow"))?;
   let value = data
      .get(*at..end)
      .ok_or_else(|| FreeformDecodeError::incomplete("truncated CRL record"))?;
   *at = end;
   Ok(value)
}

fn varint(data: &[u8], at: &mut usize) -> Result<u64, FreeformDecodeError> {
   let mut value = 0u64;
   for shift in (0..64).step_by(7) {
      let byte = *data
         .get(*at)
         .ok_or_else(|| FreeformDecodeError::incomplete("truncated CRL varint"))?;
      *at += 1;
      let payload = u64::from(byte & 0x7f);
      if shift == 63 && payload > 1 {
         return Err(FreeformDecodeError::invalid("CRL varint exceeds 64 bits"));
      }
      value |= payload << shift;
      if byte & 0x80 == 0 {
         return Ok(value);
      }
   }
   Err(FreeformDecodeError::invalid("CRL varint exceeds 64 bits"))
}

fn string(value: Wire<'_>) -> Option<String> {
   match value {
      Wire::Bytes(bytes) => std::str::from_utf8(bytes).ok().map(str::to_owned),
      _ => None,
   }
}

fn number(value: Wire<'_>) -> Option<f64> {
   match value {
      Wire::Varint(value) => Some(value as f64),
      Wire::Fixed32(value) => Some(f64::from(f32::from_bits(value))),
      Wire::Fixed64(value) => Some(f64::from_bits(value)),
      Wire::Bytes(_) => None,
   }
}

fn dimensions(value: Wire<'_>) -> Vec<f64> {
   match value {
      Wire::Bytes(bytes) if bytes.len() % 4 == 0 => bytes
         .chunks_exact(4)
         .map(|bytes| f64::from(f32::from_le_bytes(bytes.try_into().expect("fixed-width chunk"))))
         .filter(|value| value.is_finite())
         .collect(),
      value => number(value)
         .filter(|value| value.is_finite())
         .into_iter()
         .collect(),
   }
}

const fn boolean(value: Wire<'_>) -> Option<bool> {
   match value {
      Wire::Varint(0) => Some(false),
      Wire::Varint(1) => Some(true),
      _ => None,
   }
}

fn index(value: Wire<'_>) -> Option<usize> {
   match value {
      Wire::Varint(value) => usize::try_from(value).ok(),
      _ => None,
   }
}

fn color(data: &[u8]) -> Option<FreeformPaint> {
   let fields = fields(data).ok()?;
   let mut channels = [None; 4];
   for (field, value) in fields {
      if let 1..=4 = field {
         let value = number(value)?;
         if !(0.0..=1.0).contains(&value) || !value.is_finite() {
            return None;
         }
         channels[(field - 1) as usize] = Some(value);
      }
   }
   let [Some(red), Some(green), Some(blue), Some(alpha)] = channels else {
      return None;
   };
   let byte = |value: f64| (value * 255.0).round() as u8;
   Some(FreeformPaint::Solid {
      color: FreeformColor {
         color_space: "sRGB".to_owned(),
         red,
         green,
         blue,
         alpha,
         hex: format!("#{:02x}{:02x}{:02x}", byte(red), byte(green), byte(blue)),
      },
   })
}

fn frame(data: &[u8]) -> Option<FreeformFrame> {
   let values: Vec<f64> = fields(data)
      .ok()?
      .into_iter()
      .filter_map(|(_, value)| number(value))
      .collect();
   (values.len() == 5).then(|| FreeformFrame {
      x:        values[0],
      y:        values[1],
      w:        values[2],
      h:        values[3],
      rotation: values[4],
   })
}

fn text_run(data: &[u8]) -> Option<FreeformTextRun> {
   let mut run = FreeformTextRun::default();
   for (field, value) in fields(data).ok()? {
      match field {
         1 => run.start = index(value)?,
         2 => run.end = index(value)?,
         3 => run.font_name = string(value),
         4 => run.font_size = number(value),
         5 => run.bold = boolean(value),
         6 => run.italic = boolean(value),
         7 => run.underline = string(value),
         8 => run.strikethrough = string(value),
         9 => {
            if let Wire::Bytes(data) = value {
               run.fill = color(data);
            }
         },
         10 => run.paragraph_alignment = string(value),
         11 => run.writing_direction = string(value),
         12 => run.hyperlink = string(value),
         13 => run.list_style = string(value),
         _ => {},
      }
   }
   (run.end >= run.start).then_some(run)
}

/// Decodes a TSWP text record without scanning outside that record.
///
/// Field 1 is the verified UTF-8 content field.  Empty, one-character,
/// multiline, control-containing, long, and identifier-looking content are
/// retained verbatim.  Runs are emitted only when their byte bounds are valid.
pub fn decode_text(record: &NativeRecord<'_>) -> Result<FreeformText, FreeformDecodeError> {
   let root = fields(record.bytes)?;
   let plain = root
      .iter()
      .find_map(|(field, value)| (*field == 1).then(|| string(*value)).flatten())
      .ok_or_else(|| FreeformDecodeError::invalid("TSWP record has no UTF-8 text field"))?;
   let mut text = FreeformText { plain, ..FreeformText::default() };
   for (field, value) in root {
      match (field, value) {
         (2, Wire::Bytes(data)) => {
            if let Some(run) = text_run(data)
               && run.start <= text.plain.len()
               && run.end <= text.plain.len()
               && text.plain.is_char_boundary(run.start)
               && text.plain.is_char_boundary(run.end)
            {
               text.runs.push(run);
            }
         },
         (3, Wire::Bytes(data)) => text.inset = frame(data),
         (4, value) => text.vertical_alignment = string(value),
         (5, value) => text.shrink_to_fit = boolean(value),
         _ => {},
      }
   }
   Ok(text)
}

fn cell(data: &[u8]) -> Option<FreeformTableCell> {
   let root = fields(data).ok()?;
   let id = root
      .iter()
      .find_map(|(field, value)| (*field == 1).then(|| string(*value)).flatten());
   let mut cell = FreeformTableCell { id, ..FreeformTableCell::default() };
   for (field, value) in root {
      match (field, value) {
         (2, value) => cell.row = Some(index(value)?),
         (3, value) => cell.column = Some(index(value)?),
         (4, value) => {
            cell.row_span = Some(index(value)?);
         },
         (5, value) => {
            cell.column_span = Some(index(value)?);
         },
         (6, Wire::Bytes(data)) => cell.text = decode_text_bytes(data).ok(),
         (7, Wire::Bytes(data)) => cell.style = style(data),
         (8, value) => {
            if let Some(id) = string(value) {
               cell.anchored_item_ids.push(id);
            }
         },
         _ => {},
      }
   }
   (cell.row_span.is_some_and(|span| span > 0) && cell.column_span.is_some_and(|span| span > 0))
      .then_some(cell)
}

fn decode_text_bytes(bytes: &[u8]) -> Result<FreeformText, FreeformDecodeError> {
   decode_text(&NativeRecord { owner_id: String::new(), offset: 0, bytes })
}

fn style(data: &[u8]) -> FreeformStyle {
   let mut style = FreeformStyle::default();
   if let Ok(values) = fields(data) {
      for (field, value) in values {
         match (field, value) {
            (1, Wire::Bytes(data)) => style.fill = color(data),
            (2, Wire::Bytes(data)) => style.stroke = stroke(data),
            _ => {},
         }
      }
   }
   style
}

fn stroke(data: &[u8]) -> Option<crate::types::FreeformStrokeStyle> {
   let fields = fields(data).ok()?;
   let paint = fields.iter().find_map(|(field, value)| {
      (*field == 1)
         .then(|| match value {
            Wire::Bytes(data) => color(data),
            _ => None,
         })
         .flatten()
   })?;
   let width = fields
      .iter()
      .find_map(|(field, value)| (*field == 2).then(|| number(*value)).flatten())?;
   Some(crate::types::FreeformStrokeStyle {
      paint,
      width,
      dash: Vec::new(),
      dash_offset: None,
      cap: None,
      join: None,
      miter_limit: None,
      tail_end: None,
      head_end: None,
   })
}

/// Decoded table dimensions and cells: (rows, columns, cells).
pub type DecodedTable = (Vec<f64>, Vec<f64>, Vec<FreeformTableCell>);

/// Decodes a CRL table record as row heights, column widths, and bounded cells.
///
/// Dimensions are preserved in archive order.  Cells carry their explicit
/// positions, non-zero spans, text (including embedded newlines), visual
/// style, and explicit anchored child UUIDs.
pub fn decode_table(record: &NativeRecord<'_>) -> Result<DecodedTable, FreeformDecodeError> {
   let root = fields(record.bytes)?;
   let mut columns = Vec::new();
   let mut rows = Vec::new();
   let mut cells = Vec::new();
   for (field, value) in root {
      match (field, value) {
         (1, value) => columns.extend(dimensions(value)),
         (2, value) => rows.extend(dimensions(value)),
         (3, Wire::Bytes(data)) => {
            if let Some(cell) = cell(data) {
               cells.push(cell);
            }
         },
         _ => {},
      }
   }
   if columns.is_empty() && rows.is_empty() && cells.is_empty() {
      return Err(FreeformDecodeError::invalid(
         "CRL table record has no proven dimensions or cells",
      ));
   }
   Ok((rows, columns, cells))
}

#[cfg(test)]
mod tests {
   use super::*;

   fn bytes(field: u8, value: &[u8]) -> Vec<u8> {
      let mut out = vec![field << 3 | 2, value.len() as u8];
      out.extend_from_slice(value);
      out
   }

   fn integer(field: u8, value: u8) -> Vec<u8> {
      vec![field << 3, value]
   }

   fn float(field: u8, value: f32) -> Vec<u8> {
      let mut out = vec![field << 3 | 5];
      out.extend_from_slice(&value.to_le_bytes());
      out
   }

   fn record(bytes: &[u8]) -> NativeRecord<'_> {
      NativeRecord { owner_id: String::new(), offset: 0, bytes }
   }

   fn color(red: f32, green: f32, blue: f32, alpha: f32) -> Vec<u8> {
      [float(1, red), float(2, green), float(3, blue), float(4, alpha)].concat()
   }

   #[test]
   fn retains_all_previously_filtered_user_text() {
      for plain in [
         "a",
         "first\nsecond\twith control",
         "c6f3c7a1-bb39-4fa9-a4db-8d0f94a53b6d",
         "Parallelogram_950",
         "thumbnail.png",
         "com.apple.user.note",
         "a very long line that remains user text because this record is structurally a TSWP \
          record",
      ] {
         let encoded = bytes(1, plain.as_bytes());
         assert_eq!(decode_text(&record(&encoded)).unwrap().plain, plain);
      }
   }

   #[test]
   fn decodes_styled_ranges_and_container_properties() {
      let fill = color(0.25, 0.5, 0.75, 1.0);
      let run = [
         integer(1, 0),
         integer(2, 5),
         bytes(3, b"Helvetica-Bold"),
         float(4, 13.5),
         integer(5, 1),
         integer(6, 0),
         bytes(7, b"single"),
         bytes(8, b"double"),
         bytes(9, &fill),
         bytes(10, b"center"),
         bytes(11, b"rtl"),
         bytes(12, b"https://example.test"),
         bytes(13, b"bullet"),
      ]
      .concat();
      let inset =
         [float(1, 1.0), float(2, 2.0), float(3, 3.0), float(4, 4.0), float(5, 0.0)].concat();
      let encoded =
         [bytes(1, b"hello"), bytes(2, &run), bytes(3, &inset), bytes(4, b"middle"), integer(5, 1)]
            .concat();
      let text = decode_text(&record(&encoded)).unwrap();
      assert_eq!(text.inset.unwrap().w, 3.0);
      assert_eq!(text.vertical_alignment.as_deref(), Some("middle"));
      assert_eq!(text.shrink_to_fit, Some(true));
      assert_eq!(text.runs.len(), 1);
      let run = &text.runs[0];
      assert_eq!((run.start, run.end), (0, 5));
      assert_eq!(run.font_name.as_deref(), Some("Helvetica-Bold"));
      assert_eq!(run.font_size, Some(13.5));
      assert_eq!(run.bold, Some(true));
      assert_eq!(run.italic, Some(false));
      assert_eq!(run.underline.as_deref(), Some("single"));
      assert_eq!(run.strikethrough.as_deref(), Some("double"));
      assert_eq!(run.paragraph_alignment.as_deref(), Some("center"));
      assert_eq!(run.writing_direction.as_deref(), Some("rtl"));
      assert_eq!(run.hyperlink.as_deref(), Some("https://example.test"));
      assert_eq!(run.list_style.as_deref(), Some("bullet"));
      assert!(matches!(run.fill, Some(FreeformPaint::Solid { .. })));
   }

   #[test]
   fn preserves_table_cell_boundaries_dimensions_and_style() {
      let text = bytes(1, b"one\ntwo");
      let fill = color(1.0, 0.0, 0.0, 1.0);
      let border = [bytes(1, &color(0.0, 0.0, 1.0, 1.0)), float(2, 2.0)].concat();
      let style = [bytes(1, &fill), bytes(2, &border)].concat();
      let first = [
         bytes(1, b"cell-a"),
         integer(2, 0),
         integer(3, 0),
         integer(4, 1),
         integer(5, 2),
         bytes(6, &text),
         bytes(7, &style),
         bytes(8, b"shape-uuid"),
      ]
      .concat();
      let second = [
         bytes(1, b"cell-b"),
         integer(2, 0),
         integer(3, 2),
         integer(4, 1),
         integer(5, 1),
         bytes(6, &bytes(1, b"three")),
      ]
      .concat();
      let table =
         [float(1, 72.0), float(1, 144.0), float(2, 28.0), bytes(3, &first), bytes(3, &second)]
            .concat();
      let (rows, columns, cells) = decode_table(&record(&table)).unwrap();
      assert_eq!(columns, [72.0, 144.0]);
      assert_eq!(rows, [28.0]);
      assert_eq!(cells[0].text.as_ref().unwrap().plain, "one\ntwo");
      assert_eq!(cells[1].text.as_ref().unwrap().plain, "three");
      assert_eq!(
         (cells[0].row, cells[0].column, cells[0].row_span, cells[0].column_span),
         (Some(0), Some(0), Some(1), Some(2))
      );
      assert_eq!(cells[0].anchored_item_ids, ["shape-uuid"]);
      assert!(matches!(cells[0].style.fill, Some(FreeformPaint::Solid { .. })));
      assert!(cells[0].style.stroke.is_some());
   }
}
