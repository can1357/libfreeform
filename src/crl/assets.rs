//! Bounded decoders for Freeform asset descriptors and media board items.
//!
//! CRL asset records are not self-describing enough to safely infer every
//! archived property.  This module only promotes values whose wire shape is
//! unambiguous and retains each bounded descriptor verbatim for the rest.

use super::envelope::NativeRecord;
use crate::types::{FreeformAsset, FreeformFrame, FreeformItemKind, FreeformPath, FreeformSize};
#[derive(Clone, Copy)]
enum Wire<'a> {
   Varint,
   Fixed32(u32),
   Bytes(&'a [u8]),
}

struct Field<'a> {
   number: u64,
   value:  Wire<'a>,
}

fn varint(bytes: &[u8], mut offset: usize) -> Option<(u64, usize)> {
   let mut value = 0_u64;
   for shift in 0..10 {
      let byte = *bytes.get(offset)?;
      offset += 1;
      value |= u64::from(byte & 0x7f) << (shift * 7);
      if byte & 0x80 == 0 {
         return Some((value, offset));
      }
   }
   None
}

fn fields(bytes: &[u8]) -> Option<Vec<Field<'_>>> {
   let mut offset = 0;
   let mut result = Vec::new();
   while offset < bytes.len() {
      let (tag, next) = varint(bytes, offset)?;
      offset = next;
      let number = tag >> 3;
      if number == 0 {
         return None;
      }
      let value = match tag & 7 {
         0 => {
            let (_, next) = varint(bytes, offset)?;
            offset = next;
            Wire::Varint
         },
         1 => {
            bytes.get(offset..offset.checked_add(8)?)?;
            offset += 8;
            // Fixed64 values have no proven asset mapping.
            continue;
         },
         2 => {
            let (length, next) = varint(bytes, offset)?;
            let length = usize::try_from(length).ok()?;
            let end = next.checked_add(length)?;
            let value = bytes.get(next..end)?;
            offset = end;
            Wire::Bytes(value)
         },
         5 => {
            let raw: [u8; 4] = bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?;
            offset += 4;
            Wire::Fixed32(u32::from_le_bytes(raw))
         },
         _ => return None,
      };
      result.push(Field { number, value });
   }
   Some(result)
}

fn strings(bytes: &[u8], output: &mut Vec<String>, depth: usize) {
   if depth == 8 {
      return;
   }
   let Some(message) = fields(bytes) else {
      return;
   };
   for field in message {
      let Wire::Bytes(value) = field.value else {
         continue;
      };
      if let Ok(text) = std::str::from_utf8(value)
         && !text.is_empty()
         && !text.chars().any(char::is_control)
      {
         output.push(text.to_owned());
      }
      strings(value, output, depth + 1);
   }
}

fn filename(strings: &[String]) -> Option<String> {
   strings.iter().find_map(|value| {
      let name = value.rsplit(['/', '\\']).next()?;
      let (_, extension) = name.rsplit_once('.')?;
      (!name.is_empty() && !extension.is_empty()).then(|| name.to_owned())
   })
}

fn extension(filename: &str) -> Option<&str> {
   let (_, extension) = filename.rsplit_once('.')?;
   (!extension.is_empty()).then_some(extension)
}

fn media_extension(extension: &str) -> bool {
   matches!(
      extension.to_ascii_lowercase().as_str(),
      "mov" | "mp4" | "m4v" | "avi" | "mkv" | "webm" | "mp3" | "m4a" | "wav" | "aiff"
   )
}

fn image_extension(extension: &str) -> bool {
   matches!(
      extension.to_ascii_lowercase().as_str(),
      "jpg" | "jpeg" | "png" | "heic" | "heif" | "gif" | "webp" | "tif" | "tiff" | "bmp"
   )
}

fn usdz_extension(extension: &str) -> bool {
   extension.eq_ignore_ascii_case("usdz")
}

fn frame(message: &[u8]) -> Option<FreeformFrame> {
   let fields = fields(message)?;
   if fields.len() != 4
      || !fields
         .iter()
         .enumerate()
         .all(|(index, field)| field.number == index as u64 + 1)
   {
      return None;
   }
   let mut values = [0.0; 4];
   for (index, field) in fields.iter().enumerate() {
      let Wire::Fixed32(bits) = field.value else {
         return None;
      };
      let value = f64::from(f32::from_bits(bits));
      if !value.is_finite() {
         return None;
      }
      values[index] = value;
   }
   // A crop frame has no archived rotation channel; zero is its defined
   // coordinate-system rotation, not an inferred item rotation.
   Some(FreeformFrame {
      x:        values[0],
      y:        values[1],
      w:        values[2],
      h:        values[3],
      rotation: 0.0,
   })
}

fn intrinsic_size(bytes: &[u8]) -> Option<FreeformSize> {
   let message = fields(bytes)?;
   message.into_iter().find_map(|field| {
      let Wire::Bytes(nested) = field.value else {
         return None;
      };
      let values = fields(nested)?;
      if values.len() != 2 || values[0].number != 1 || values[1].number != 2 {
         return None;
      }
      let (Wire::Fixed32(width), Wire::Fixed32(height)) = (values[0].value, values[1].value) else {
         return None;
      };
      let (width, height) = (f64::from(f32::from_bits(width)), f64::from(f32::from_bits(height)));
      (width.is_finite() && height.is_finite() && width >= 0.0 && height >= 0.0)
         .then_some(FreeformSize { width, height })
   })
}

fn crop(bytes: &[u8]) -> Option<FreeformFrame> {
   fields(bytes)?
      .into_iter()
      .find_map(|field| match field.value {
         Wire::Bytes(nested) => frame(nested),
         Wire::Varint | Wire::Fixed32(_) => None,
      })
}

fn mask(bytes: &[u8], crop: Option<FreeformFrame>) -> Option<FreeformPath> {
   let crop_seen = crop.is_some();
   fields(bytes)?.into_iter().find_map(|field| {
      let Wire::Bytes(nested) = field.value else {
         return None;
      };
      if crop_seen && frame(nested).is_none() && fields(nested).is_some() {
         Some(FreeformPath {
            commands:     Vec::new(),
            natural_size: None,
            raw_data:     nested.to_vec(),
         })
      } else {
         None
      }
   })
}

fn spatial_transform(bytes: &[u8]) -> Option<Vec<f64>> {
   fields(bytes)?.into_iter().find_map(|field| {
      let Wire::Bytes(nested) = field.value else {
         return None;
      };
      let values = fields(nested)?;
      if values.len() != 16
         || !values
            .iter()
            .enumerate()
            .all(|(index, field)| field.number == index as u64 + 1)
      {
         return None;
      }
      values
         .into_iter()
         .map(|field| match field.value {
            Wire::Fixed32(bits) => {
               let value = f64::from(f32::from_bits(bits));
               value.is_finite().then_some(value)
            },
            Wire::Varint | Wire::Bytes(_) => None,
         })
         .collect()
   })
}

fn url(strings: &[String]) -> Option<String> {
   strings
      .iter()
      .find(|value| value.starts_with("https://") || value.starts_with("http://"))
      .cloned()
}

/// Decodes one bounded native asset descriptor without inspecting neighbouring
/// records.
///
/// The descriptor's owner UUID is authoritative.  Unrecognised descriptor
/// fields remain in [`FreeformAsset::raw_descriptor`], including malformed
/// protobuf data.
pub fn decode_asset(record: &NativeRecord<'_>) -> FreeformAsset {
   let mut values = Vec::new();
   strings(record.bytes, &mut values, 0);
   FreeformAsset {
      id:             record.owner_id.clone(),
      filename:       filename(&values),
      uti:            None,
      bytes:          None,
      premium:        values
         .iter()
         .any(|value| value.eq_ignore_ascii_case("premium"))
         .then_some(true),
      intrinsic_size: intrinsic_size(record.bytes),
      raw_descriptor: record.bytes.to_vec(),
   }
}

/// Decodes a bounded image, media, file, URL, or USDZ record.
///
/// The decoder classifies only concrete URL schemes and filename extensions.
/// It returns [`FreeformItemKind::Unknown`] for records without a proven item
/// payload.
pub fn decode_media(record: &NativeRecord<'_>) -> FreeformItemKind {
   let mut values = Vec::new();
   strings(record.bytes, &mut values, 0);
   if let Some(url) = url(&values) {
      let title = values
         .into_iter()
         .find(|value| value != &url && filename(std::slice::from_ref(value)).is_none());
      return FreeformItemKind::Url { url: Some(url), title };
   }
   let name = filename(&values);
   if let Some(name) = name.as_deref()
      && let Some(extension) = extension(name)
   {
      if usdz_extension(extension) {
         return FreeformItemKind::Usdz {
            asset_id:          Some(record.owner_id.clone()),
            spatial_transform: spatial_transform(record.bytes),
         };
      }
      if media_extension(extension) {
         return FreeformItemKind::Media {
            asset_id:   Some(record.owner_id.clone()),
            media_type: Some(extension.to_ascii_lowercase()),
         };
      }
      if image_extension(extension) {
         let crop = crop(record.bytes);
         return FreeformItemKind::Image {
            asset_id: Some(record.owner_id.clone()),
            mask: mask(record.bytes, crop),
            crop,
         };
      }
      return FreeformItemKind::File {
         asset_id: Some(record.owner_id.clone()),
         filename: Some(name.to_owned()),
      };
   }
   if let Some(crop) = crop(record.bytes) {
      return FreeformItemKind::Image {
         asset_id: Some(record.owner_id.clone()),
         mask:     mask(record.bytes, Some(crop)),
         crop:     Some(crop),
      };
   }
   FreeformItemKind::Unknown
}

#[cfg(test)]
mod tests {
   use super::*;

   fn encoded_string(value: &str) -> Vec<u8> {
      let mut bytes = vec![0x0a, value.len() as u8];
      bytes.extend_from_slice(value.as_bytes());
      bytes
   }

   fn encoded_bytes(field: u8, value: &[u8]) -> Vec<u8> {
      let mut bytes = vec![field << 3 | 2, value.len() as u8];
      bytes.extend_from_slice(value);
      bytes
   }

   fn fixed32(field: u8, value: f32) -> Vec<u8> {
      let mut bytes = vec![field << 3 | 5];
      bytes.extend_from_slice(&value.to_bits().to_le_bytes());
      bytes
   }

   fn record<'a>(id: &str, bytes: &'a [u8]) -> NativeRecord<'a> {
      NativeRecord { owner_id: id.to_owned(), offset: 0, bytes }
   }

   #[test]
   fn decodes_embedded_image_crop_and_raw_mask() {
      let mut crop = Vec::new();
      for (field, value) in [(1, 1.0), (2, 2.0), (3, 30.0), (4, 40.0)] {
         crop.extend(fixed32(field, value));
      }
      let mut bytes = encoded_string("photo.png");
      bytes.extend(encoded_bytes(2, &crop));
      bytes.extend(encoded_bytes(3, &[0x08, 0x01]));

      let item = decode_media(&record("A", &bytes));
      let FreeformItemKind::Image { asset_id, crop, mask } = item else {
         panic!("expected image");
      };
      assert_eq!(asset_id.as_deref(), Some("A"));
      assert_eq!(crop.unwrap().w, 30.0);
      assert_eq!(mask.unwrap().raw_data, vec![0x08, 0x01]);
   }

   #[test]
   fn premium_descriptor_without_bytes_retains_descriptor() {
      let bytes = encoded_string("premium");
      let asset = decode_asset(&record("P", &bytes));
      assert_eq!(asset.id, "P");
      assert_eq!(asset.premium, Some(true));
      assert_eq!(asset.bytes, None);
      assert_eq!(asset.raw_descriptor, bytes);
   }

   #[test]
   fn decodes_movie_file_url_and_usdz() {
      let movie = decode_media(&record("M", &encoded_string("clip.mov")));
      assert_eq!(movie, FreeformItemKind::Media {
         asset_id:   Some("M".into()),
         media_type: Some("mov".into()),
      });
      let file = decode_media(&record("F", &encoded_string("notes.unknownext")));
      assert_eq!(file, FreeformItemKind::File {
         asset_id: Some("F".into()),
         filename: Some("notes.unknownext".into()),
      });
      let mut url = encoded_string("https://example.test/page");
      url.extend(encoded_bytes(2, b"Example"));
      assert_eq!(decode_media(&record("U", &url)), FreeformItemKind::Url {
         url:   Some("https://example.test/page".into()),
         title: Some("Example".into()),
      });
      let usdz = decode_media(&record("S", &encoded_string("model.usdz")));
      assert_eq!(usdz, FreeformItemKind::Usdz {
         asset_id:          Some("S".into()),
         spatial_transform: None,
      });
   }

   #[test]
   fn malformed_descriptor_is_a_non_lossy_unknown_fallback() {
      let raw = [0x0a, 0x05, b'x'];
      let asset = decode_asset(&record("X", &raw));
      assert_eq!(asset.raw_descriptor, raw);
      assert_eq!(decode_media(&record("X", &raw)), FreeformItemKind::Unknown);
   }
}
