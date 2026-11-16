//! Native freehand-shape ink: shape affinity supplies coordinates, while
//! `CRLProto_PKStrokePathData` supplies delta-compressed `PencilKit` channels.

use super::{
   coherence::{Node, Value},
   finite, float32, invalid, shape,
};
use crate::{
   pkdrawing,
   types::{
      FreeformDecodeError, FreeformInkPoint, FreeformInkPointRole, FreeformInkStroke, FreeformPath,
      FreeformPathCommand, FreeformPoint, FreeformTransform,
   },
   wire::Repeated,
};

type Result<T, E = FreeformDecodeError> = std::result::Result<T, E>;

/// `CRLProto` stroke schemas.
mod proto {
   use crate::wire::{self, Repeated};

   wire::message! {
      /// `CRLProto_PKStrokePathData`: delta-compressed `PencilKit` channels.
      pub struct StrokePathData<'a> {
         points: Repeated<'a, StrokePoint<'a>> = 1,
         /// Inline `CRLProto_Path` stroke mask.
         mask: Option<&'a [u8]> = 2,
         random_seed: Option<u64> = 3,
         x_scale: Option<f32> = 5,
         y_scale: Option<f32> = 6,
      }
   }

   wire::message! {
      /// One point's channels as little-endian bytes; omitted or empty bytes
      /// repeat the preceding point's value.
      pub struct StrokePoint<'a> {
         /// `uint16`.
         altitude: Option<&'a [u8]> = 1,
         /// `uint16`.
         azimuth: Option<&'a [u8]> = 2,
         /// `uint16`.
         force: Option<&'a [u8]> = 3,
         /// `uint16`.
         opacity: Option<&'a [u8]> = 4,
         /// Two `float32` values: width, height.
         size: Option<&'a [u8]> = 5,
         /// `float32`.
         time_offset: Option<&'a [u8]> = 6,
         /// `float32`.
         secondary_scale: Option<&'a [u8]> = 7,
         /// `uint16`.
         edge_width: Option<&'a [u8]> = 8,
         /// `uint16`.
         threshold: Option<&'a [u8]> = 9,
      }
   }
}

fn optional_field(value: Value<'_>, key: u64) -> Result<Option<Value<'_>>> {
   value
      .field(key)?
      .map(Value::optional)
      .transpose()
      .map(Option::flatten)
}

/// Freehand drawing containers contain child objects, not embedded stroke
/// messages. Their child freehand-shape objects use common.b key 1 for the
/// path, key 2 for compact channels, and specific.a.a.b for the stroke style.
pub fn decode_native_ink(common: Node<'_>, specific: Node<'_>) -> Result<Vec<FreeformInkStroke>> {
   let Some(affinity) = common.property("b")? else {
      return Ok(Vec::new());
   };
   let packed = optional_field(affinity, 2)?
      .map(Value::data)
      .transpose()?
      .flatten();
   if let Some(bytes) = packed.filter(|bytes| pkdrawing::is_pk_drawing(bytes)) {
      return Ok(pkdrawing::decode_pk_drawing(bytes)?.strokes);
   }
   let Some(style) = specific.at(&["a", "a"])? else {
      return Ok(Vec::new());
   };
   let Some(stroke) = style.property("b")? else {
      return Ok(Vec::new());
   };
   let Some(ink) = optional_field(stroke, 8)? else {
      return Ok(Vec::new());
   };
   let Some(identifier) = ink.field(0)?.map(Value::string).transpose()?.flatten() else {
      return Ok(Vec::new());
   };
   let Some(path) = optional_field(affinity, 1)?
      .map(shape::path)
      .transpose()?
      .flatten()
   else {
      return Ok(Vec::new());
   };
   let compact = packed.map(proto::StrokePathData::parse).transpose()?;
   // Native converter 0x1003a5690 inverts these scales before reconstructing
   // control points, and installs the scales as the PKStroke transform.
   let sx = float32(compact.and_then(|compact| compact.x_scale))?.unwrap_or(1.0);
   let sy = float32(compact.and_then(|compact| compact.y_scale))?.unwrap_or(1.0);
   if sx == 0.0 || sy == 0.0 {
      return Err(invalid("zero native ink render scale"));
   }
   let mut points = spline_controls(&path, sx, sy)?;
   if let Some(compact) = compact {
      apply_channels(compact.points, &mut points)?;
   }
   let fountain_v2 = optional_field(ink, 2)?
      .map(Value::boolean)
      .transpose()?
      .flatten()
      .unwrap_or(false);
   if identifier == "com.apple.ink.fountainPen" && !fountain_v2 {
      for point in &mut points {
         point.azimuth = Some(0.0);
      }
   }
   let mut mask = compact
      .and_then(|compact| compact.mask)
      .map(shape::bezier_bytes)
      .transpose()?;
   if let Some(mask) = &mut mask {
      for command in &mut mask.commands {
         let scale = |point: &mut FreeformPoint| -> Result<()> {
            point.x = finite(point.x / sx)?;
            point.y = finite(point.y / sy)?;
            Ok(())
         };
         match command {
            FreeformPathCommand::Move { point } | FreeformPathCommand::Line { point } => {
               scale(point)?;
            },
            FreeformPathCommand::Quadratic { control, point } => {
               scale(control)?;
               scale(point)?;
            },
            FreeformPathCommand::Cubic { control_1, control_2, point } => {
               scale(control_1)?;
               scale(control_2)?;
               scale(point)?;
            },
            FreeformPathCommand::Close => {},
         }
      }
   }
   let color = optional_field(stroke, 1)?
      .map(shape::color)
      .transpose()?
      .flatten();
   let random_seed = compact
      .and_then(|compact| compact.random_seed)
      .map(|seed| u32::try_from(seed).map_err(|_| invalid("native ink seed exceeds u32")))
      .transpose()?;
   Ok(vec![FreeformInkStroke {
      ink_type: pkdrawing::ink_type_from_identifier(identifier).to_owned(),
      ink_identifier: identifier.to_owned(),
      color,
      transform: FreeformTransform { a: sx, d: sy, ..FreeformTransform::default() },
      point_role: FreeformInkPointRole::SplineControl,
      points,
      visible_ranges: None,
      mask,
      random_seed,
      // Includes the native mask and unprojected/new channels. Never
      // reinterpret those fields as points or manufacture a transform from
      // arbitrary bytes.
      raw_data: packed.unwrap_or(specific.bytes).to_vec(),
   }])
}

// TSUPointsAlmostEqual uses independent strict 0.1-point bounds on x/y.
// The native converter tests after undoing the render scale.
fn almost_equal(a: FreeformPoint, b: FreeformPoint, sx: f64, sy: f64) -> bool {
   ((a.x - b.x) / sx).abs() < 0.1 && ((a.y - b.y) / sy).abs() < 0.1
}

fn control(point: FreeformPoint, sx: f64, sy: f64) -> Result<FreeformInkPoint> {
   Ok(FreeformInkPoint {
      x: finite(point.x / sx)?,
      y: finite(point.y / sy)?,
      ..FreeformInkPoint::default()
   })
}

/// The native inverse at 0x10000ef84 discards the first and final cap control
/// points: first cubic contributes B-spline controls 1..3, subsequent non-final
/// cubics contribute control 3. Lines contribute duplicated start/end pairs.
fn spline_controls(path: &FreeformPath, sx: f64, sy: f64) -> Result<Vec<FreeformInkPoint>> {
   let mut points = Vec::new();
   let mut previous = None;
   let mut start = None;
   for (index, command) in path.commands.iter().enumerate() {
      match *command {
         FreeformPathCommand::Move { point } => {
            previous = Some(point);
            start = Some(point);
         },
         FreeformPathCommand::Line { point } => {
            let from = previous.ok_or_else(|| invalid("native ink path missing move"))?;
            if path.commands.len() == 2 && almost_equal(from, point, sx, sy) {
               points.push(control(from, sx, sy)?);
               points.push(control(point, sx, sy)?);
            } else {
               for value in [from, from, point, point] {
                  points.push(control(value, sx, sy)?);
               }
            }
            previous = Some(point);
         },
         FreeformPathCommand::Cubic { control_1: c1, control_2: c2, point } => {
            let from = previous.ok_or_else(|| invalid("native ink path missing move"))?;
            if path.commands.len() == 2
               && [c1, c2, point]
                  .iter()
                  .all(|&p| almost_equal(from, p, sx, sy))
            {
               points.push(control(from, sx, sy)?);
               points.push(control(c1, sx, sy)?);
            } else {
               let last = FreeformPoint {
                  x: 6.0 * point.x + 2.0 * c1.x - 7.0 * c2.x,
                  y: 6.0 * point.y + 2.0 * c1.y - 7.0 * c2.y,
               };
               if index == 1 {
                  for value in [
                     FreeformPoint { x: 2.0 * c1.x - c2.x, y: 2.0 * c1.y - c2.y },
                     FreeformPoint { x: 2.0 * c2.x - c1.x, y: 2.0 * c2.y - c1.y },
                     last,
                  ] {
                     points.push(control(value, sx, sy)?);
                  }
               } else if index + 1 < path.commands.len() {
                  points.push(control(last, sx, sy)?);
               }
            }
            previous = Some(point);
         },
         FreeformPathCommand::Close => {
            if let (Some(from), Some(to)) = (previous, start) {
               if from != to {
                  for value in [from, from, to, to] {
                     points.push(control(value, sx, sy)?);
                  }
               }
               previous = Some(to);
            }
         },
         FreeformPathCommand::Quadratic { .. } => {
            return Err(FreeformDecodeError::unsupported("native ink quadratic path"));
         },
      }
   }
   Ok(points)
}

/// Generated protobuf decoder 0x10079a65c: every `StrokePoint` channel is
/// bytes, not a protobuf scalar. Encoder 0x1008ef76c omits unchanged channels;
/// omitted or empty bytes reuse the preceding value, independently for each
/// channel.
fn apply_channels(
   channels: Repeated<'_, proto::StrokePoint<'_>>,
   points: &mut Vec<FreeformInkPoint>,
) -> Result<()> {
   // Native init(from:) 0x1008ed904 initializes predecessor channels to zero.
   // These are established defaults, not values inferred from unknown fields.
   let mut previous = FreeformInkPoint {
      time_offset: Some(0.0),
      width: Some(0.0),
      height: Some(0.0),
      opacity: Some(0.0),
      force: Some(0.0),
      azimuth: Some(0.0),
      altitude: Some(0.0),
      secondary_scale: Some(0.0),
      threshold: Some(0.0),
      edge_width: Some(0.0),
      ..FreeformInkPoint::default()
   };
   let mut index = 0;
   for channel in channels {
      let channel = channel?;
      update_u16(channel.altitude, 41720.876782109444, &mut previous.altitude)?;
      update_u16(channel.azimuth, 10430.219195527361, &mut previous.azimuth)?;
      update_u16(channel.force, 1000.0, &mut previous.force)?;
      update_u16(channel.opacity, 32767.5, &mut previous.opacity)?;
      if let Some(size) = channel.size.filter(|bytes| !bytes.is_empty()) {
         let raw = size
            .as_array::<8>()
            .ok_or_else(|| invalid("native ink size is not two float32 values"))?;
         previous.width = Some(float(&raw[..4])? / 1000.0);
         previous.height = Some(float(&raw[4..])? / 1000.0);
      }
      update_float(channel.time_offset, 1.0, &mut previous.time_offset)?;
      update_float(channel.secondary_scale, 1000.0, &mut previous.secondary_scale)?;
      update_u16(channel.edge_width, 1000.0, &mut previous.edge_width)?;
      update_u16(channel.threshold, 32767.5, &mut previous.threshold)?;
      if let Some(point) = points.get_mut(index) {
         let (x, y) = (point.x, point.y);
         *point = previous;
         point.x = x;
         point.y = y;
      }
      index += 1;
   }
   // CRLPKStrokePathConverter uses min(pointCount, reconstructedControlCount).
   points.truncate(index);
   Ok(())
}

fn update_u16(channel: Option<&[u8]>, scale: f64, target: &mut Option<f64>) -> Result<()> {
   if let Some(bytes) = channel.filter(|bytes| !bytes.is_empty()) {
      let raw = bytes
         .try_into()
         .map_err(|_| invalid("native ink channel is not uint16"))?;
      *target = Some(f64::from(u16::from_le_bytes(raw)) / scale);
   }
   Ok(())
}

fn float(bytes: &[u8]) -> Result<f64> {
   let raw = bytes
      .as_array::<4>()
      .ok_or_else(|| invalid("native ink channel is not float32"))?;
   finite(f64::from(f32::from_le_bytes(*raw)))
}

fn update_float(channel: Option<&[u8]>, scale: f64, target: &mut Option<f64>) -> Result<()> {
   if let Some(bytes) = channel.filter(|bytes| !bytes.is_empty()) {
      *target = Some(float(bytes)? / scale);
   }
   Ok(())
}

#[cfg(test)]
mod tests {
   use super::*;

   #[test]
   fn native_spline_inverse_and_degenerate_tolerance() {
      // Direct TSUtility native probe: TSUBezierToBSpline for these four
      // points returned (-5,-4),(1,2),(7,8),(13,14). The Freeform consumer
      // omits the first cap control. TSUPointsAlmostEqual uses strict 0.1.
      let path = FreeformPath {
         commands: vec![
            FreeformPathCommand::Move { point: FreeformPoint { x: 1.0, y: 2.0 } },
            FreeformPathCommand::Cubic {
               control_1: FreeformPoint { x: 3.0, y: 4.0 },
               control_2: FreeformPoint { x: 5.0, y: 6.0 },
               point:     FreeformPoint { x: 7.0, y: 8.0 },
            },
         ],
         ..FreeformPath::default()
      };
      let points = spline_controls(&path, 1.0, 1.0).unwrap();
      assert_eq!(points.iter().map(|p| (p.x, p.y)).collect::<Vec<_>>(), [
         (1.0, 2.0),
         (7.0, 8.0),
         (13.0, 14.0)
      ]);
      let zero = FreeformPoint { x: 0.0, y: 0.0 };
      assert!(almost_equal(zero, FreeformPoint { x: 0.09, y: 0.09 }, 1.0, 1.0));
      assert!(!almost_equal(zero, FreeformPoint { x: 0.1, y: 0.0 }, 1.0, 1.0));
   }

   #[test]
   fn native_delta_channels_repeat_without_reinterpreting_unknown_fields() {
      // CRLPKStrokePathData.encode(to:) 0x1008ef76c writes little-endian
      // channel bytes and emits an empty StrokePoint for unchanged values.
      // Quantizers are from CRLPKStrokePathConverter's table 0x10154b6f0.
      let data = [
         0x0a, 0x32, 0x0a, 2, 0xff, 0xff, 0x12, 2, 0, 0x80, 0x1a, 2, 0xe8, 3, 0x22, 2, 0xff, 0xff,
         0x2a, 8, 0, 0x40, 0x1c, 0x45, 0, 0x80, 0x3b, 0x45, 0x32, 4, 0, 0, 0xc0, 0x3f, 0x3a, 4, 0,
         0, 0xfa, 0x44, 0x42, 2, 0xe8, 3, 0x4a, 2, 0, 0x80, 0x52, 2, 0xaa,
         0xbb, // Unknown channel, not another point.
         0x0a, 0,
      ];
      let mut points = vec![
         FreeformInkPoint { x: 4.0, y: 7.0, ..FreeformInkPoint::default() },
         FreeformInkPoint { x: 5.0, y: 8.0, ..FreeformInkPoint::default() },
      ];
      let channels = proto::StrokePathData::parse(&data).unwrap().points;
      apply_channels(channels, &mut points).unwrap();
      assert_eq!(points.len(), 2);
      for point in &points {
         assert_eq!(point.force, Some(1.0));
         assert_eq!(point.opacity, Some(2.0));
         assert_eq!(point.width, Some(2.5));
         assert_eq!(point.height, Some(3.0));
         assert_eq!(point.time_offset, Some(1.5));
         assert_eq!(point.secondary_scale, Some(2.0));
         assert_eq!(point.edge_width, Some(1.0));
         assert!((point.altitude.unwrap() - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
      }
      assert_eq!((points[1].x, points[1].y), (5.0, 8.0));
   }
}
