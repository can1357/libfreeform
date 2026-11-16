//! Decoder for Apple's `com.apple.drawing` `PencilKit` pasteboard flavor.
//!
//! The `wrd` body is protobuf, while each path's point records are packed
//! binary data.  This module deliberately only exposes fields whose layout is
//! established by the installed `PencilKit` format; all other bytes remain on
//! their enclosing stroke.

use crate::{
   types::{
      FreeformColor, FreeformColorComponents, FreeformDecodeError, FreeformDrawing, FreeformFrame,
      FreeformInkPoint, FreeformInkPointRole, FreeformInkRange, FreeformInkStroke, FreeformPath,
      FreeformPathCommand, FreeformPoint, FreeformTransform,
   },
   wire::Repeated,
};

const MAGIC: &[u8; 3] = b"wrd";
// PKStrokePathPointsShared::computeCachedValues enumerates these twelve
// channels in bit order, independently for constant and per-point data.
// Elevation (bit 11) remains in the raw stroke until its semantics are
// established.
const CHANNEL_SIZES: [usize; 12] = [8, 4, 4, 2, 2, 2, 2, 2, 2, 4, 2, 4];
const KNOWN_CHANNELS: u64 = (1 << CHANNEL_SIZES.len()) - 1;

/// `PencilKit` drawing schemas.
mod proto {
   use crate::wire::{self, Repeated};

   wire::message! {
      /// `wrd` body: the shared ink table and every stroke.
      pub struct Drawing<'a> {
         inks: Repeated<'a, Ink<'a>> = 4,
         /// Encoded strokes, retained verbatim on each decoded stroke.
         strokes: Repeated<'a, &'a [u8]> = 5,
      }
   }

   wire::message! {
      /// One ink: its color and `com.apple.ink.*` identifier.
      pub struct Ink<'a> {
         color: Option<Color> = 1,
         identifier: Option<&'a str> = 2,
      }
   }

   wire::message! {
      /// sRGB color channels.
      pub struct Color {
         red: Option<f32> = 1,
         green: Option<f32> = 2,
         blue: Option<f32> = 3,
         alpha: Option<f32> = 4,
      }
   }

   wire::message! {
      /// One stroke record. Masked or erased records keep the shared path here
      /// and list each visible stroke in `substrokes`.
      pub struct Stroke<'a> {
         /// Index into [`Drawing::inks`].
         ink: Option<u64> = 4,
         path: Option<Path<'a>> = 5,
         /// Canvas-space render bounds, already transformed and expanded for
         /// the actual ink.
         bounds: Option<Rect> = 6,
         transform: Option<Transform> = 7,
         /// Bit 37 marks bits 5..36 as the random seed.
         flags: Option<u64> = 8,
         substrokes: Repeated<'a, Substroke<'a>> = 11,
      }
   }

   wire::message! {
      /// One visible stroke cut from its record's path. Absent ink,
      /// transform, and flags inherit the record's; bounds never do.
      pub struct Substroke<'a> {
         ink: Option<u64> = 4,
         bounds: Option<Rect> = 6,
         transform: Option<Transform> = 7,
         flags: Option<u64> = 8,
         /// Closed polygons in stroke-local space, as float32 (x, y) pairs.
         mask: Repeated<'a, &'a [u8]> = 10,
         /// Visible parametric ranges as consecutive (start, end) values.
         ranges: Repeated<'a, f32> = 12,
      }
   }

   wire::message! {
      /// Stroke points as packed per-point and constant channel records.
      pub struct Path<'a> {
         count: Option<u64> = 3,
         /// Channels stored per point.
         point_mask: Option<u64> = 4,
         /// Channels shared by every point.
         constant_mask: Option<u64> = 5,
         constants: Option<&'a [u8]> = 6,
         points: Option<&'a [u8]> = 7,
      }
   }

   wire::message! {
      /// Affine stroke transform.
      pub struct Transform {
         a: Option<f32> = 1,
         b: Option<f32> = 2,
         c: Option<f32> = 3,
         d: Option<f32> = 4,
         tx: Option<f32> = 5,
         ty: Option<f32> = 6,
      }
   }

   wire::message! {
      /// Axis-aligned rectangle.
      pub struct Rect {
         x: Option<f32> = 1,
         y: Option<f32> = 2,
         width: Option<f32> = 3,
         height: Option<f32> = 4,
      }
   }
}

struct Ink<'a> {
   identifier: &'a str,
   color:      Option<FreeformColor>,
}

/// The fields of one stroke `PKDrawing` exposes: a whole record, or one
/// substroke with its record's fields inherited.
struct Visible<'a> {
   ink:       Option<u64>,
   bounds:    Option<proto::Rect>,
   transform: Option<proto::Transform>,
   flags:     Option<u64>,
   cut:       Option<proto::Substroke<'a>>,
}

impl<'a> Visible<'a> {
   const fn whole(record: &proto::Stroke<'a>) -> Self {
      Self {
         ink:       record.ink,
         bounds:    record.bounds,
         transform: record.transform,
         flags:     record.flags,
         cut:       None,
      }
   }

   fn cut(record: &proto::Stroke<'a>, substroke: proto::Substroke<'a>) -> Self {
      Self {
         ink:       substroke.ink.or(record.ink),
         bounds:    substroke.bounds,
         transform: substroke.transform.or(record.transform),
         flags:     substroke.flags.or(record.flags),
         cut:       Some(substroke),
      }
   }
}

fn invalid(message: impl Into<String>) -> FreeformDecodeError {
   FreeformDecodeError::invalid(format!("com.apple.drawing: {}", message.into()))
}

fn finite(value: f32) -> Option<f64> {
   let value = f64::from(value);
   value.is_finite().then_some(value)
}

/// Requires every channel; a missing or non-finite one leaves the ink
/// uncolored.
fn color(color: proto::Color) -> Option<FreeformColor> {
   let [Some(red), Some(green), Some(blue), Some(alpha)] =
      [color.red, color.green, color.blue, color.alpha].map(|channel| channel.and_then(finite))
   else {
      return None;
   };
   Some(FreeformColor {
      color_space: "sRGB".into(),
      components: FreeformColorComponents::Rgb { red, green, blue },
      alpha,
      hex: Some(to_hex(red, green, blue)),
   })
}

fn transform(transform: proto::Transform) -> Result<FreeformTransform, FreeformDecodeError> {
   let proto::Transform { a, b, c, d, tx, ty } = transform;
   let [Some(a), Some(b), Some(c), Some(d), Some(tx), Some(ty)] =
      [a, b, c, d, tx, ty].map(|value| value.and_then(finite))
   else {
      return Err(invalid("incomplete stroke transform"));
   };
   Ok(FreeformTransform { a, b, c, d, tx, ty })
}

/// Closes each mask polygon as one subpath; `None` when nothing is stored.
fn mask(polygons: Repeated<'_, &[u8]>) -> Result<Option<FreeformPath>, FreeformDecodeError> {
   let mut path = FreeformPath::default();
   for polygon in polygons {
      let polygon = polygon?;
      let (coordinates, []) = polygon.as_chunks::<4>() else {
         return Err(invalid("mask polygon is not float32 pairs"));
      };
      let (vertices, []) = coordinates.as_chunks::<2>() else {
         return Err(invalid("mask polygon is not float32 pairs"));
      };
      for (index, &[x, y]) in vertices.iter().enumerate() {
         let coordinate = |bytes| {
            finite(f32::from_le_bytes(bytes)).ok_or_else(|| invalid("non-finite mask vertex"))
         };
         let point = FreeformPoint { x: coordinate(x)?, y: coordinate(y)? };
         path.commands.push(if index == 0 {
            FreeformPathCommand::Move { point }
         } else {
            FreeformPathCommand::Line { point }
         });
      }
      if !vertices.is_empty() {
         path.commands.push(FreeformPathCommand::Close);
      }
      path.raw_data.extend_from_slice(polygon);
   }
   Ok((!path.commands.is_empty()).then_some(path))
}

/// Pairs consecutive range bounds; `None` when nothing is stored.
fn visible_ranges(
   bounds: Repeated<'_, f32>,
) -> Result<Option<Vec<FreeformInkRange>>, FreeformDecodeError> {
   let bound = |value: Result<f32, _>| {
      finite(value?).ok_or_else(|| invalid("non-finite visible range bound"))
   };
   let mut bounds = bounds.into_iter();
   let mut ranges = Vec::new();
   while let Some(start) = bounds.next() {
      let end = bounds
         .next()
         .ok_or_else(|| invalid("visible range has no end"))?;
      ranges.push(FreeformInkRange { start: bound(start)?, end: bound(end)? });
   }
   Ok((!ranges.is_empty()).then_some(ranges))
}

/// Requires every edge and a non-negative size.
fn frame(rect: proto::Rect) -> Option<FreeformFrame> {
   let [Some(x), Some(y), Some(w), Some(h)] =
      [rect.x, rect.y, rect.width, rect.height].map(|value| value.and_then(finite))
   else {
      return None;
   };
   (w >= 0.0 && h >= 0.0).then_some(FreeformFrame { x, y, w, h, rotation: 0.0 })
}

fn union_bounds(left: FreeformFrame, right: FreeformFrame) -> FreeformFrame {
   let x = left.x.min(right.x);
   let y = left.y.min(right.y);
   FreeformFrame {
      x,
      y,
      w: (left.x + left.w).max(right.x + right.w) - x,
      h: (left.y + left.h).max(right.y + right.h) - y,
      rotation: 0.0,
   }
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
   let drawing = proto::Drawing::parse(body)?;
   let inks = drawing
      .inks
      .into_iter()
      .map(|ink| {
         let ink = ink?;
         Ok(Ink {
            identifier: ink.identifier.unwrap_or_default(),
            color:      ink.color.and_then(color),
         })
      })
      .collect::<Result<Vec<_>, FreeformDecodeError>>()?;
   let mut strokes = Vec::new();
   let mut bounds = None;
   let mut complete_bounds = true;
   for raw_data in drawing.strokes {
      let raw_data = raw_data?;
      let record = proto::Stroke::parse(raw_data)?;
      let path = record.path.ok_or_else(|| invalid("stroke missing path"))?;
      let mut points = decode_points(&path)?;
      let mut emit = |visible: Visible<'_>, points| -> Result<(), FreeformDecodeError> {
         // Control extrema cannot reproduce the native render bounds.
         if let Some(stroke_bounds) = visible.bounds.and_then(frame) {
            bounds =
               Some(bounds.map_or(stroke_bounds, |previous| union_bounds(previous, stroke_bounds)));
         } else {
            complete_bounds = false;
         }
         let index = visible
            .ink
            .ok_or_else(|| invalid("stroke missing ink reference"))?;
         let ink = inks
            .get(usize::try_from(index).map_err(|_| invalid("ink reference overflow"))?)
            .ok_or_else(|| invalid("stroke references missing ink"))?;
         strokes.push(FreeformInkStroke {
            ink_type: ink_type_from_identifier(ink.identifier).to_owned(),
            ink_identifier: ink.identifier.to_owned(),
            color: ink.color.clone(),
            transform: visible
               .transform
               .map(transform)
               .transpose()?
               .unwrap_or_default(),
            point_role: FreeformInkPointRole::SplineControl,
            points,
            visible_ranges: visible
               .cut
               .map(|cut| visible_ranges(cut.ranges))
               .transpose()?
               .flatten(),
            mask: visible.cut.map(|cut| mask(cut.mask)).transpose()?.flatten(),
            // -[PKStroke randomSeed] uses bits 5..36 only when bit 37 is set.
            // Older strokes derive a seed from identity instead; leave that
            // absent.
            random_seed: visible
               .flags
               .filter(|flags| flags & (1 << 37) != 0)
               .map(|flags| (flags >> 5) as u32),
            raw_data: raw_data.to_vec(),
         });
         Ok(())
      };
      let mut substrokes = record.substrokes.into_iter().peekable();
      if substrokes.peek().is_none() {
         emit(Visible::whole(&record), points)?;
         continue;
      }
      while let Some(substroke) = substrokes.next() {
         let points = if substrokes.peek().is_some() {
            points.clone()
         } else {
            std::mem::take(&mut points)
         };
         emit(Visible::cut(&record, substroke?), points)?;
      }
   }
   Ok(FreeformDrawing {
      required_content_version: None,
      bounds: if complete_bounds { bounds } else { None },
      strokes,
   })
}

fn decode_points(path: &proto::Path<'_>) -> Result<Vec<FreeformInkPoint>, FreeformDecodeError> {
   let count = usize::try_from(
      path
         .count
         .ok_or_else(|| invalid("path missing point count"))?,
   )
   .map_err(|_| invalid("point count overflow"))?;
   let point_mask = path
      .point_mask
      .ok_or_else(|| invalid("path missing point channel mask"))?;
   let constant_mask = path.constant_mask.unwrap_or(0);
   if (point_mask | constant_mask) & !KNOWN_CHANNELS != 0 {
      return Err(FreeformDecodeError::unsupported(
         "com.apple.drawing: unsupported point channel mask",
      ));
   }
   if point_mask & constant_mask != 0 {
      return Err(invalid("overlapping constant and point channel masks"));
   }
   let stride = channel_size(point_mask);
   let blob = path.points.unwrap_or_default();
   if count.checked_mul(stride) != Some(blob.len()) {
      return Err(invalid("point count and channel mask do not match packed point data"));
   }
   let constants = path.constants.unwrap_or_default();
   if constants.len() != channel_size(constant_mask) {
      return Err(invalid("constant channel mask does not match packed constant data"));
   }
   // A mask is authoritative: do not invent channels that are absent from both
   // streams. Native archives always keep the location in the per-point stream.
   if count != 0 && point_mask & 1 == 0 {
      return Err(invalid("path missing per-point locations"));
   }
   let mut template = [None; 13];
   decode_channels(constants, constant_mask, &mut template)?;
   let mut points = Vec::with_capacity(count);
   if count == 0 {
      return Ok(points);
   }
   for record in blob.chunks_exact(stride) {
      let mut channels = template;
      decode_channels(record, point_mask, &mut channels)?;
      let [
         x,
         y,
         time_offset,
         width,
         aspect,
         edge_width,
         force,
         azimuth,
         altitude,
         opacity,
         radius2,
         threshold,
         _,
      ] = channels;
      let x = x.ok_or_else(|| invalid("non-finite point x"))?;
      let y = y.ok_or_else(|| invalid("non-finite point y"))?;
      points.push(FreeformInkPoint {
         x,
         y,
         time_offset,
         width,
         height: width.zip(aspect).map(|(width, aspect)| width * aspect),
         opacity,
         force,
         azimuth,
         altitude,
         secondary_scale: radius2.zip(width).and_then(|(radius, width)| {
            let scale = radius / width;
            scale.is_finite().then_some(scale)
         }),
         threshold,
         edge_width,
      });
   }
   Ok(points)
}

fn channel_size(mask: u64) -> usize {
   CHANNEL_SIZES
      .iter()
      .enumerate()
      .fold(0, |size, (index, width)| size + if mask & (1 << index) != 0 { *width } else { 0 })
}

fn decode_channels(
   bytes: &[u8],
   mask: u64,
   channels: &mut [Option<f64>; 13],
) -> Result<(), FreeformDecodeError> {
   let mut offset = 0;
   for (index, size) in CHANNEL_SIZES.iter().copied().enumerate() {
      if mask & (1 << index) == 0 {
         continue;
      }
      let value = match index {
         0 => {
            channels[0] = f32_at(bytes, offset)?;
            f32_at(bytes, offset + 4)?
         },
         1 | 2 | 9 | 11 => f32_at(bytes, offset)?,
         _ => {
            let raw = f64::from(u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap()));
            Some(match index {
               3..=5 => raw / 1000.0,
               6 => raw / (65535.0 / std::f64::consts::TAU) - std::f64::consts::PI,
               7 => std::f64::consts::FRAC_PI_2 - raw / (65535.0 / std::f64::consts::FRAC_PI_2),
               8 | 10 => raw / 32767.5,
               _ => raw,
            })
         },
      };
      channels[index + 1] = value;
      offset += size;
   }
   Ok(())
}

fn f32_at(record: &[u8], offset: usize) -> Result<Option<f64>, FreeformDecodeError> {
   let bytes: [u8; 4] = record
      .get(offset..offset + 4)
      .ok_or_else(|| invalid("truncated point record"))?
      .try_into()
      .unwrap();
   Ok(finite(f32::from_le_bytes(bytes)))
}

/// Normalizes known `PencilKit` ink families without changing unknown
/// identifiers.
pub(crate) fn ink_type_from_identifier(identifier: &str) -> &str {
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

   #[test]
   fn masked_and_erased_records_match_pencilkit_strokes() {
      // PencilKit-generated: a red pen stroke split by
      // -[PKDrawing drawingByErasingStrokePath:mask:transform:], then a blue
      // marker stroke with an explicit mask. Expected values are what
      // PKDrawing(data:) reports for the same bytes.
      let drawing = decode_pk_drawing(include_bytes!("../fixtures/pk-masked.drawing")).unwrap();
      let outline = |path: &FreeformPath| {
         let commands = path.commands.iter().map(|command| match *command {
            FreeformPathCommand::Move { point } => {
               format!("M{},{}", point.x as f32, point.y as f32)
            },
            FreeformPathCommand::Line { point } => {
               format!("L{},{}", point.x as f32, point.y as f32)
            },
            FreeformPathCommand::Close => "Z".to_owned(),
            _ => unreachable!("masks are polygons"),
         });
         commands.collect::<Vec<_>>().join(" ")
      };
      let summary = drawing
         .strokes
         .iter()
         .map(|stroke| {
            (
               stroke.ink_type.as_str(),
               stroke.color.as_ref().and_then(|color| color.hex.clone()),
               stroke.random_seed,
               (stroke.transform.tx, stroke.transform.ty),
               stroke.visible_ranges.as_ref().map(|ranges| {
                  ranges
                     .iter()
                     .map(|range| (range.start as f32, range.end as f32))
                     .collect::<Vec<_>>()
               }),
               stroke.mask.as_ref().map(outline),
            )
         })
         .collect::<Vec<_>>();
      let red = Some("#ff0000".to_owned());
      assert_eq!(summary, [
         (
            "pen",
            red.clone(),
            Some(12345),
            (100.0, 50.0),
            Some(vec![(0.0, 3.7)]),
            Some("M37,2.2 L-2.2,2.2 L-2.2,-2.2 L37,-2.2 Z".to_owned())
         ),
         (
            "pen",
            red,
            Some(12345),
            (100.0, 50.0),
            Some(vec![(5.3, 9.0)]),
            Some("M92.2,2.2 L53,2.2 L53,-2.2 L92.2,-2.2 Z".to_owned())
         ),
         (
            "marker",
            Some("#0000ff".to_owned()),
            Some(777),
            (100.0, 120.0),
            None,
            Some("M55,10 L25,10 L25,-10 L55,-10 Z".to_owned())
         ),
      ]);
      // Both erased pieces keep their record's full path and bytes.
      assert_eq!(drawing.strokes[0].points.len(), 10);
      assert_eq!(drawing.strokes[0].points, drawing.strokes[1].points);
      assert_eq!(drawing.strokes[0].raw_data, drawing.strokes[1].raw_data);
      assert_eq!(
         drawing.bounds,
         Some(FreeformFrame {
            x:        97.0,
            y:        47.0,
            w:        96.0,
            h:        83.0,
            rotation: 0.0,
         })
      );
   }
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
      let stride = points.len() / usize::try_from(count.max(1)).unwrap();
      let mask = match stride {
         12 => 3,
         16 => 7,
         18 => 39,
         _ => 3,
      };
      path.extend(vint(4, mask));
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
      let drawing = decode_pk_drawing(FIXTURE).unwrap();
      assert_eq!(
         drawing.bounds,
         Some(FreeformFrame {
            x:        95.0,
            y:        45.0,
            w:        53.0,
            h:        53.0,
            rotation: 0.0,
         })
      );
      assert_eq!(drawing.strokes.len(), 2);
      assert_eq!((drawing.strokes[0].points.len(), drawing.strokes[1].points.len()), (4, 2));
      assert_eq!(drawing.strokes[0].random_seed, Some(413_047_802));
      assert_eq!(drawing.strokes[1].random_seed, Some(1_555_745_639));
      for stroke in &drawing.strokes {
         assert_eq!((stroke.transform.tx, stroke.transform.ty), (100.0, 50.0));
         for (index, point) in stroke.points.iter().enumerate() {
            assert_eq!(point.time_offset, Some(f64::from(index as f32 * 0.01)));
            assert_eq!(point.height, point.width);
            assert_eq!(point.opacity, Some(0.9999847409781033));
            assert_eq!(point.azimuth, Some(-4.7937631091787836e-05));
            assert_eq!(point.altitude, Some(std::f64::consts::FRAC_PI_2));
            assert_eq!(point.secondary_scale, Some(1.0));
            assert_eq!(point.threshold, Some(0.0));
         }
      }
   }

   #[test]
   fn native_varied_and_constant_channels_match_public_api() {
      // Generated by macOS PencilKit using PKStrokePoint's public initializer,
      // then dataRepresentation(). The second stroke stores only locations in
      // its point stream: all remaining channels are in the constant template.
      const CHANNELS: &[u8] = include_bytes!("../fixtures/pk-channels-varied.drawing");
      let drawing = decode_pk_drawing(CHANNELS).unwrap();
      assert_eq!(
         drawing.bounds,
         Some(FreeformFrame {
            x:        23.0,
            y:        46.0,
            w:        143.0,
            h:        87.0,
            rotation: 0.0,
         })
      );
      assert_eq!(drawing.strokes.len(), 2);
      let stroke = &drawing.strokes[0];
      assert_eq!(stroke.random_seed, Some(123_456_789));
      assert_eq!(stroke.transform, FreeformTransform {
         a:  1.25,
         b:  0.5,
         c:  -0.25,
         d:  0.75,
         tx: 100.0,
         ty: 50.0,
      });
      let expected = [
         [
            2.0,
            3.0,
            0.0,
            4.0,
            8.0,
            0.19998474097810331,
            0.25,
            0.29994575774031196,
            0.4000155626440838,
            0.699999988079071,
            0.19998474097810331,
         ],
         [
            12.0,
            23.0,
            0.125,
            10.0,
            6.0,
            0.5999847409781033,
            0.75,
            1.1999268438545219,
            0.9000050549297561,
            1.2,
            0.899977111467155,
         ],
         [
            32.0,
            -7.0,
            0.375,
            2.0,
            12.0,
            0.9999847409781033,
            1.25,
            -0.7832529544060978,
            std::f64::consts::FRAC_PI_2,
            0.30000001192092896,
            0.4,
         ],
         [
            20.0,
            9.0,
            1.25,
            16.0,
            2.992,
            0.04998855573357748,
            0.01,
            -0.700033226831033,
            0.10002186727268025,
            2.0,
            0.0,
         ],
      ];
      assert_eq!(stroke.points.len(), expected.len());
      for (point, expected) in stroke.points.iter().zip(expected) {
         let actual = [
            Some(point.x),
            Some(point.y),
            point.time_offset,
            point.width,
            point.height,
            point.opacity,
            point.force,
            point.azimuth,
            point.altitude,
            point.secondary_scale,
            point.threshold,
         ];
         for (actual, expected) in actual.into_iter().zip(expected) {
            assert!((actual.unwrap() - expected).abs() < 1e-12);
         }
      }
      let constant = &drawing.strokes[1];
      assert_eq!(constant.random_seed, Some(987_654_321));
      assert_eq!(constant.points.len(), 2);
      assert_eq!((constant.points[0].x, constant.points[0].y), (50.0, 20.0));
      assert_eq!((constant.points[1].x, constant.points[1].y), (70.0, 30.0));
      for point in &constant.points {
         assert_eq!(*point, FreeformInkPoint { x: point.x, y: point.y, ..stroke.points[0] });
      }
      let raw = proto::Drawing::parse(&CHANNELS[3..]).unwrap().strokes;
      let raw = raw.into_iter().collect::<Result<Vec<_>, _>>().unwrap();
      assert_eq!(
         drawing
            .strokes
            .iter()
            .map(|s| s.raw_data.as_slice())
            .collect::<Vec<_>>(),
         raw
      );
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
      assert_eq!(point.time_offset, Some(0.0));
      assert_eq!(point.width, None);
      assert_eq!(point.height, None);
      assert_eq!(point.opacity, None);
      assert_eq!(point.force, None);
      assert_eq!(point.azimuth, None);
      assert_eq!(point.altitude, None);
      assert_eq!(drawing.strokes[0].color, None);
      assert_eq!(drawing.strokes[0].random_seed, None);
      assert_eq!(drawing.required_content_version, None);
      assert_eq!(drawing.bounds, None);
   }
   #[test]
   fn rejects_unknown_overlapping_and_misaligned_channel_layouts() {
      let locations = [0u8; 8];
      let base = proto::Path {
         count:         Some(1),
         point_mask:    Some(1),
         constant_mask: None,
         constants:     None,
         points:        Some(&locations),
      };
      assert!(decode_points(&proto::Path { point_mask: Some(1 | (1 << 12)), ..base }).is_err());
      assert!(decode_points(&proto::Path { constant_mask: Some(1), ..base }).is_err());
      assert!(decode_points(&proto::Path { point_mask: Some(3), ..base }).is_err());
      assert!(decode_points(&proto::Path { constant_mask: Some(2), ..base }).is_err());
      assert!(decode_points(&proto::Path { points: Some(&locations[..7]), ..base }).is_err());
   }

   #[test]
   fn incomplete_render_bounds_do_not_become_partial_drawing_bounds() {
      let record = point(0.0, 0.0, None, None);
      let mut bounded = stroke(0, 1, &record, None);
      let bounds: Vec<_> = [1.0, 2.0, 3.0, 4.0]
         .into_iter()
         .enumerate()
         .flat_map(|(index, value)| fixed(index as u64 + 1, value))
         .collect();
      bounded.extend(bytes(6, &bounds));
      let result = decode_pk_drawing(&drawing(&[ink("com.apple.ink.pen")], &[
         bounded,
         stroke(0, 1, &record, None),
      ]))
      .unwrap();
      assert_eq!(result.bounds, None);
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
