//! Native connector references, path affinity, and ordered group membership.

use super::{
   coherence::{Node, Value},
   shape,
};
use crate::types::{
   FreeformConnectorEndpoint, FreeformDecodeError, FreeformPath, FreeformPathCommand,
};

type Result<T, E = FreeformDecodeError> = std::result::Result<T, E>;

fn endpoint(specific: Node<'_>, key: &str) -> Result<FreeformConnectorEndpoint> {
   Ok(FreeformConnectorEndpoint {
      item_id:             specific
         .property(key)?
         .map(Value::uuid)
         .transpose()?
         .flatten(),
      magnet:              None,
      normalized_position: None,
      point:               None,
      line_end:            None,
   })
}

fn magnet(endpoint: &mut FreeformConnectorEndpoint, value: Option<Value<'_>>) -> Result<()> {
   let Some(value) = value.map(Value::optional).transpose()?.flatten() else {
      return Ok(());
   };
   endpoint.magnet = value
      .field(0)?
      .map(Value::integer)
      .transpose()?
      .flatten()
      .and_then(|kind| match kind {
         0 => Some("none"),
         1 => Some("center"),
         2 => Some("cardinalNorth"),
         3 => Some("cardinalEast"),
         4 => Some("cardinalSouth"),
         5 => Some("cardinalWest"),
         6 => Some("customEdge"),
         7 => Some("customFloating"),
         _ => None,
      })
      .map(str::to_owned);
   endpoint.normalized_position = value.field(1)?.map(shape::point).transpose()?.flatten();
   Ok(())
}

fn line_end(style: Node<'_>, key: &str) -> Result<Option<String>> {
   Ok(style
      .property(key)?
      .map(|value| value.field(4))
      .transpose()?
      .flatten()
      .map(Value::string)
      .transpose()?
      .flatten()
      .map(str::to_owned))
}

/// `CRLConnectionLineItemCRDTData` is `CRStruct_3`: a=shape superData,
/// b=connectedFrom, c=connectedTo. Geometry and routing are not these
/// registers: they are `common.b` shape-parent affinity, key 1
/// `CRLPathSourceData`.
pub fn decode_connector(
   common: Node<'_>,
   specific: Node<'_>,
) -> Result<(
   FreeformConnectorEndpoint,
   FreeformConnectorEndpoint,
   Option<String>,
   Option<FreeformPath>,
)> {
   let mut tail = endpoint(specific, "b")?;
   let mut head = endpoint(specific, "c")?;
   let source = common
      .property("b")?
      .map(|value| value.field(1))
      .transpose()?
      .flatten();
   let path = source.map(shape::path).transpose()?.flatten();
   let connection = source
      .map(|value| value.field(8))
      .transpose()?
      .flatten()
      .map(Value::optional)
      .transpose()?
      .flatten();
   let mut routing = None;
   if let Some(connection) = connection {
      routing = connection
         .field(0)?
         .map(Value::integer)
         .transpose()?
         .flatten()
         .and_then(|kind| match kind {
            0 => Some("quadratic"),
            1 => Some("orthogonal"),
            _ => None,
         })
         .map(str::to_owned);
      magnet(&mut head, connection.field(4)?)?;
      magnet(&mut tail, connection.field(5)?)?;
   }
   if let Some(style) = specific.at(&["a", "a"])? {
      head.line_end = line_end(style, "c")?;
      tail.line_end = line_end(style, "d")?;
   }
   if let Some(path) = &path {
      let mut points = path.commands.iter().filter_map(|command| match command {
         FreeformPathCommand::Move { point }
         | FreeformPathCommand::Line { point }
         | FreeformPathCommand::Quadratic { point, .. }
         | FreeformPathCommand::Cubic { point, .. } => Some(*point),
         FreeformPathCommand::Close => None,
      });
      if let Some(first) = points.next() {
         let last = points.next_back().unwrap_or(first);
         if tail.item_id.is_none() {
            tail.point = Some(first);
         }
         if head.item_id.is_none() {
            head.point = Some(last);
         }
      }
   }
   Ok((tail, head, routing, path))
}

/// The container's sole CRDT field is `items: CROrderedSet<UUID>`.
/// The shared collection decoder resolves order and tombstones, not wire order.
pub fn decode_group(specific: Node<'_>) -> Result<Vec<String>> {
   let Some(items) = specific.field("a")? else {
      return Ok(Vec::new());
   };
   let mut result = Vec::new();
   for value in items.ordered_set()? {
      if let Some(id) = value.uuid()? {
         result.push(id);
      }
   }
   Ok(result)
}

#[cfg(test)]
mod tests {
   use super::*;
   use crate::{
      crl::{coherence::Document, envelope::parse_native_archive},
      types::FreeformPoint,
   };

   #[test]
   fn captured_free_connector_uses_common_path_not_register_timestamps() {
      let archive =
         parse_native_archive(include_bytes!("../../fixtures/real-board.crlnative")).unwrap();
      let object = archive
         .objects
         .iter()
         .find(|object| object.persistence_type == 4)
         .unwrap();
      let common = Document::parse(object.region("commonCRDTData").unwrap().bytes)
         .unwrap()
         .root()
         .unwrap()
         .unwrap();
      let specific = Document::parse(object.region("specificCRDTData").unwrap().bytes)
         .unwrap()
         .root()
         .unwrap()
         .unwrap();
      let (tail, head, routing, path) = decode_connector(common, specific).unwrap();
      assert!(tail.item_id.is_none() && head.item_id.is_none());
      assert_eq!(tail.point, Some(FreeformPoint { x: 0.0, y: 150.0 }));
      assert_eq!(head.point, Some(FreeformPoint { x: 150.0, y: 0.0 }));
      assert_eq!(routing.as_deref(), Some("quadratic"));
      assert_eq!(path.unwrap().commands.len(), 3);
   }
}
