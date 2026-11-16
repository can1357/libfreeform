//! Freeform's native text storage and table capsule schemas.
//!
//! Text is `CRLWPStorageCRDTData.a`; `.b` is instructional text, not content.
//! `Proto_Sequence.attributeRuns` contains consecutive UTF-16 lengths and named
//! `CRValues`. Table rows/columns are ordered UUID dictionaries; cells are
//! keyed by the native `(rowID, columnID)` value, not by their reference-record
//! order.

use std::collections::BTreeMap;

use super::{
   coherence::{Document, Node, Value},
   invalid, shape,
};
use crate::types::{
   FreeformDecodeError, FreeformStyle, FreeformTableCell, FreeformTableOptions, FreeformText,
   FreeformTextRun,
};

type Result<T, E = FreeformDecodeError> = std::result::Result<T, E>;
pub(super) struct DecodedTable {
   pub row_heights:   Vec<f64>,
   pub column_widths: Vec<f64>,
   pub cells:         Vec<FreeformTableCell>,
   pub style:         FreeformStyle,
   pub options:       FreeformTableOptions,
}

fn number(value: Option<Value<'_>>) -> Result<Option<f64>> {
   value.map(Value::number).transpose().map(Option::flatten)
}

fn integer(value: Option<Value<'_>>) -> Result<Option<i64>> {
   value.map(Value::integer).transpose().map(Option::flatten)
}

fn boolean(value: Option<Value<'_>>) -> Result<Option<bool>> {
   value.map(Value::boolean).transpose().map(Option::flatten)
}

fn string(value: Option<Value<'_>>) -> Result<Option<String>> {
   Ok(value
      .map(Value::string)
      .transpose()?
      .flatten()
      .map(str::to_owned))
}

fn coding_field(value: Value<'_>, key: u64) -> Result<Option<Value<'_>>> {
   value
      .field(key)?
      .map(Value::optional)
      .transpose()
      .map(Option::flatten)
}

fn named_enum(
   value: Option<Value<'_>>,
   names: &[(i64, &str)],
   kind: &str,
) -> Result<Option<String>> {
   integer(value)?
      .map(|value| {
         names
            .iter()
            .find(|(number, _)| *number == value)
            .map(|(_, name)| (*name).to_owned())
            .ok_or_else(|| {
               FreeformDecodeError::unsupported(format!("native text: {kind} value {value}"))
            })
      })
      .transpose()
}

fn alignment(value: Option<Value<'_>>) -> Result<Option<String>> {
   named_enum(
      value,
      &[(0, "left"), (1, "right"), (2, "center"), (3, "justified"), (4, "natural")],
      "paragraph alignment",
   )
}

fn direction(value: Option<Value<'_>>) -> Result<Option<String>> {
   named_enum(
      value,
      &[(-1, "natural"), (0, "left-to-right"), (1, "right-to-left")],
      "writing direction",
   )
}

fn vertical(value: Option<Value<'_>>) -> Result<Option<String>> {
   named_enum(
      value,
      &[(0, "top"), (1, "middle"), (2, "bottom"), (3, "justify")],
      "vertical alignment",
   )
}

fn list_style(value: Option<Value<'_>>) -> Result<Option<String>> {
   named_enum(value, &[(0, "none"), (1, "numbered"), (2, "dashed"), (3, "bulleted")], "list style")
}

/// `NSUnderlineStyle` is an option set, shared by underline and strikethrough.
fn decoration(value: Option<Value<'_>>) -> Result<Option<String>> {
   let Some(value) = integer(value)? else {
      return Ok(None);
   };
   let line = match value & 0xff {
      0 => "none",
      1 => "single",
      2 => "thick",
      9 => "double",
      _ => {
         return Err(FreeformDecodeError::unsupported(format!("native text: decoration {value}")));
      },
   };
   let pattern = match value & 0x700 {
      0 => "",
      0x100 => " dot",
      0x200 => " dash",
      0x300 => " dash-dot",
      0x400 => " dash-dot-dot",
      _ => {
         return Err(FreeformDecodeError::unsupported(format!(
            "native text: decoration pattern {value}"
         )));
      },
   };
   if value & !0x87ff != 0 {
      return Err(FreeformDecodeError::unsupported(format!(
         "native text: decoration flags {value}"
      )));
   }
   Ok(Some(format!("{line}{pattern}{}", if value & 0x8000 != 0 { " by-word" } else { "" })))
}

/// `CRLWPTextPresetData`'s integer `CodingKeys`, verified by its native
/// descriptor.
fn apply_preset(run: &mut FreeformTextRun, preset: Option<Value<'_>>) -> Result<()> {
   let Some(preset) = preset else {
      return Ok(());
   };
   macro_rules! apply {
      ($field:ident, $value:expr) => {
         if let Some(value) = $value? {
            run.$field = Some(value);
         }
      };
   }
   apply!(font_name, string(coding_field(preset, 0)?));
   apply!(font_size, number(coding_field(preset, 1)?));
   apply!(bold, boolean(coding_field(preset, 2)?));
   apply!(italic, boolean(coding_field(preset, 3)?));
   apply!(underline, decoration(coding_field(preset, 4)?));
   apply!(strikethrough, decoration(coding_field(preset, 5)?));
   if let Some(value) = coding_field(preset, 6)? {
      apply!(fill, shape::paint(value));
   }
   apply!(paragraph_alignment, alignment(coding_field(preset, 7)?));
   apply!(writing_direction, direction(coding_field(preset, 8)?));
   apply!(list_style, list_style(coding_field(preset, 9)?));
   apply!(list_start, integer(coding_field(preset, 10)?));
   Ok(())
}

fn apply_attribute(run: &mut FreeformTextRun, name: &str, value: Value<'_>) -> Result<()> {
   let Some(value) = value.optional()? else {
      return Ok(());
   };
   match name {
      "fontName" => run.font_name = string(Some(value))?,
      "fontSize" => run.font_size = number(Some(value))?,
      "bold" => run.bold = boolean(Some(value))?,
      "italic" => run.italic = boolean(Some(value))?,
      "underline" => run.underline = decoration(Some(value))?,
      "strikethrough" => run.strikethrough = decoration(Some(value))?,
      "characterFill" => run.fill = shape::paint(value)?,
      "paragraphAlignment" => run.paragraph_alignment = alignment(Some(value))?,
      "baseWritingDirection" => run.writing_direction = direction(Some(value))?,
      "listStyle" => run.list_style = list_style(Some(value))?,
      "listStart" => run.list_start = integer(Some(value))?,
      "paragraphLevel" => run.paragraph_level = integer(Some(value))?,
      // CRLWPHyperlinkSmartFieldData: 0 type, 1 optional URL, 2 UUID.
      "hyperlink" => {
         run.hyperlink = coding_field(value, 1)?
            .map(Value::url)
            .transpose()?
            .flatten();
      },
      _ => {},
   }
   Ok(())
}

/// Converts monotonically increasing UTF-16 boundaries without a second string
/// allocation or rescanning earlier runs. A boundary inside a surrogate pair is
/// malformed; rounding would silently assign attributes to the wrong character.
struct Utf16Cursor<'a> {
   remaining: &'a str,
   byte:      usize,
}

impl<'a> Utf16Cursor<'a> {
   const fn new(text: &'a str) -> Self {
      Self { remaining: text, byte: 0 }
   }

   fn advance(&mut self, mut length: u64) -> Result<usize> {
      while length != 0 {
         let character = self
            .remaining
            .chars()
            .next()
            .ok_or_else(|| invalid("text attribute range exceeds its string"))?;
         length = length
            .checked_sub(character.len_utf16() as u64)
            .ok_or_else(|| invalid("text attribute range splits a UTF-16 surrogate pair"))?;
         self.byte += character.len_utf8();
         self.remaining = &self.remaining[character.len_utf8()..];
      }
      Ok(self.byte)
   }
}

fn attributed_string(node: Node<'_>, base: &FreeformTextRun) -> Result<Option<FreeformText>> {
   let Some(string) = node.crdt()?.string else {
      return Ok(None);
   };
   let plain = string.text.unwrap_or_default();
   let mut cursor = Utf16Cursor::new(plain);
   let mut runs = Vec::new();
   for attribute_run in string.attribute_runs {
      let attribute_run = attribute_run?;
      let start = cursor.byte;
      let end = cursor.advance(attribute_run.length.unwrap_or(0))?;
      let mut run = base.clone();
      run.start = start;
      run.end = end;
      for attribute in attribute_run.attributes {
         let attribute = attribute?;
         let key = node.context.string(attribute.key.unwrap_or(0))?;
         let value = attribute
            .value
            .ok_or_else(|| invalid("text attribute has no value"))?;
         apply_attribute(&mut run, key, node.context.value(value))?;
      }
      runs.push(run);
   }
   if runs.is_empty() {
      if !plain.is_empty() {
         runs.push(FreeformTextRun { start: 0, end: plain.len(), ..base.clone() });
      }
   } else if !cursor.remaining.is_empty() {
      return Err(invalid("text attribute runs do not cover their string"));
   }
   Ok(Some(FreeformText { plain: plain.to_owned(), runs, ..FreeformText::default() }))
}

fn storage_text(storage: Option<Node<'_>>, base: &FreeformTextRun) -> Result<Option<FreeformText>> {
   let Some(storage) = storage else {
      return Ok(None);
   };
   let Some(text) = storage.field("a")? else {
      return Ok(None);
   };
   attributed_string(text, base)
}

fn shape_text_style(text: &mut FreeformText, style: Option<Node<'_>>) -> Result<()> {
   let Some(style) = style else {
      return Ok(());
   };
   if let Some(value) = number(style.property("a")?)? {
      text.inset = Some(value);
   }
   if let Some(value) = vertical(style.property("b")?)? {
      text.vertical_alignment = Some(value);
   }
   if let Some(value) = boolean(style.property("c")?)? {
      text.shrink_to_fit = Some(value);
   }
   Ok(())
}

/// `WPShape` labels and textboxes share storage; sticky notes wrap that shape
/// in a.
pub(super) fn decode_text(
   specific: Node<'_>,
   persistence_type: u32,
) -> Result<Option<FreeformText>> {
   let shape = match persistence_type {
      3 => specific,
      12 => match specific.field("a")? {
         Some(shape) => shape,
         None => return Ok(None),
      },
      _ => return Ok(None),
   };
   let mut base = FreeformTextRun::default();
   apply_preset(&mut base, shape.property("d")?)?;
   let Some(mut text) = storage_text(shape.field("b")?, &base)? else {
      return Ok(None);
   };
   shape_text_style(&mut text, shape.field("c")?)?;
   Ok(Some(text))
}

fn axis(
   document: Document<'_>,
   node: Option<Node<'_>>,
   default: f64,
) -> Result<(Vec<String>, Vec<f64>)> {
   let Some(node) = node else {
      return Ok((Vec::new(), Vec::new()));
   };
   let entries = node.ordered_dictionary()?;
   let mut ids = Vec::with_capacity(entries.len());
   let mut dimensions = Vec::with_capacity(entries.len());
   for (key, reference) in entries {
      let id = key
         .uuid()?
         .ok_or_else(|| invalid("table axis key is not a UUID"))?;
      let data = document
         .resolve(reference)?
         .ok_or_else(|| invalid("unresolved table axis reference"))?;
      ids.push(id);
      dimensions.push(number(data.property("a")?)?.unwrap_or(default));
   }
   Ok((ids, dimensions))
}

fn table_text_style(text: &mut FreeformText, style: Option<Node<'_>>) -> Result<()> {
   let Some(style) = style else {
      return Ok(());
   };
   text.vertical_alignment = vertical(style.property("a")?)?;
   text.inset = number(style.property("b")?)?;
   Ok(())
}

/// Decodes the capsule root and only its explicitly referenced live records.
/// Missing cell records are genuine empty cells, not missing rows or columns.
/// Default dimensions are native `CRLTableItem` getter defaults (258 / 344 pt).
/// Anchored children are associated by the caller using their common anchor
/// hint.
pub(super) fn decode_table(capsule: Document<'_>) -> Result<DecodedTable> {
   let root = capsule
      .root()?
      .ok_or_else(|| invalid("table capsule has no root"))?;
   let (row_ids, rows) = axis(capsule, root.field("b")?, 258.0)?;
   let (column_ids, columns) = axis(capsule, root.field("c")?, 344.0)?;
   let count = rows
      .len()
      .checked_mul(columns.len())
      .ok_or_else(|| invalid("table dimensions overflow"))?;
   let row_indices: BTreeMap<_, _> = row_ids
      .iter()
      .enumerate()
      .map(|(index, id)| (id.as_str(), index))
      .collect();
   let column_indices: BTreeMap<_, _> = column_ids
      .iter()
      .enumerate()
      .map(|(index, id)| (id.as_str(), index))
      .collect();
   let table_style = root.field("e")?;
   let text_style = match table_style {
      Some(style) => style.field("j")?,
      None => None,
   };
   let mut base = FreeformTextRun::default();
   if let Some(style) = text_style {
      apply_preset(&mut base, style.property("d")?)?;
   }
   let mut style = FreeformStyle::default();
   let mut options = FreeformTableOptions::default();
   if let Some(table_style) = table_style {
      if let Some(fill) = table_style.property("g")? {
         style.fill = shape::paint(fill)?;
      }
      if let Some(stroke) = table_style.property("a")? {
         style.stroke = shape::stroke(stroke)?;
      }
      if let Some(border) = table_style.property("c")? {
         options.border_options = integer(border.field(0)?)?;
      }
      options.rounded_corners = boolean(table_style.property("d")?)?;
      options.cell_autogrow = boolean(table_style.property("e")?)?;
      options.right_to_left = boolean(table_style.property("f")?)?;
   }
   let mut cells = Vec::with_capacity(count);
   for (row, row_id) in row_ids.iter().enumerate() {
      for (column, column_id) in column_ids.iter().enumerate() {
         cells.push(FreeformTableCell {
            id: Some(format!("{row_id}/{column_id}")),
            row: Some(row),
            column: Some(column),
            row_span: Some(1),
            column_span: Some(1),
            style: FreeformStyle { fill: style.fill.clone(), ..FreeformStyle::default() },
            ..FreeformTableCell::default()
         });
      }
   }
   if let Some(dictionary) = root.field("d")? {
      for (key, reference) in dictionary.dictionary()? {
         let row_id = key
            .field(0)?
            .map(Value::uuid)
            .transpose()?
            .flatten()
            .ok_or_else(|| invalid("table cell has no row UUID"))?;
         let column_id = key
            .field(1)?
            .map(Value::uuid)
            .transpose()?
            .flatten()
            .ok_or_else(|| invalid("table cell has no column UUID"))?;
         // Deleting an axis can leave an unreferenced cell in the dictionary.
         let (Some(&row), Some(&column)) =
            (row_indices.get(row_id.as_str()), column_indices.get(column_id.as_str()))
         else {
            continue;
         };
         let data = capsule
            .resolve(reference)?
            .ok_or_else(|| invalid("unresolved table cell reference"))?;
         let cell = &mut cells[row * columns.len() + column];
         if let Some(fill) = data.property("b")? {
            cell.style.fill = shape::paint(fill)?;
         }
         let mut cell_base = base.clone();
         apply_preset(&mut cell_base, data.property("c")?)?;
         if let Some(mut text) = storage_text(data.field("a")?, &cell_base)? {
            table_text_style(&mut text, text_style)?;
            shape_text_style(&mut text, data.field("d")?)?;
            cell.text = Some(text);
         }
      }
   }
   Ok(DecodedTable { row_heights: rows, column_widths: columns, cells, style, options })
}

#[cfg(test)]
mod tests {
   use super::{super::envelope::parse_native_archive, *};

   const REAL: &[u8] = include_bytes!("../../fixtures/real-board.crlnative");

   #[test]
   fn real_sticky_uses_content_not_instructional_text() {
      let archive = parse_native_archive(REAL).unwrap();
      let sticky = archive
         .objects
         .iter()
         .find(|object| object.persistence_type == 12)
         .unwrap();
      let document = Document::parse(sticky.region("specificCRDTData").unwrap().bytes).unwrap();
      let text = decode_text(document.root().unwrap().unwrap(), 12)
         .unwrap()
         .unwrap();
      assert_eq!(text.plain, "hi");
      assert_eq!(text.inset, Some(10.0));
      assert_eq!(text.vertical_alignment.as_deref(), Some("middle"));
      assert_eq!(text.shrink_to_fit, Some(true));
      assert_eq!(text.runs[0].bold, Some(true));
      assert_eq!((text.runs[0].start, text.runs[0].end), (0, 2));
   }

   #[test]
   fn real_shape_instructional_space_is_not_a_label() {
      let archive = parse_native_archive(REAL).unwrap();
      let shape = &archive.objects[0];
      let document = Document::parse(shape.region("specificCRDTData").unwrap().bytes).unwrap();
      assert!(
         decode_text(document.root().unwrap().unwrap(), 3)
            .unwrap()
            .is_none()
      );
   }

   #[test]
   fn real_table_preserves_order_sparse_cells_and_native_attributes() {
      let archive = parse_native_archive(REAL).unwrap();
      let table = archive
         .objects
         .iter()
         .find(|object| object.region("capsuleData").is_some())
         .unwrap();
      let document = Document::parse(table.region("capsuleData").unwrap().bytes).unwrap();
      let DecodedTable { row_heights, column_widths, cells, style, options } =
         decode_table(document).unwrap();
      assert_eq!(row_heights, [353.8125, 258.0]);
      assert_eq!(column_widths, [344.0, 344.0]);
      assert!(style.stroke.is_some());
      assert!(style.fill.is_some());
      assert_eq!(options.border_options, Some(7));
      assert_eq!(options.rounded_corners, Some(true));
      assert_eq!(options.cell_autogrow, Some(true));
      assert_eq!(cells.len(), 4);
      assert_eq!(
         cells[0].id.as_deref(),
         Some("D8173D6F-4E25-458B-93E2-132B146FDEC8/4C041466-EA06-4A6D-A003-01F7CFDA3AD3")
      );
      assert!(cells[3].text.is_none());
      assert_eq!(
         cells
            .iter()
            .filter(|cell| cell.text.as_ref().is_some_and(|text| text.plain == "aaa"))
            .count(),
         3
      );
      let styled = cells
         .iter()
         .filter_map(|cell| cell.text.as_ref())
         .flat_map(|text| &text.runs)
         .find(|run| run.font_size == Some(72.0))
         .unwrap();
      assert_eq!(styled.bold, Some(true));
      assert_eq!(styled.paragraph_alignment.as_deref(), Some("center"));
      assert_eq!(styled.list_style.as_deref(), Some("bulleted"));
      assert!(styled.fill.is_some());
      assert!(
         cells
            .iter()
            .all(|cell| cell.row_span == Some(1) && cell.column_span == Some(1))
      );
   }

   #[test]
   fn utf16_ranges_convert_to_documented_utf8_byte_offsets() {
      let mut cursor = Utf16Cursor::new("a🦀é");
      assert_eq!(cursor.advance(1).unwrap(), 1);
      assert_eq!(cursor.advance(2).unwrap(), 5);
      assert_eq!(cursor.advance(1).unwrap(), 7);
      assert!(cursor.advance(1).is_err());
      assert!(Utf16Cursor::new("🦀").advance(1).is_err());
   }
}
