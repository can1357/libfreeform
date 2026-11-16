//! Native media CRDTs and their integer-keyed `CRLAssetData` descriptors.

use super::{
   coherence::{Node, Value},
   finite, invalid, shape,
};
use crate::types::{FreeformAsset, FreeformDecodeError, FreeformItemKind};

type Result<T, E = FreeformDecodeError> = std::result::Result<T, E>;

/// `CRLAssetData.encode(to:)` (0x101208a70): identity is key 0, filename
/// key 1, media-library UUID key 2, and may-need-watermark key 3. In
/// particular the media-library UUID and the owning item's UUID are not asset
/// IDs.
fn asset(value: Value<'_>) -> Result<Option<FreeformAsset>> {
   let Some(value) = value.optional()? else {
      return Ok(None);
   };
   let Some(id) = value.field(0)?.map(Value::uuid).transpose()?.flatten() else {
      return Ok(None);
   };
   let filename = value
      .field(1)?
      .map(Value::string)
      .transpose()?
      .flatten()
      .map(str::to_owned);
   let premium = value
      .field(3)?
      .map(Value::optional)
      .transpose()?
      .flatten()
      .map(Value::boolean)
      .transpose()?
      .flatten();
   Ok(Some(FreeformAsset {
      id,
      filename,
      uti: None,
      bytes: None,
      premium,
      intrinsic_size: None,
      raw_descriptor: value.bytes.to_vec(),
   }))
}

fn append_asset(
   values: Option<Value<'_>>,
   key: u64,
   assets: &mut Vec<FreeformAsset>,
) -> Result<Option<String>> {
   let Some(value) = values.map(|value| value.field(key)).transpose()?.flatten() else {
      return Ok(None);
   };
   let Some(asset) = asset(value)? else {
      return Ok(None);
   };
   let id = asset.id.clone();
   if !assets.iter().any(|existing| existing.id == id) {
      assets.push(asset);
   }
   Ok(Some(id))
}

/// `CRL3DRotation.rawValue` stores SIMD quaternion x/y/z/w as four big-endian
/// float32 values (0x100f9af30), separate from the item's full 3D transform.
fn rotation(value: Value<'_>) -> Result<Option<[f64; 4]>> {
   let Some(bytes) = value.data()? else {
      return Ok(None);
   };
   let raw = bytes
      .as_array::<16>()
      .ok_or_else(|| invalid("invalid USDZ quaternion length"))?;
   let mut q = [0.0; 4];
   for (component, bytes) in q.iter_mut().zip(raw.as_chunks::<4>().0) {
      *component = finite(f64::from(f32::from_be_bytes(*bytes)))?;
   }
   Ok(Some(q))
}

/// `CRL3DTransform` encoder 0x100a88ffc stores four SIMD4<Float> columns,
/// each an unkeyed Coherence value collection, under integer keys 0 through 3.
fn transform(value: Value<'_>) -> Result<Option<Vec<f64>>> {
   let Some(value) = value.optional()? else {
      return Ok(None);
   };
   let mut matrix = Vec::with_capacity(16);
   for key in 0..4 {
      let column = value
         .field(key)?
         .ok_or_else(|| invalid("missing spatial transform column"))?;
      let start = matrix.len();
      for component in column.elements()? {
         matrix.push(
            component?
               .number()?
               .ok_or_else(|| invalid("non-numeric spatial transform component"))?,
         );
      }
      if matrix.len() - start != 4 {
         return Err(invalid("spatial transform column is not SIMD4"));
      }
   }
   Ok(Some(matrix))
}

/// Classifies by the native persistence discriminator, never by a filename or
/// strings found in unrelated nested records. Auxiliary assets retain their own
/// IDs.
pub fn decode_media(
   common: Node<'_>,
   specific: Node<'_>,
   persistence_type: u32,
) -> Result<(FreeformItemKind, Vec<FreeformAsset>)> {
   let values = specific.property("a")?;
   let mut assets = Vec::new();
   let asset_id = append_asset(values, 0, &mut assets)?;
   let kind = match persistence_type {
      5 => {
         append_asset(values, 1, &mut assets)?;
         let mask_data = specific.property("c")?;
         let mask = mask_data
            .map(|value| value.field(0))
            .transpose()?
            .flatten()
            .map(shape::path)
            .transpose()?
            .flatten();
         let crop = mask_data
            .map(|value| value.field(1))
            .transpose()?
            .flatten()
            .map(shape::geometry)
            .transpose()?
            .and_then(|geometry| geometry.frame);
         FreeformItemKind::Image { asset_id, crop, mask }
      },
      6 => {
         append_asset(values, 1, &mut assets)?;
         let audio_only = values
            .map(|value| value.field(5))
            .transpose()?
            .flatten()
            .map(Value::boolean)
            .transpose()?
            .flatten();
         FreeformItemKind::Media {
            asset_id,
            media_type: audio_only.map(|audio| if audio { "audio" } else { "movie" }.to_owned()),
         }
      },
      7 => {
         append_asset(values, 1, &mut assets)?;
         let filename = assets.first().and_then(|asset| asset.filename.clone());
         FreeformItemKind::File { asset_id, filename }
      },
      8 => {
         let url = values
            .map(|value| value.field(1))
            .transpose()?
            .flatten()
            .map(Value::url)
            .transpose()?
            .flatten();
         FreeformItemKind::Url { url, title: None }
      },
      9 => {
         append_asset(values, 1, &mut assets)?;
         let orientation = values
            .map(|value| value.field(2))
            .transpose()?
            .flatten()
            .map(rotation)
            .transpose()?
            .flatten();
         let spatial_transform = common
            .property("b")?
            .map(|value| value.field(2))
            .transpose()?
            .flatten()
            .map(transform)
            .transpose()?
            .flatten();
         FreeformItemKind::Usdz { asset_id, spatial_transform, orientation }
      },
      _ => FreeformItemKind::Unknown,
   };
   Ok((kind, assets))
}

#[cfg(test)]
mod tests {
   use super::*;
   use crate::crl::{coherence::Document, envelope::parse_native_archive};

   #[test]
   fn captured_premium_image_uses_asset_identity_and_native_mask() {
      let archive =
         parse_native_archive(include_bytes!("../../fixtures/real-board.crlnative")).unwrap();
      let image = archive
         .objects
         .iter()
         .find(|object| object.persistence_type == 5)
         .unwrap();
      let common = Document::parse(image.region("commonCRDTData").unwrap().bytes)
         .unwrap()
         .root()
         .unwrap()
         .unwrap();
      let specific = Document::parse(image.region("specificCRDTData").unwrap().bytes)
         .unwrap()
         .root()
         .unwrap()
         .unwrap();
      let (kind, assets) = decode_media(common, specific, 5).unwrap();
      assert_eq!(assets.len(), 1);
      assert_eq!(assets[0].id, "EC3BE892-0B72-5127-9352-B3304685DF29");
      assert_ne!(assets[0].id, image.id);
      assert_eq!(assets[0].filename.as_deref(), Some("2F9A0393-9103-584B-A2CA-F9B0AE4BD4AB.jpg"));
      assert_eq!(assets[0].premium, Some(true));
      assert!(assets[0].bytes.is_none());
      let FreeformItemKind::Image { asset_id, crop, mask } = kind else {
         panic!("expected image");
      };
      assert_eq!(asset_id.as_deref(), Some(assets[0].id.as_str()));
      let crop = crop.unwrap();
      assert_eq!((crop.x, crop.y, crop.w, crop.h), (0.0, 0.0, 250.0, 187.5));
      assert!(matches!(mask.unwrap().source,
         Some(crate::types::FreeformPathSource::Scalar { name, scalar, .. }) if name == "roundedRectangle" && scalar == 10.0));
   }
}
