//! Native shape schemas, shared by shape, text, table, and media decoders.
//!
//! The field paths below are the installed application's Codable coding keys,
//! not searches through arbitrary nested protobuf messages. `CRLFillData`
//! currently has only the `color` alternative; gradients are not part of this
//! schema.

use super::{
   coherence::{Node, Value},
   finite, float32, invalid,
};
use crate::types::{
   FreeformColor, FreeformColorComponents, FreeformDecodeError, FreeformFrame, FreeformGeometry,
   FreeformPaint, FreeformPath, FreeformPathCommand, FreeformPathSource,
   FreeformPencilKitStrokeStyle, FreeformPoint, FreeformShadow, FreeformSize, FreeformStrokeStyle,
   FreeformStyle,
};

type Result<T, E = FreeformDecodeError> = std::result::Result<T, E>;

/// `CRLProto` geometry schemas.
mod proto {
   use crate::wire::{self, Repeated};

   wire::message! {
      /// `CRLProto_Point`; protobuf scalar defaults are significant, so an
      /// empty message is (0, 0).
      pub struct Point {
         x: Option<f32> = 1,
         y: Option<f32> = 2,
      }
   }

   wire::message! {
      /// `CRLProto_Path`.
      pub struct Path<'a> {
         elements: Repeated<'a, PathElement<'a>> = 1,
      }
   }

   wire::message! {
      /// One path command; points are packed float pairs.
      pub struct PathElement<'a> {
         /// 0 move, 1 line, 2 cubic, 3 close.
         kind: Option<u64> = 1,
         end_point: Option<&'a [u8]> = 2,
         /// Offset from the end point.
         control_point_1: Option<&'a [u8]> = 3,
         /// Offset from the end point.
         control_point_2: Option<&'a [u8]> = 4,
      }
   }
}

pub(super) struct DecodedShape {
   pub preset:   Option<String>,
   pub path:     Option<FreeformPath>,
   pub geometry: FreeformGeometry,
   pub style:    FreeformStyle,
}

/// Common.b is `CRLShapeItemParentAffinity` for shape-derived persistence
/// types, and `CRLBoardItemParentAffinity` for other board items.
/// `ShapeStyleData` is the shape superclass's `a` composite, underneath each
/// subclass's `a` superData.
pub(super) fn decode_shape(
   common: Node<'_>,
   specific: Option<Node<'_>>,
   persistence_type: u32,
) -> Result<DecodedShape> {
   let shape_item = matches!(persistence_type, 3 | 4 | 11 | 12);
   let affinity = common.property("b")?;
   let general = if shape_item {
      affinity.map(|value| value.field(0)).transpose()?.flatten()
   } else {
      affinity
   };
   let geometry = match general.map(|value| value.field(1)).transpose()?.flatten() {
      Some(value) => geometry(value)?,
      None => FreeformGeometry::default(),
   };
   let source = if shape_item {
      affinity.map(|value| value.field(1)).transpose()?.flatten()
   } else {
      None
   };
   let preset = source.map(|value| string(value, 2)).transpose()?.flatten();
   let path = source.map(path).transpose()?.flatten();
   let mut style = FreeformStyle::default();
   if let Some(value) = common.property("f")? {
      style.shadows.push(shadow(value)?);
   }
   let style_node = match (specific, persistence_type) {
      (Some(node), 12) => node.at(&["a", "a", "a"])?,
      (Some(node), 3 | 4 | 11) => node.at(&["a", "a"])?,
      _ => None,
   };
   if let Some(node) = style_node {
      style.fill = node.property("a")?.map(paint).transpose()?.flatten();
      style.stroke = node.property("b")?.map(stroke).transpose()?.flatten();
      if let Some(stroke) = &mut style.stroke {
         stroke.head_end = node
            .property("c")?
            .map(|value| string(value, 4))
            .transpose()?
            .flatten();
         stroke.tail_end = node
            .property("d")?
            .map(|value| string(value, 4))
            .transpose()?
            .flatten();
         if let Some(offset) = node.property("e")? {
            stroke.pattern_offset_distance = offset.number()?;
         }
      }
   }
   Ok(DecodedShape { preset, path, geometry, style })
}

fn field(value: Value<'_>, key: u64) -> Result<Option<Value<'_>>> {
   value
      .field(key)?
      .map(Value::optional)
      .transpose()
      .map(Option::flatten)
}

fn required(value: Value<'_>, key: u64) -> Result<Value<'_>> {
   field(value, key)?.ok_or_else(|| invalid("missing native shape coding key"))
}

fn number(value: Value<'_>, key: u64) -> Result<Option<f64>> {
   field(value, key)?
      .map(Value::number)
      .transpose()
      .map(Option::flatten)
}

fn scalar(value: Value<'_>, key: u64) -> Result<f64> {
   number(value, key)?.ok_or_else(|| invalid("missing native shape number"))
}

fn integer(value: Value<'_>, key: u64) -> Result<i64> {
   required(value, key)?
      .integer()?
      .ok_or_else(|| invalid("missing native shape enum"))
}

fn boolean(value: Value<'_>, key: u64) -> Result<Option<bool>> {
   field(value, key)?
      .map(Value::boolean)
      .transpose()
      .map(Option::flatten)
}

fn string(value: Value<'_>, key: u64) -> Result<Option<String>> {
   Ok(field(value, key)?
      .map(Value::string)
      .transpose()?
      .flatten()
      .map(str::to_owned))
}

/// `CRLPointData` and `CRLSizeData` encode as opaque `CRLProto_Point` data.
/// Protobuf scalar defaults are significant: an empty data value is the point
/// (0, 0).
pub(super) fn point(value: Value<'_>) -> Result<Option<FreeformPoint>> {
   let Some(bytes) = value.data()? else {
      return Ok(None);
   };
   let point = proto::Point::parse(bytes)?;
   Ok(Some(FreeformPoint {
      x: float32(point.x)?.unwrap_or(0.0),
      y: float32(point.y)?.unwrap_or(0.0),
   }))
}

fn required_point(value: Value<'_>, key: u64) -> Result<FreeformPoint> {
   point(required(value, key)?)?.ok_or_else(|| invalid("missing native shape point data"))
}

fn size(value: Value<'_>, key: u64) -> Result<FreeformSize> {
   let point = required_point(value, key)?;
   Ok(FreeformSize { width: point.x, height: point.y })
}

/// `CRLCanvasInfoGeometryData` encoder 0x101259338; the canvas consumer
/// 0x1000397c0 changes clockwise archived degrees to counterclockwise radians.
pub(super) fn geometry(value: Value<'_>) -> Result<FreeformGeometry> {
   let position = required_point(value, 0)?;
   let size = required_point(value, 1)?;
   Ok(FreeformGeometry {
      frame: Some(FreeformFrame {
         x:        position.x,
         y:        position.y,
         w:        size.x,
         h:        size.y,
         rotation: -scalar(value, 2)?.to_radians(),
      }),
      width_valid: boolean(value, 3)?,
      height_valid: boolean(value, 4)?,
      horizontal_flip: boolean(value, 5)?,
      vertical_flip: boolean(value, 6)?,
      ..FreeformGeometry::default()
   })
}

/// `TSUColorData` encoder 0x100bdec44 and consumer 0x100bdcbb0. Alpha is
/// optional key 0 (native default 1), not a fourth RGB component. Device CMYK
/// and gray stay in their source model rather than pretending to be sRGB.
pub(super) fn color(value: Value<'_>) -> Result<Option<FreeformColor>> {
   let Some(value) = value.optional()? else {
      return Ok(None);
   };
   let alpha = number(value, 0)?.unwrap_or(1.0);
   let rgb = field(value, 1)?;
   let cmyk = field(value, 2)?;
   let monochrome = field(value, 3)?;
   if usize::from(rgb.is_some()) + usize::from(cmyk.is_some()) + usize::from(monochrome.is_some())
      != 1
   {
      return Err(invalid("color must contain exactly one color model"));
   }
   let (color_space, components, hex) = if let Some(rgb) = rgb {
      let red = scalar(rgb, 1)?;
      let green = scalar(rgb, 2)?;
      let blue = scalar(rgb, 3)?;
      let space = integer(rgb, 0)?;
      let (name, hex) = match space {
         0 => ("sRGB", Some(css_hex(red, green, blue, alpha))),
         1 => ("Display P3", None),
         _ => {
            return Err(FreeformDecodeError::unsupported(format!(
               "native RGB color space {space}"
            )));
         },
      };
      (name, FreeformColorComponents::Rgb { red, green, blue }, hex)
   } else if let Some(cmyk) = cmyk {
      (
         "Device CMYK",
         FreeformColorComponents::Cmyk {
            cyan:    scalar(cmyk, 0)?,
            magenta: scalar(cmyk, 1)?,
            yellow:  scalar(cmyk, 2)?,
            black:   scalar(cmyk, 3)?,
         },
         None,
      )
   } else {
      let monochrome = monochrome.expect("exactly one native color model");
      ("Device Gray", FreeformColorComponents::Monochrome { white: scalar(monochrome, 0)? }, None)
   };
   Ok(Some(FreeformColor { color_space: color_space.into(), components, alpha, hex }))
}

fn css_hex(red: f64, green: f64, blue: f64, alpha: f64) -> String {
   let channel = |value: f64| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
   if alpha == 1.0 {
      format!("#{:02X}{:02X}{:02X}", channel(red), channel(green), channel(blue))
   } else {
      format!(
         "#{:02X}{:02X}{:02X}{:02X}",
         channel(red),
         channel(green),
         channel(blue),
         channel(alpha)
      )
   }
}

/// `CRLFillData` encoder 0x100ceb448: type 0 is color, key 1 is `TSUColorData`.
/// Unknown extensible alternatives retain exactly their own `CRValue` bytes.
pub(super) fn paint(value: Value<'_>) -> Result<Option<FreeformPaint>> {
   let Some(value) = value.optional()? else {
      return Ok(None);
   };
   if integer(value, 0)? != 0 {
      return Ok(Some(FreeformPaint::Unknown { raw_data: value.bytes.to_vec() }));
   }
   Ok(color(required(value, 1)?)?.map(|color| FreeformPaint::Solid { color }))
}

/// `CRLStrokeData` encoder 0x1012f0c5c. Pattern is an optional custom value,
/// not a scan for adjacent floats. Empty is a real enum case, not width zero.
pub(super) fn stroke(value: Value<'_>) -> Result<Option<FreeformStrokeStyle>> {
   let Some(value) = value.optional()? else {
      return Ok(None);
   };
   match integer(value, 0)? {
      0 | 1 => {},
      2 => return Ok(None),
      kind => return Err(FreeformDecodeError::unsupported(format!("native stroke type {kind}"))),
   }
   let color = color(required(value, 1)?)?.ok_or_else(|| invalid("stroke has no color"))?;
   let mut dash = Vec::new();
   let dash_offset = if let Some(pattern) = field(value, 6)? {
      for value in required(pattern, 0)?.elements()? {
         dash.push(
            value?
               .number()?
               .ok_or_else(|| invalid("stroke dash is not numeric"))?,
         );
      }
      number(pattern, 1)?
   } else {
      None
   };
   let smart_stroke = field(value, 7)?
      .map(|source| {
         enum_name(
            integer(source, 0)?,
            &["chalk", "dryBrush", "featheredBrush", "pen", "pencil", "crayon", "calligraphy"],
            "smart stroke",
         )
      })
      .transpose()?;
   let pencil_kit = field(value, 8)?
      .map(|source| -> Result<_> {
         Ok(FreeformPencilKitStrokeStyle {
            ink_type:        string(source, 0)?
               .ok_or_else(|| invalid("missing PencilKit ink identifier"))?,
            width:           scalar(source, 1)?,
            fountain_pen_v2: boolean(source, 2)?,
         })
      })
      .transpose()?;
   Ok(Some(FreeformStrokeStyle {
      paint: FreeformPaint::Solid { color },
      width: scalar(value, 2)?,
      dash,
      dash_offset,
      cap: Some(enum_name(integer(value, 3)?, &["butt", "round", "square"], "stroke cap")?),
      join: Some(enum_name(integer(value, 4)?, &["miter", "round", "bevel"], "stroke join")?),
      miter_limit: number(value, 5)?,
      head_end: None,
      tail_end: None,
      pattern_offset_distance: None,
      smart_stroke,
      pencil_kit,
   }))
}

fn enum_name(value: i64, names: &[&str], kind: &str) -> Result<String> {
   usize::try_from(value)
      .ok()
      .and_then(|index| names.get(index))
      .map(|name| (*name).to_owned())
      .ok_or_else(|| FreeformDecodeError::unsupported(format!("native {kind} {value}")))
}

/// Shadow offsetDelta (0x100410fec) rotates (offset,0) by positive degrees.
fn shadow(value: Value<'_>) -> Result<FreeformShadow> {
   let angle = scalar(value, 1)?;
   let distance = scalar(value, 2)?;
   let (sin, cos) = angle.to_radians().sin_cos();
   let curve = field(value, 5)?.map(|value| scalar(value, 0)).transpose()?;
   Ok(FreeformShadow {
      kind: if curve.is_some() { "curved" } else { "drop" }.into(),
      color: color(required(value, 0)?)?.ok_or_else(|| invalid("shadow has no color"))?,
      offset_x: distance * cos,
      offset_y: distance * sin,
      radius: scalar(value, 3)?,
      opacity: number(value, 4)?,
      angle: Some(angle),
      curve,
   })
}

/// `CRLPathSourceData` coding keys (descriptor 0x10179f34c). Parametric sources
/// remain parameters; we never substitute approximate polygons or Bézier arcs.
pub(super) fn path(value: Value<'_>) -> Result<Option<FreeformPath>> {
   let Some(value) = value.optional()? else {
      return Ok(None);
   };
   let mut result = FreeformPath::default();
   let mut alternatives = 0;
   for key in 4..=9 {
      let Some(source) = field(value, key)? else {
         continue;
      };
      alternatives += 1;
      match key {
         4 => {
            result.natural_size = Some(size(source, 2)?);
            result.source = Some(FreeformPathSource::Point {
               name:  enum_name(
                  integer(source, 0)?,
                  &["leftSingleArrow", "rightSingleArrow", "doubleArrow", "star", "plus"],
                  "point path",
               )?,
               point: required_point(source, 1)?,
            });
         },
         5 => {
            result.natural_size = Some(size(source, 2)?);
            result.source = Some(FreeformPathSource::Scalar {
               name:             enum_name(
                  integer(source, 0)?,
                  &["roundedRectangle", "regularPolygon", "chevron"],
                  "scalar path",
               )?,
               scalar:           scalar(source, 1)?,
               curve_continuous: boolean(source, 3)?
                  .ok_or_else(|| invalid("missing path curve continuity"))?,
               show_knob:        boolean(source, 4)?
                  .ok_or_else(|| invalid("missing path knob state"))?,
            });
         },
         6 => decode_bezier_source(source, &mut result)?,
         7 => {
            result.natural_size = Some(size(source, 4)?);
            result.source = Some(FreeformPathSource::Callout {
               tail_position: required_point(source, 0)?,
               tail_size:     scalar(source, 1)?,
               corner_radius: scalar(source, 2)?,
               center_tail:   boolean(source, 3)?
                  .ok_or_else(|| invalid("missing callout center state"))?,
            });
         },
         8 => decode_bezier_source(required(source, 1)?, &mut result)?,
         9 => decode_editable(source, &mut result)?,
         _ => unreachable!(),
      }
   }
   if alternatives > 1 {
      return Err(invalid("multiple native path source alternatives"));
   }
   if alternatives == 0 {
      return Ok(None);
   }
   result.horizontal_flip = boolean(value, 0)?;
   result.vertical_flip = boolean(value, 1)?;
   result.raw_data = value.bytes.to_vec();
   Ok(Some(result))
}

fn decode_bezier_source(value: Value<'_>, result: &mut FreeformPath) -> Result<()> {
   let bytes = required(value, 0)?
      .data()?
      .ok_or_else(|| invalid("missing bezier path data"))?;
   decode_bezier(bytes, &mut result.commands)?;
   result.natural_size = Some(size(value, 1)?);
   Ok(())
}

/// Decodes an inline `CRLProto_Path`, including `PencilKit` stroke mask paths.
pub(super) fn bezier_bytes(bytes: &[u8]) -> Result<FreeformPath> {
   let mut result = FreeformPath::default();
   decode_bezier(bytes, &mut result.commands)?;
   result.raw_data = bytes.to_vec();
   Ok(result)
}

/// Native decoder 0x100c8e428 adds both control offsets to the END point, not
/// to the preceding point.
fn decode_bezier(bytes: &[u8], commands: &mut Vec<FreeformPathCommand>) -> Result<()> {
   for element in proto::Path::parse(bytes)?.elements {
      let element = element?;
      let end_point = || packed_point(element.end_point.unwrap_or_default(), false);
      let kind = element.kind.unwrap_or(0);
      let command = match kind {
         0 => FreeformPathCommand::Move { point: end_point()? },
         1 => FreeformPathCommand::Line { point: end_point()? },
         2 => {
            let point = end_point()?;
            let first = packed_point(element.control_point_1.unwrap_or_default(), true)?;
            let second = packed_point(element.control_point_2.unwrap_or_default(), true)?;
            FreeformPathCommand::Cubic {
               point,
               control_1: FreeformPoint {
                  x: finite(point.x + first.x)?,
                  y: finite(point.y + first.y)?,
               },
               control_2: FreeformPoint {
                  x: finite(point.x + second.x)?,
                  y: finite(point.y + second.y)?,
               },
            }
         },
         3 => FreeformPathCommand::Close,
         _ => {
            return Err(FreeformDecodeError::unsupported(format!("native bezier element {kind}")));
         },
      };
      commands.push(command);
   }
   Ok(())
}

/// Empty points are native zero points. Endpoints require Float32 pairs;
/// relative controls additionally accept Float16 pairs (decoder 0x100c8dd8c).
fn packed_point(bytes: &[u8], control: bool) -> Result<FreeformPoint> {
   let (x, y) = match bytes.len() {
      0 => (0.0, 0.0),
      8 => (
         f64::from(f32::from_le_bytes(bytes[..4].try_into().expect("four bytes"))),
         f64::from(f32::from_le_bytes(bytes[4..].try_into().expect("four bytes"))),
      ),
      4 if control => (
         half(u16::from_le_bytes(bytes[..2].try_into().expect("two bytes"))),
         half(u16::from_le_bytes(bytes[2..].try_into().expect("two bytes"))),
      ),
      _ => return Err(invalid("invalid native bezier point byte length")),
   };
   Ok(FreeformPoint { x: finite(x)?, y: finite(y)? })
}

fn half(bits: u16) -> f64 {
   let sign = if bits & 0x8000 == 0 { 1.0 } else { -1.0 };
   let fraction = f64::from(bits & 0x3ff);
   let exponent = (bits >> 10) & 0x1f;
   sign
      * match exponent {
         0 => fraction * 2.0_f64.powi(-24),
         31 if fraction == 0.0 => f64::INFINITY,
         31 => f64::NAN,
         _ => (1024.0 + fraction) * 2.0_f64.powi(i32::from(exponent) - 25),
      }
}

#[derive(Clone, Copy)]
struct EditableNode {
   point:    FreeformPoint,
   incoming: FreeformPoint,
   outgoing: FreeformPoint,
}

fn editable_node(value: Value<'_>) -> Result<EditableNode> {
   Ok(EditableNode {
      incoming: required_point(value, 0)?,
      point:    required_point(value, 1)?,
      outgoing: required_point(value, 2)?,
   })
}

fn editable_edge(from: EditableNode, to: EditableNode) -> FreeformPathCommand {
   // Exact equality preserves every archived control point. Near-equal native
   // controls may draw as a line, but their actual cubic remains lossless here.
   if from.point == from.outgoing && to.incoming == to.point {
      FreeformPathCommand::Line { point: to.point }
   } else {
      FreeformPathCommand::Cubic {
         control_1: from.outgoing,
         control_2: to.incoming,
         point:     to.point,
      }
   }
}

fn decode_editable(value: Value<'_>, result: &mut FreeformPath) -> Result<()> {
   result.natural_size = Some(size(value, 2)?);
   for subpath in required(value, 1)?.elements()? {
      let subpath = subpath?;
      let mut nodes = required(subpath, 0)?.elements()?;
      let Some(first) = nodes.next() else {
         continue;
      };
      let first = editable_node(first?)?;
      result
         .commands
         .push(FreeformPathCommand::Move { point: first.point });
      let mut previous = first;
      for node in nodes {
         let node = editable_node(node?)?;
         result.commands.push(editable_edge(previous, node));
         previous = node;
      }
      if boolean(subpath, 1)?.ok_or_else(|| invalid("missing subpath closed state"))? {
         result.commands.push(editable_edge(previous, first));
         result.commands.push(FreeformPathCommand::Close);
      }
   }
   Ok(())
}

#[cfg(test)]
mod tests {
   use super::{
      super::{coherence::Document, envelope::parse_native_archive},
      *,
   };

   const REAL: &[u8] = include_bytes!("../../fixtures/real-board.crlnative");

   #[test]
   fn native_shapes_decode_real_geometry_fill_shadow_and_paths() {
      let archive = parse_native_archive(REAL).unwrap();
      let shapes: Vec<_> = archive
         .objects
         .iter()
         .map(|object| {
            let common = Document::parse(object.region("commonCRDTData").unwrap().bytes)
               .unwrap()
               .root()
               .unwrap()
               .unwrap();
            let specific = object
               .region("specificCRDTData")
               .and_then(|region| Document::parse(region.bytes).unwrap().root().unwrap());
            decode_shape(common, specific, object.persistence_type).unwrap()
         })
         .collect();
      let textbox = &shapes[0];
      assert_eq!(textbox.geometry.frame.unwrap(), FreeformFrame {
         x:        763.0,
         y:        1226.5,
         w:        0.0,
         h:        0.0,
         rotation: -0.0,
      });
      assert_eq!(textbox.geometry.width_valid, Some(false));
      assert_eq!(shapes[1].geometry.frame.unwrap().rotation, -std::f64::consts::FRAC_PI_4);
      assert_eq!(shapes[1].style.stroke.as_ref().unwrap().width, 3.0);
      assert_eq!(shapes[5].preset.as_deref(), Some("Parallelogram_950"));
      for index in [3, 4, 5, 9] {
         let Some(FreeformPaint::Solid { color }) = &shapes[index].style.fill else {
            panic!("missing shape fill")
         };
         assert_eq!(color.hex.as_deref(), Some("#5AC4F6"));
      }
      let sticky = &shapes[7];
      let Some(FreeformPaint::Solid { color }) = &sticky.style.fill else {
         panic!("missing sticky fill")
      };
      assert_eq!(color.hex.as_deref(), Some("#FFE16C"));
      assert_eq!(sticky.geometry.frame.unwrap().w, 200.0);
      let shadow = &sticky.style.shadows[0];
      assert_eq!(shadow.kind, "curved");
      assert_eq!(shadow.radius, 15.0);
      assert_eq!(shadow.curve, Some(f64::from(-0.123095_f32)));
      assert_eq!(shadow.color.alpha, f64::from(0.35_f32));
      assert_eq!(sticky.path.as_ref().unwrap().commands.len(), 6);
      assert_eq!(shapes[10].style.shadows[0].opacity, Some(0.25));
   }

   #[test]
   fn captured_bezier_commands_preserve_zero_points_and_relative_controls() {
      let archive = parse_native_archive(REAL).unwrap();
      let common = Document::parse(archive.objects[3].region("commonCRDTData").unwrap().bytes)
         .unwrap()
         .root()
         .unwrap()
         .unwrap();
      let path = path(
         common
            .property("b")
            .unwrap()
            .unwrap()
            .field(1)
            .unwrap()
            .unwrap(),
      )
      .unwrap()
      .unwrap();
      assert_eq!(path.commands.len(), 7);
      let FreeformPathCommand::Cubic { point, control_1, control_2 } = path.commands[1] else {
         panic!("missing native circle curve");
      };
      assert_eq!(point, FreeformPoint { x: 128.0330047607422, y: 128.0330047607422 });
      assert_eq!(control_1, FreeformPoint {
         x: point.x + 29.289321899414062,
         y: point.y - 76.77669525146484,
      });
      assert_eq!(control_2, FreeformPoint {
         x: point.x + 29.289321899414062,
         y: point.y - 29.289321899414062,
      });
      assert_eq!(path.horizontal_flip, Some(false));
      assert_eq!(path.vertical_flip, Some(false));
      assert!(path.natural_size.is_some());
   }
}
