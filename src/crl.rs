//! CRL native-archive integration.
//!
//! Explicit object directories establish ownership, boundaries, and native
//! kinds. Common, specific, and capsule payloads are decoded together; TSU
//! independently adds routing hints without replacing the native object graph.

mod assets;
mod coherence;
mod connector;
mod envelope;
mod native_ink;
mod shape;
mod text_table;

use std::collections::{BTreeMap, BTreeSet, btree_map::Entry};

use coherence::{Document, Node};
use envelope::{NativeArchive, NativeObject, parse_native_archive};

pub use super::types::TsuEntry;
use super::{
   bplist::{Plist, parse_bplist},
   types::{
      FreeformAsset, FreeformBoardItem, FreeformDecodeError, FreeformItemKind, FreeformNative,
      FreeformRecordRange, FreeformTableAnchor, TsuValue,
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

/// Decodes native kinds and their complete named payloads without requiring
/// TSU.
///
/// Malformed or truncated known records fail explicitly. Unsupported item kinds
/// retain their native discriminator and exact directory ranges.
pub fn decode_crl_native(data: &[u8]) -> Result<FreeformNative, FreeformDecodeError> {
   let archive = parse_native_archive(data)?;
   native_from_archive(&archive)
}

fn native_from_archive(archive: &NativeArchive<'_>) -> Result<FreeformNative, FreeformDecodeError> {
   let objects = archive
      .objects
      .iter()
      .map(|object| (object.id.as_str(), object))
      .collect::<BTreeMap<_, _>>();
   let mut assets = BTreeMap::new();
   let items = archive
      .item_ids
      .iter()
      .enumerate()
      .map(|(index, id)| {
         let object = objects.get(id.as_str()).ok_or_else(|| {
            FreeformDecodeError::correlation(format!("native index references missing object {id}"))
         })?;
         decode_object(index, object, &objects, &mut assets)
      })
      .collect::<Result<Vec<_>, _>>()?;
   validate_references(&items)?;
   Ok(FreeformNative {
      paste_id: archive.paste_id.clone(),
      compatibility: archive.compatibility.clone(),
      items,
      assets,
      raw_manifest: archive.manifest.to_vec(),
      raw_index: archive.raw_index.to_vec(),
      raw_archive: archive.archive.to_vec(),
   })
}

fn root<'a>(
   object: &NativeObject<'a>,
   name: &str,
) -> Result<Option<Node<'a>>, FreeformDecodeError> {
   object
      .region(name)
      .map(|region| Document::parse(region.bytes)?.root())
      .transpose()
      .map(Option::flatten)
}

fn decode_object(
   index: usize,
   object: &NativeObject<'_>,
   objects: &BTreeMap<&str, &NativeObject<'_>>,
   assets: &mut BTreeMap<String, FreeformAsset>,
) -> Result<FreeformBoardItem, FreeformDecodeError> {
   let common = root(object, "commonCRDTData")?;
   let specific = root(object, "specificCRDTData")?;
   if let Some(common) = common
      && let Some(id) = common
         .property("a")?
         .map(|value| value.uuid())
         .transpose()?
         .flatten()
      && id != object.id
   {
      return Err(FreeformDecodeError::correlation(format!(
         "object reference {} disagrees with common CRDT identity {id}",
         object.id
      )));
   }
   let shape::DecodedShape { preset, path, geometry, mut style } = common
      .map(|common| shape::decode_shape(common, specific, object.persistence_type))
      .transpose()?
      .unwrap_or_else(|| shape::DecodedShape {
         preset:   None,
         path:     None,
         geometry: Default::default(),
         style:    Default::default(),
      });
   let mut kind = if let Some(common) = common {
      match object.persistence_type {
         3 => FreeformItemKind::Shape {
            preset,
            path,
            text: specific
               .map(|specific| text_table::decode_text(specific, object.persistence_type))
               .transpose()?
               .flatten(),
         },
         12 => FreeformItemKind::StickyNote {
            text: specific
               .map(|specific| text_table::decode_text(specific, object.persistence_type))
               .transpose()?
               .flatten(),
         },
         2 if object.persistence_subtype == Some(1) => {
            let capsule = object.region("capsuleData").ok_or_else(|| {
               FreeformDecodeError::invalid("native table has no capsuleData payload")
            })?;
            let table = text_table::decode_table(Document::parse(capsule.bytes)?)?;
            style.fill = table.style.fill;
            style.stroke = table.style.stroke;
            FreeformItemKind::Table {
               row_heights:   table.row_heights,
               column_widths: table.column_widths,
               cells:         table.cells,
               options:       table.options,
            }
         },
         4 => {
            let specific = specific
               .ok_or_else(|| FreeformDecodeError::invalid("connector has no specificCRDTData"))?;
            let (tail, head, routing, path) = connector::decode_connector(common, specific)?;
            FreeformItemKind::Connector { tail, head, routing, path }
         },
         2 if object.persistence_subtype.unwrap_or(0) == 0 => {
            let specific = specific
               .ok_or_else(|| FreeformDecodeError::invalid("group has no specificCRDTData"))?;
            FreeformItemKind::Group {
               child_ids:         connector::decode_group(specific)?,
               counter_transform: None,
            }
         },
         5..=9 => {
            let specific = specific
               .ok_or_else(|| FreeformDecodeError::invalid("media has no specificCRDTData"))?;
            let (kind, descriptors) =
               assets::decode_media(common, specific, object.persistence_type)?;
            for asset in descriptors {
               match assets.entry(asset.id.clone()) {
                  Entry::Vacant(entry) => {
                     entry.insert(asset);
                  },
                  Entry::Occupied(entry) if entry.get() == &asset => {},
                  Entry::Occupied(_) => {
                     return Err(FreeformDecodeError::correlation(
                        "conflicting native asset descriptors share an identifier",
                     ));
                  },
               }
            }
            kind
         },
         10 => {
            let container = specific
               .map(|specific| specific.field("a"))
               .transpose()?
               .flatten()
               .ok_or_else(|| {
                  FreeformDecodeError::invalid("native ink container has no containerCRDTData")
               })?;
            FreeformItemKind::Group {
               child_ids:         connector::decode_group(container)?,
               counter_transform: None,
            }
         },
         11 => {
            let specific = specific
               .ok_or_else(|| FreeformDecodeError::invalid("native ink has no specificCRDTData"))?;
            let strokes = native_ink::decode_native_ink(common, specific)?;
            if strokes.is_empty() {
               FreeformItemKind::Shape { preset, path, text: None }
            } else {
               FreeformItemKind::Ink { strokes }
            }
         },
         _ => FreeformItemKind::Unknown,
      }
   } else {
      FreeformItemKind::Unknown
   };
   let children = object
      .children
      .iter()
      .enumerate()
      .map(|(index, id)| {
         let child = objects.get(id.as_str()).ok_or_else(|| {
            FreeformDecodeError::correlation(format!(
               "object {} references missing child {id}",
               object.id
            ))
         })?;
         decode_object(index, child, objects, assets)
      })
      .collect::<Result<Vec<_>, _>>()?;
   if let FreeformItemKind::Table { cells, .. } = &mut kind {
      for child in &children {
         if let Some(anchor) = &child.table_anchor {
            let cell = cells
               .iter_mut()
               .find(|cell| cell.id.as_deref() == Some(anchor.cell_id.as_str()))
               .ok_or_else(|| {
                  FreeformDecodeError::correlation(format!(
                     "table child {} references missing cell {}",
                     child.uuid, anchor.cell_id
                  ))
               })?;
            cell.anchored_item_ids.push(child.uuid.clone());
         }
      }
   }
   let table_anchor = common
      .map(|common| table_anchor(common, object.persistence_type))
      .transpose()?
      .flatten();
   Ok(FreeformBoardItem {
      index,
      uuid: object.id.clone(),
      parent_id: object.parent_id.clone(),
      table_anchor,
      persistence_type: object.persistence_type,
      persistence_subtype: object.persistence_subtype,
      class_name: class_name(object.persistence_type, object.persistence_subtype)
         .map(str::to_owned),
      hints: BTreeMap::new(),
      geometry,
      style,
      kind,
      children,
      record_ranges: object
         .regions
         .iter()
         .map(|region| FreeformRecordRange {
            name:   region.name.to_owned(),
            offset: region.offset,
            length: region.bytes.len(),
         })
         .collect(),
   })
}

fn table_anchor(
   common: Node<'_>,
   persistence_type: u32,
) -> Result<Option<FreeformTableAnchor>, FreeformDecodeError> {
   let Some(affinity) = common.property("b")? else {
      return Ok(None);
   };
   let affinity = if matches!(persistence_type, 3 | 4 | 11 | 12) {
      let Some(affinity) = affinity.field(0)? else {
         return Ok(None);
      };
      affinity
   } else {
      affinity
   };
   let Some(hint) = affinity.field(3)? else {
      return Ok(None);
   };
   let Some(cell) = hint.field(0)? else {
      return Ok(None);
   };
   let row = cell
      .field(0)?
      .map(|value| value.uuid())
      .transpose()?
      .flatten()
      .ok_or_else(|| FreeformDecodeError::invalid("table anchor has no row UUID"))?;
   let column = cell
      .field(1)?
      .map(|value| value.uuid())
      .transpose()?
      .flatten()
      .ok_or_else(|| FreeformDecodeError::invalid("table anchor has no column UUID"))?;
   let geometry = hint
      .field(1)?
      .ok_or_else(|| FreeformDecodeError::invalid("table anchor has no cell geometry"))?;
   Ok(Some(FreeformTableAnchor {
      cell_id:  format!("{row}/{column}"),
      geometry: shape::geometry(geometry)?,
   }))
}

fn class_name(persistence_type: u32, subtype: Option<u32>) -> Option<&'static str> {
   match persistence_type {
      2 if subtype.unwrap_or(0) == 0 => Some("CRLGroupItem"),
      2 if subtype == Some(1) => Some("CRLTableItem"),
      3 => Some("CRLWPShapeItem"),
      4 => Some("CRLConnectionLineItem"),
      5 => Some("CRLImageItem"),
      6 => Some("CRLMovieItem"),
      7 => Some("CRLFileItem"),
      8 => Some("CRLURLItem"),
      9 => Some("CRLUsdzItem"),
      10 => Some("CRLFreehandDrawingItem"),
      11 => Some("CRLFreehandDrawingShapeItem"),
      12 => Some("CRLWPStickyNoteItem"),
      _ => None,
   }
}

/// Correlates a successfully decoded native tier with a successfully decoded
/// TSU tier, preserving already decoded native kinds and nested children.
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

   for (item, entry) in native.items.iter_mut().zip(entries) {
      if let Some(native_class) = item.class_name.as_deref()
         && native_class != entry.class_name
      {
         return Err(FreeformDecodeError::correlation(format!(
            "native object {} is {native_class}, not {}",
            item.uuid, entry.class_name
         )));
      }
      item.class_name = Some(entry.class_name.clone());
      item.hints = entry.hints.clone();
      if matches!(entry.hints.get("textbox"), Some(TsuValue::Bool(true)))
         && let FreeformItemKind::Shape { text, .. } = &mut item.kind
      {
         item.kind = FreeformItemKind::TextBox { text: text.take() };
      }
      if let Some(TsuValue::Bool(premium)) = entry.hints.get("containsPremiumContent") {
         let asset_id = match &item.kind {
            FreeformItemKind::Image { asset_id, .. }
            | FreeformItemKind::Media { asset_id, .. }
            | FreeformItemKind::File { asset_id, .. }
            | FreeformItemKind::Usdz { asset_id, .. } => asset_id.as_deref(),
            _ => None,
         };
         if let Some(asset) = asset_id.and_then(|id| native.assets.get_mut(id)) {
            asset.premium = Some(*premium);
         }
      }
   }
   Ok(native)
}

fn validate_references(items: &[FreeformBoardItem]) -> Result<(), FreeformDecodeError> {
   fn collect<'a>(items: &'a [FreeformBoardItem], ids: &mut BTreeSet<&'a str>) {
      for item in items {
         ids.insert(&item.uuid);
         collect(&item.children, ids);
      }
   }
   fn validate(
      items: &[FreeformBoardItem],
      ids: &BTreeSet<&str>,
   ) -> Result<(), FreeformDecodeError> {
      for item in items {
         if let FreeformItemKind::Group { child_ids, .. } = &item.kind {
            for id in child_ids {
               if !item.children.iter().any(|child| &child.uuid == id) {
                  return Err(FreeformDecodeError::correlation(format!(
                     "group {} has no serialized child {id}",
                     item.uuid
                  )));
               }
            }
         }
         if let FreeformItemKind::Table { cells, .. } = &item.kind {
            for id in cells.iter().flat_map(|cell| &cell.anchored_item_ids) {
               if !ids.contains(id.as_str()) {
                  return Err(FreeformDecodeError::correlation(format!(
                     "table {} references missing item {id}",
                     item.uuid
                  )));
               }
            }
         }
         validate(&item.children, ids)?;
      }
      Ok(())
   }
   let mut ids = BTreeSet::new();
   collect(items, &mut ids);
   validate(items, &ids)
}

/// Labels malformed native data found by the CRL submodules.
fn invalid(message: &str) -> FreeformDecodeError {
   FreeformDecodeError::invalid(format!("native protobuf: {message}"))
}

/// Rejects non-finite numeric values before they reach the public model.
fn finite(value: f64) -> Result<f64, FreeformDecodeError> {
   value
      .is_finite()
      .then_some(value)
      .ok_or_else(|| invalid("non-finite numeric value"))
}

/// Widens an optional native float32, rejecting non-finite values.
fn float32(value: Option<f32>) -> Result<Option<f64>, FreeformDecodeError> {
   value.map(|value| finite(f64::from(value))).transpose()
}

#[cfg(test)]
mod tests {
   use super::*;
   const NATIVE: &[u8] = include_bytes!("../fixtures/real-board.crlnative");
   const TSU: &[u8] = include_bytes!("../fixtures/real-board.tsudescription");

   #[test]
   fn real_native_payloads_decode_without_tsu() {
      let native = decode_crl_native(NATIVE).unwrap();
      assert!(
         matches!(&native.items[7].kind, FreeformItemKind::StickyNote { text: Some(text) } if text.plain == "hi")
      );
      let table = &native.items[8];
      assert!(
         matches!(&table.kind, FreeformItemKind::Table { column_widths, cells, .. } if column_widths == &[344.0,344.0] && cells.len() == 4)
      );
      assert_eq!(table.children[0].uuid, "5AAC3C24-07E5-4429-8B8C-D0FF59B8961D");
      assert_eq!(table.children[0].parent_id.as_deref(), Some(table.uuid.as_str()));
      assert_eq!(native.raw_archive, parse_native_archive(NATIVE).unwrap().archive);
   }

   #[test]
   fn tsu_refines_textboxes_without_discarding_native_labels() {
      let native =
         join_crl_tsu(decode_crl_native(NATIVE).unwrap(), &parse_tsu_description(TSU).unwrap())
            .unwrap();
      assert!(matches!(native.items[0].kind, FreeformItemKind::TextBox { .. }));
      assert!(matches!(native.items[2].kind, FreeformItemKind::TextBox { .. }));
      assert!(matches!(native.items[5].kind, FreeformItemKind::Shape { .. }));
   }

   #[test]
   fn incorrect_tsu_count_or_class_cannot_relabel_native_objects() {
      let native = decode_crl_native(NATIVE).unwrap();
      let mut entries = parse_tsu_description(TSU).unwrap();
      assert!(join_crl_tsu(native.clone(), &entries[..entries.len() - 1]).is_err());
      entries[0].class_name = "CRLImageItem".into();
      assert!(join_crl_tsu(native, &entries).is_err());
   }

   #[test]
   fn malformed_tsu_does_not_prevent_native_decoding() {
      assert!(parse_tsu_description(b"not a plist").is_err());
      assert!(decode_crl_native(NATIVE).is_ok());
   }
}
