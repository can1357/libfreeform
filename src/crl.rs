//! CRL native-archive integration.
//!
//! The envelope parser establishes record boundaries. This module never scans
//! the archive globally or associates values by byte distance: item decoders
//! receive only the record that owns the item. A TSU description is decoded and
//! joined separately so an invalid optional manifest cannot discard native
//! data.

mod assets;
mod connector;
mod envelope;
mod native_ink;
mod shape;
mod text_table;

use std::collections::{BTreeMap, BTreeSet};

use envelope::{NativeArchive, NativeRecord, parse_native_archive};

pub use super::types::TsuEntry;
use super::{
   bplist::{Plist, parse_bplist},
   types::{
      FreeformAsset, FreeformBoardItem, FreeformConnectorEndpoint, FreeformDecodeError,
      FreeformGeometry, FreeformItemKind, FreeformNative, FreeformRecordRange, FreeformStyle,
      TsuValue,
   },
};

/// Parses a `TSUDescription` plist into its ordered board-item routing entries.
///
/// TSU is deliberately strict: a description must be a dictionary containing
/// a `boardItems` array, and every entry must have a string `class`. Every hint
/// is converted recursively so nested values are retained without stringifying
/// or silently dropping channels.
pub fn parse_tsu_description(data: &[u8]) -> Result<Vec<TsuEntry>, FreeformDecodeError> {
   let plist = parse_bplist(data)
      .ok_or_else(|| FreeformDecodeError::invalid("TSUDescription: invalid binary plist"))?;
   let root = plist
      .as_dict()
      .ok_or_else(|| FreeformDecodeError::invalid("TSUDescription: root is not a dictionary"))?;
   let board_items = root
      .get("boardItems")
      .and_then(Plist::as_array)
      .ok_or_else(|| FreeformDecodeError::invalid("TSUDescription: missing boardItems array"))?;

   board_items
      .iter()
      .enumerate()
      .map(|(index, item)| parse_tsu_entry(index, item))
      .collect()
}

fn parse_tsu_entry(index: usize, item: &Plist) -> Result<TsuEntry, FreeformDecodeError> {
   let item = item.as_dict().ok_or_else(|| {
      FreeformDecodeError::invalid(format!(
         "TSUDescription: boardItems[{index}] is not a dictionary"
      ))
   })?;
   let class_name = item
      .get("class")
      .and_then(Plist::as_str)
      .filter(|class_name| !class_name.is_empty())
      .ok_or_else(|| {
         FreeformDecodeError::invalid(format!("TSUDescription: boardItems[{index}] has no class"))
      })?;
   let hints = item
      .iter()
      .filter(|(key, _)| key.as_str() != "class")
      .map(|(key, value)| Ok((key.clone(), tsu_value(value)?)))
      .collect::<Result<BTreeMap<_, _>, FreeformDecodeError>>()?;

   Ok(TsuEntry {
      class_name: class_name
         .strip_prefix("Freeform.")
         .unwrap_or(class_name)
         .to_owned(),
      hints,
   })
}

fn tsu_value(value: &Plist) -> Result<TsuValue, FreeformDecodeError> {
   match value {
      Plist::Null => Ok(TsuValue::Null),
      Plist::Bool(value) => Ok(TsuValue::Bool(*value)),
      Plist::Int(value) => Ok(TsuValue::Integer(*value)),
      Plist::Real(value) if value.is_finite() => Ok(TsuValue::Real(*value)),
      Plist::Real(_) => Err(FreeformDecodeError::invalid("TSUDescription: non-finite real")),
      Plist::String(value) => Ok(TsuValue::String(value.clone())),
      Plist::Data(value) => Ok(TsuValue::Data(value.clone())),
      Plist::Array(values) => values
         .iter()
         .map(tsu_value)
         .collect::<Result<Vec<_>, _>>()
         .map(TsuValue::Array),
      Plist::Dict(values) => values
         .iter()
         .map(|(key, value)| Ok((key.clone(), tsu_value(value)?)))
         .collect::<Result<BTreeMap<_, _>, FreeformDecodeError>>()
         .map(TsuValue::Dictionary),
      Plist::Uid(_) | Plist::OrderedSet(_) | Plist::Set(_) => {
         Err(FreeformDecodeError::invalid("TSUDescription: value cannot be represented losslessly"))
      },
   }
}

/// Decodes the self-contained `CRLNativeData` tier without requiring TSU.
///
/// Returned items retain every exact bounded owner-record range in
/// [`FreeformNative::raw_archive`]. `raw_data` remains the primary common
/// record for schema-bounded dispatch. Without a TSU routing manifest items
/// are deliberately `Unknown`; use [`join_crl_tsu`] only after the independent
/// TSU tier has decoded successfully.
pub fn decode_crl_native(data: &[u8]) -> Result<FreeformNative, FreeformDecodeError> {
   let archive = parse_native_archive(data)?;
   Ok(native_from_archive(&archive))
}

fn native_from_archive(archive: &NativeArchive<'_>) -> FreeformNative {
   let items = archive
      .item_ids
      .iter()
      .enumerate()
      .map(|(index, uuid)| board_item_from_records(index, uuid, archive.records(uuid).collect()))
      .collect();

   FreeformNative {
      paste_id: archive.paste_id.clone(),
      compatibility: archive.compatibility.clone(),
      items,
      assets: BTreeMap::new(),
      raw_manifest: archive.manifest.to_vec(),
      raw_index: archive.raw_index.to_vec(),
      raw_archive: archive.archive.to_vec(),
   }
}

fn board_item_from_records(
   index: usize,
   uuid: &str,
   records: Vec<&NativeRecord<'_>>,
) -> FreeformBoardItem {
   let record_ranges = records
      .iter()
      .map(|record| FreeformRecordRange { offset: record.offset, length: record.bytes.len() })
      .collect();
   let raw_data = records
      .first()
      .map_or_else(Vec::new, |record| record.bytes.to_vec());
   FreeformBoardItem {
      index,
      uuid: uuid.to_owned(),
      parent_id: None,
      class_name: None,
      hints: BTreeMap::new(),
      geometry: FreeformGeometry::default(),
      style: FreeformStyle::default(),
      kind: FreeformItemKind::Unknown,
      record_ranges,
      raw_data,
   }
}

/// Correlates a successfully decoded native tier with a successfully decoded
/// TSU tier and dispatches class-specific, record-bounded decoders.
///
/// Correlation is positional because both formats define an ordered top-level
/// board-item list. A cardinality mismatch is never guessed or partially
/// joined. On success, every item gets exactly one TSU entry.
pub fn join_crl_tsu(
   mut native: FreeformNative,
   entries: &[TsuEntry],
) -> Result<FreeformNative, FreeformDecodeError> {
   if native.items.len() != entries.len() {
      return Err(FreeformDecodeError::correlation(format!(
         "CRLNativeData has {} board items but TSUDescription has {}",
         native.items.len(),
         entries.len()
      )));
   }

   let item_ids = native
      .items
      .iter()
      .map(|item| item.uuid.clone())
      .collect::<Vec<_>>();
   for (item, entry) in native.items.iter_mut().zip(entries) {
      item.class_name = Some(entry.class_name.clone());
      item.hints = entry.hints.clone();
      if item.raw_data.is_empty() {
         continue;
      }
      let raw_data = std::mem::take(&mut item.raw_data);
      let record = NativeRecord { owner_id: item.uuid.clone(), offset: 0, bytes: &raw_data };
      let decoded = apply_class_decoder(item, &entry.class_name, &record, &item_ids);
      item.raw_data = raw_data;
      let asset = match decoded {
         Ok(asset) => asset,
         Err(error) if error.kind == super::types::FreeformDecodeErrorKind::CorrelationMismatch => {
            return Err(error);
         },
         Err(_) => continue,
      };
      if let Some(asset) = asset {
         if native.assets.insert(asset.id.clone(), asset).is_some() {
            return Err(FreeformDecodeError::correlation(
               "multiple native asset descriptors share an identifier",
            ));
         }
      }
   }
   apply_parent_ids(&mut native.items)?;

   Ok(native)
}

fn apply_class_decoder(
   item: &mut FreeformBoardItem,
   class_name: &str,
   record: &NativeRecord<'_>,
   item_ids: &[String],
) -> Result<Option<FreeformAsset>, FreeformDecodeError> {
   let common = shape::decode_shape(record).ok();
   if let Some((_, _, geometry, style)) = &common {
      item.geometry = geometry.clone();
      item.style = style.clone();
   }

   match class_name {
      "CRLWPShapeItem" | "CRLShapeItem" => {
         let (preset, path) = common.map_or((None, None), |(preset, path, ..)| (preset, path));
         item.kind = FreeformItemKind::Shape { preset, path };
      },
      "CRLWPTextBoxItem" | "CRLTextBoxItem" => {
         item.kind = FreeformItemKind::TextBox { text: text_table::decode_text(record).ok() };
      },
      "CRLWPStickyNoteItem" | "CRLStickyNoteItem" => {
         item.kind = FreeformItemKind::StickyNote { text: text_table::decode_text(record).ok() };
      },
      "CRLWPTableItem" | "CRLTableItem" => {
         let (row_heights, column_widths, cells) =
            text_table::decode_table(record).unwrap_or_default();
         item.kind = FreeformItemKind::Table { row_heights, column_widths, cells };
      },
      "CRLConnectionLineItem" | "CRLConnectorItem" => {
         let (tail, head, routing, path, horizontal_flip, vertical_flip) =
            connector::decode_connector(record)?;
         validate_connector_references(&tail, &head, item_ids)?;
         item.geometry.horizontal_flip = horizontal_flip;
         item.geometry.vertical_flip = vertical_flip;
         item.kind = FreeformItemKind::Connector { tail, head, routing, path };
      },
      "CRLGroupItem" => {
         let (child_ids, counter_transform) = connector::decode_group(record)?;
         validate_group_references(&child_ids, item_ids)?;
         item.kind = FreeformItemKind::Group { child_ids, counter_transform };
      },
      "CRLInkItem" | "CRLNativeInkItem" | "CRLFreehandDrawingItem" => {
         item.kind = FreeformItemKind::Ink { strokes: native_ink::decode_native_ink(record)? };
      },
      "CRLImageItem" => {
         let asset = assets::decode_asset(record);
         item.kind = match assets::decode_media(record) {
            FreeformItemKind::Image { asset_id, crop, mask } => {
               FreeformItemKind::Image { asset_id, crop, mask }
            },
            _ => FreeformItemKind::Image { asset_id: None, crop: None, mask: None },
         };
         return Ok(Some(asset));
      },
      "CRLMediaItem" | "CRLMovieItem" => {
         let asset = assets::decode_asset(record);
         item.kind = match assets::decode_media(record) {
            FreeformItemKind::Media { asset_id, media_type } => {
               FreeformItemKind::Media { asset_id, media_type }
            },
            _ => FreeformItemKind::Media { asset_id: None, media_type: None },
         };
         return Ok(Some(asset));
      },
      "CRLFileItem" => {
         let asset = assets::decode_asset(record);
         item.kind = match assets::decode_media(record) {
            FreeformItemKind::File { asset_id, filename } => {
               FreeformItemKind::File { asset_id, filename }
            },
            _ => FreeformItemKind::File { asset_id: None, filename: None },
         };
         return Ok(Some(asset));
      },
      "CRLURLItem" | "CRLUrlItem" => {
         item.kind = match assets::decode_media(record) {
            FreeformItemKind::Url { url, title } => FreeformItemKind::Url { url, title },
            _ => FreeformItemKind::Url { url: None, title: None },
         };
      },
      "CRLUsdzItem" | "CRLUSDZItem" => {
         let asset = assets::decode_asset(record);
         item.kind = match assets::decode_media(record) {
            FreeformItemKind::Usdz { asset_id, spatial_transform } => {
               FreeformItemKind::Usdz { asset_id, spatial_transform }
            },
            _ => FreeformItemKind::Usdz { asset_id: None, spatial_transform: None },
         };
         return Ok(Some(asset));
      },
      _ => {},
   }
   Ok(None)
}

fn validate_connector_references(
   tail: &FreeformConnectorEndpoint,
   head: &FreeformConnectorEndpoint,
   item_ids: &[String],
) -> Result<(), FreeformDecodeError> {
   for endpoint in [tail, head] {
      if let Some(item_id) = &endpoint.item_id {
         if !item_ids.iter().any(|candidate| candidate == item_id) {
            return Err(FreeformDecodeError::correlation(format!(
               "connector references missing board item {item_id}"
            )));
         }
      }
   }
   Ok(())
}

fn validate_group_references(
   child_ids: &[String],
   item_ids: &[String],
) -> Result<(), FreeformDecodeError> {
   for child_id in child_ids {
      if !item_ids.iter().any(|candidate| candidate == child_id) {
         return Err(FreeformDecodeError::correlation(format!(
            "group references missing board item {child_id}"
         )));
      }
   }
   Ok(())
}

fn apply_parent_ids(items: &mut [FreeformBoardItem]) -> Result<(), FreeformDecodeError> {
   let mut parents = BTreeMap::new();
   for item in items.iter() {
      if let FreeformItemKind::Group { child_ids, .. } = &item.kind {
         for child_id in child_ids {
            if child_id == &item.uuid {
               return Err(FreeformDecodeError::correlation(format!(
                  "group {} contains itself",
                  item.uuid
               )));
            }
            if let Some(previous) = parents.insert(child_id.clone(), item.uuid.clone()) {
               return Err(FreeformDecodeError::correlation(format!(
                  "board item {child_id} belongs to both {previous} and {}",
                  item.uuid
               )));
            }
         }
      }
   }
   for child_id in parents.keys() {
      let mut visited = BTreeSet::new();
      let mut current = child_id.as_str();
      while let Some(parent_id) = parents.get(current) {
         if !visited.insert(current) {
            return Err(FreeformDecodeError::correlation(format!(
               "groups contain a parent cycle through {current}"
            )));
         }
         current = parent_id;
      }
   }
   for item in items {
      item.parent_id = parents.remove(&item.uuid);
   }
   if let Some((child_id, parent_id)) = parents.into_iter().next() {
      return Err(FreeformDecodeError::correlation(format!(
         "group {parent_id} references missing board item {child_id}"
      )));
   }
   Ok(())
}

#[cfg(test)]
mod tests {
   use std::collections::HashMap;

   use super::*;

   const NATIVE: &[u8] = include_bytes!("../fixtures/real-board.crlnative");
   const TSU: &[u8] = include_bytes!("../fixtures/real-board.tsudescription");
   #[test]
   fn native_records_remain_unknown_and_lossless_without_tsu() {
      let native = decode_crl_native(NATIVE).expect("native fixture parses");
      assert_eq!(native.items.len(), 10);
      assert!(native.items.iter().any(|item| !item.raw_data.is_empty()));
      assert_eq!(native.paste_id, "42753C09-5C97-4F7E-A652-F842FEFFCEAD");
      assert!(native.items.iter().all(|item| item.class_name.is_none()));
      assert_eq!(native.raw_archive, parse_native_archive(NATIVE).unwrap().archive);
   }

   #[test]
   fn parsed_tsu_joins_by_equal_ordered_cardinality() {
      let entries = parse_tsu_description(TSU).expect("TSU fixture parses");
      let native = join_crl_tsu(decode_crl_native(NATIVE).unwrap(), &entries).expect("tiers join");
      for (item, entry) in native.items.iter().zip(entries.iter()) {
         assert_eq!(item.class_name.as_deref(), Some(entry.class_name.as_str()));
      }
   }

   #[test]
   fn malformed_tsu_is_an_independent_invalid_tier() {
      assert_eq!(
         parse_tsu_description(b"not a plist").unwrap_err().kind,
         super::super::types::FreeformDecodeErrorKind::Invalid
      );
      assert_eq!(decode_crl_native(NATIVE).unwrap().items.len(), 10);
   }

   #[test]
   fn tsu_values_remain_recursive_and_reject_unrepresentable_plist_types() {
      let value = Plist::Dict(HashMap::from([(
         "nested".into(),
         Plist::Array(vec![Plist::Bool(true), Plist::Data(vec![7, 8])]),
      )]));
      assert_eq!(
         tsu_value(&value).unwrap(),
         TsuValue::Dictionary(BTreeMap::from([(
            "nested".into(),
            TsuValue::Array(vec![TsuValue::Bool(true), TsuValue::Data(vec![7, 8])]),
         )]))
      );
      assert_eq!(
         tsu_value(&Plist::Uid(1)).unwrap_err().kind,
         super::super::types::FreeformDecodeErrorKind::Invalid
      );
   }
   #[test]
   fn mismatch_is_a_correlation_error_without_partial_join() {
      let native = decode_crl_native(NATIVE).unwrap();
      let entries = parse_tsu_description(TSU).unwrap();
      let error = join_crl_tsu(native, &entries[..entries.len() - 1]).unwrap_err();
      assert_eq!(error.kind, super::super::types::FreeformDecodeErrorKind::CorrelationMismatch);
   }

   #[test]
   fn unknown_item_retains_its_bounded_raw_record() {
      let bytes = [0xde, 0xad, 0xbe, 0xef];
      let record = NativeRecord { owner_id: "item-id".into(), offset: 17, bytes: &bytes };
      let item = board_item_from_records(0, "item-id", vec![&record]);
      assert!(matches!(item.kind, FreeformItemKind::Unknown));
      assert_eq!(item.raw_data, bytes);
   }
}
