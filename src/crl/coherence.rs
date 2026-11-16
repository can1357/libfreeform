//! Borrowed views of Coherence's versioned protobuf documents.
//!
//! [`proto`] declares the native field tables of `Proto_Document`,
//! `Proto_CRDT`, `Proto_Value`, `Proto_Struct`, and `Proto_Constants` from the
//! installed Coherence framework. `CRDT` composites have string keys; custom
//! values have integer coding keys.

use std::{
   collections::{BTreeMap, btree_map::Entry},
   ops::Range,
};

use url::Url;

use super::{envelope::uuid_string, finite, float32, invalid};
use crate::{
   types::FreeformDecodeError,
   wire::{Decode, Repeated, RepeatedIter, Varints},
};

type Result<T, E = FreeformDecodeError> = std::result::Result<T, E>;

/// Coherence's `Proto_*` schemas, limited to the fields this decoder reads.
pub(super) mod proto {
   use crate::wire::{self, Repeated, Varints};

   wire::message! {
      /// `Proto_Document`: an inline root or a capsule's object pool.
      pub struct Document<'a> {
         /// Inline `Proto_CRDT` root.
         crdt: Option<&'a [u8]> = 1,
         /// Capsule root, selected by its key path.
         reference: Option<Reference<'a>> = 3,
         /// Capsule object pool.
         references: Repeated<'a, Reference<'a>> = 4,
         constants: Option<Constants<'a>> = 6,
      }
   }

   wire::message! {
      /// `Proto_Constants`: document-local tables that values index into.
      pub struct Constants<'a> {
         /// Packed 16-byte UUIDs.
         uuids: Option<&'a [u8]> = 1,
         strings: Repeated<'a, &'a [u8]> = 2,
         datas: Repeated<'a, &'a [u8]> = 3,
      }
   }

   wire::message! {
      /// `Proto_Reference`: a capsule object and the key path naming it.
      pub struct Reference<'a> {
         /// Referenced `Proto_CRDT`.
         crdt: Option<&'a [u8]> = 4,
         key_path: Option<KeyPath<'a>> = 5,
      }
   }

   wire::message! {
      /// Capsule reference key path.
      pub struct KeyPath<'a> {
         data: Option<&'a [u8]> = 2,
      }
   }

   impl<'a> Reference<'a> {
      /// The opaque key that capsule lookups compare.
      pub fn key(self) -> Option<&'a [u8]> {
         self.key_path.and_then(|path| path.data)
      }
   }

   wire::message! {
      /// `Proto_CRDT`; a node carries one of these kinds.
      pub struct Crdt<'a> {
         register: Option<Register<'a>> = 1,
         dictionary: Option<Set<'a>> = 3,
         composite: Option<Composite<'a>> = 4,
         /// Attributed text.
         string: Option<AttributedString<'a>> = 5,
         ordered_set: Option<OrderedSet<'a>> = 9,
      }
   }

   wire::message! {
      /// `Proto_Register`: a timestamped `Proto_Value`.
      pub struct Register<'a> {
         value: Option<&'a [u8]> = 2,
      }
   }

   wire::message! {
      /// Parallel string-constant keys and `Proto_CRDT` values.
      pub struct Composite<'a> {
         keys: Varints<'a> = 1,
         values: Repeated<'a, &'a [u8]> = 2,
      }
   }

   wire::message! {
      /// Set or dictionary snapshot.
      pub struct Set<'a> {
         insertions: Repeated<'a, Insertion<'a>> = 4,
      }
   }

   wire::message! {
      /// One collection element.
      pub struct Insertion<'a> {
         /// `Proto_Value` key.
         key: Option<&'a [u8]> = 1,
         /// Register `Proto_CRDT` holding a dictionary value.
         value: Option<&'a [u8]> = 2,
      }
   }

   wire::message! {
      /// `Proto_OrderedSet`, also backing ordered dictionaries.
      pub struct OrderedSet<'a> {
         set: Option<Set<'a>> = 1,
         sequence: Option<Sequence<'a>> = 2,
         /// One per set insertion, breaking duplicate-key ties.
         timestamps: Repeated<'a, Timestamp> = 3,
         /// 0 for sets, 1 for dictionaries.
         kind: Option<u64> = 4,
      }
   }

   wire::message! {
      /// `Proto_Sequence`: the subsequence tree ordering content.
      pub struct Sequence<'a> {
         subsequences: Repeated<'a, Subsequence<'a>> = 3,
      }
   }

   wire::message! {
      /// A run of sequence content and its child subsequence indices.
      pub struct Subsequence<'a> {
         length: Option<u64> = 2,
         added: Option<Timestamp> = 3,
         children: Varints<'a> = 4,
         removed: Option<VersionVector<'a>> = 5,
         /// 0 when content is stored, 1 when it is elided.
         content: Option<u64> = 6,
      }
   }

   wire::message! {
      /// `Proto_Timestamp`: one replica's counter.
      pub struct Timestamp {
         /// UUID-constant index of the replica.
         uuid: Option<u64> = 1,
         counter: Option<u64> = 2,
      }
   }

   wire::message! {
      /// `Proto_VersionVector`: parallel replica UUID indices and index sets.
      pub struct VersionVector<'a> {
         uuids: Varints<'a> = 1,
         index_sets: Repeated<'a, IndexSet<'a>> = 2,
      }
   }

   wire::message! {
      /// Counter ranges as (gap from previous end, length) pairs.
      pub struct IndexSet<'a> {
         ranges: Varints<'a> = 2,
      }
   }

   wire::message! {
      /// Attributed string text and its attribute runs.
      pub struct AttributedString<'a> {
         text: Option<&'a str> = 1,
         attribute_runs: Repeated<'a, AttributeRun<'a>> = 6,
      }
   }

   wire::message! {
      /// Consecutive UTF-16 length sharing named attributes.
      pub struct AttributeRun<'a> {
         length: Option<u64> = 1,
         attributes: Repeated<'a, Attribute<'a>> = 2,
      }
   }

   wire::message! {
      /// One named attribute of a run.
      pub struct Attribute<'a> {
         /// String-constant index of the name.
         key: Option<u64> = 1,
         /// `Proto_Value`.
         value: Option<&'a [u8]> = 2,
      }
   }

   wire::message! {
      /// `Proto_Value`; one alternative is set.
      pub struct Value<'a> {
         uint: Option<u64> = 1,
         double: Option<f64> = 2,
         /// String-constant index.
         string: Option<u64> = 3,
         data: Option<&'a [u8]> = 4,
         /// Zigzag-encoded signed integer.
         sint: Option<u64> = 5,
         /// UUID-constant index.
         uuid: Option<u64> = 6,
         reference: Option<Reference<'a>> = 8,
         optional: Option<Optional<'a>> = 9,
         repeated: Option<RepeatedValue<'a>> = 11,
         custom: Option<Struct<'a>> = 14,
         float: Option<f32> = 15,
         /// Data-constant index of a capsule key path.
         weak_reference: Option<u64> = 16,
         /// Data-constant index of a capsule key path.
         strong_reference: Option<u64> = 17,
      }
   }

   wire::message! {
      /// Explicit optional; an absent wrapped value is `nil`.
      pub struct Optional<'a> {
         wrapped: Option<&'a [u8]> = 1,
      }
   }

   wire::message! {
      /// `Proto_RepeatedValue`.
      pub struct RepeatedValue<'a> {
         values: Repeated<'a, &'a [u8]> = 1,
      }
   }

   wire::message! {
      /// `Proto_Struct`: a Codable value's integer coding keys and values.
      pub struct Struct<'a> {
         /// Absent keys mean dense zero-based order.
         keys: Varints<'a> = 1,
         values: Repeated<'a, &'a [u8]> = 2,
      }
   }
}

/// Constant tables belonging to one `CRDT` document, never to its neighbours.
#[derive(Clone, Copy, Debug)]
pub(super) struct Context<'a> {
   constants: proto::Constants<'a>,
}

impl<'a> Context<'a> {
   fn entry(table: Repeated<'a, &'a [u8]>, index: u64) -> Result<&'a [u8]> {
      let index = usize::try_from(index).map_err(|_| invalid("constant index overflow"))?;
      table
         .get(index)?
         .ok_or_else(|| invalid("constant index out of range"))
   }

   /// Resolves a string constant without allocation.
   pub fn string(self, index: u64) -> Result<&'a str> {
      std::str::from_utf8(Self::entry(self.constants.strings, index)?)
         .map_err(|_| invalid("invalid string constant"))
   }

   /// Resolves opaque data used by capsule reference key paths.
   pub fn data(self, index: u64) -> Result<&'a [u8]> {
      Self::entry(self.constants.datas, index)
   }

   /// Borrows a UUID from the document's packed UUID table.
   pub fn uuid_bytes(self, index: u64) -> Result<&'a [u8; 16]> {
      let bytes = self.constants.uuids.unwrap_or_default();
      let index = usize::try_from(index)
         .ok()
         .and_then(|index| index.checked_mul(16))
         .ok_or_else(|| invalid("UUID constant index overflow"))?;
      bytes
         .get(index..)
         .and_then(|bytes| bytes.first_chunk::<16>())
         .ok_or_else(|| invalid("UUID constant index out of range"))
   }

   /// Resolves a UUID from the document's packed UUID table.
   pub fn uuid(self, index: u64) -> Result<String> {
      uuid_string(self.uuid_bytes(index)?)
   }

   /// Attaches these constants to a bounded `Proto_CRDT`.
   pub const fn node(self, bytes: &'a [u8]) -> Node<'a> {
      Node { bytes, context: self }
   }

   /// Attaches these constants to a bounded `Proto_Value`.
   pub const fn value(self, bytes: &'a [u8]) -> Value<'a> {
      Value { bytes, context: self }
   }
}

/// A complete `CRDT` or capsule document with its own constant tables.
#[derive(Clone, Copy, Debug)]
pub(super) struct Document<'a> {
   body:    proto::Document<'a>,
   context: Context<'a>,
}

impl<'a> Document<'a> {
   /// Parses native `crdt` framing and rejects unsupported encodings
   /// explicitly.
   pub fn parse(bytes: &'a [u8]) -> Result<Self> {
      let (header, body) = bytes
         .split_first_chunk::<8>()
         .ok_or_else(|| FreeformDecodeError::incomplete("Coherence: truncated document header"))?;
      if &header[..4] != b"crdt" {
         return Err(invalid("missing Coherence crdt header"));
      }
      let version = u32::from_le_bytes(*header.last_chunk().expect("four bytes"));
      if !matches!(version, 6 | 7) {
         return Err(FreeformDecodeError::unsupported(format!(
            "Coherence document version {version}"
         )));
      }
      let body = proto::Document::parse(body)?;
      let constants = body
         .constants
         .map_or_else(|| proto::Constants::parse(&[]), Ok)?;
      if !constants.uuids.unwrap_or_default().len().is_multiple_of(16) {
         return Err(invalid("truncated UUID constant table"));
      }
      Ok(Self { body, context: Context { constants } })
   }

   /// Reads an inline root or resolves the capsule's declared root key path.
   pub fn root(self) -> Result<Option<Node<'a>>> {
      if let Some(crdt) = self.body.crdt {
         return Ok(Some(self.context.node(crdt)));
      }
      let Some(reference) = self.body.reference else {
         return Ok(None);
      };
      self.resolve_key(
         reference
            .key()
            .ok_or_else(|| invalid("capsule root has no key path"))?,
      )
   }

   fn resolve_key(self, key: &[u8]) -> Result<Option<Node<'a>>> {
      let mut result = None;
      for reference in self.body.references {
         let reference = reference?;
         if reference.key() == Some(key) {
            if result.is_some() {
               return Err(invalid("duplicate capsule reference key"));
            }
            result = reference.crdt.map(|crdt| self.context.node(crdt));
         }
      }
      Ok(result)
   }

   /// Resolves a strong or weak capsule reference through the document's own
   /// object pool.
   pub fn resolve(self, value: Value<'a>) -> Result<Option<Node<'a>>> {
      let proto = value.proto()?;
      if let Some(index) = proto.strong_reference.or(proto.weak_reference) {
         return self.resolve_key(value.context.data(index)?);
      }
      match proto.reference.and_then(proto::Reference::key) {
         Some(key) => self.resolve_key(key),
         None => Ok(None),
      }
   }
}

/// One `Proto_CRDT`, such as a composite, register, dictionary, or attributed
/// string.
#[derive(Clone, Copy, Debug)]
pub(super) struct Node<'a> {
   /// Encoded `Proto_CRDT`.
   pub bytes:   &'a [u8],
   pub context: Context<'a>,
}

impl<'a> Node<'a> {
   /// Decodes this node's declared `Proto_CRDT` fields.
   pub fn crdt(self) -> Result<proto::Crdt<'a>> {
      proto::Crdt::parse(self.bytes)
   }

   /// Finds a composite field by its resolved native string key.
   pub fn field(self, key: &str) -> Result<Option<Self>> {
      let Some(composite) = self.crdt()?.composite else {
         return Ok(None);
      };
      let mut result = None;
      keyed(
         composite.keys,
         composite.values,
         "composite key/value count mismatch",
         |index, value| {
            if self.context.string(index)? == key
               && result.replace(self.context.node(value)).is_some()
            {
               return Err(invalid("duplicate composite key"));
            }
            Ok(())
         },
      )?;
      Ok(result)
   }

   /// Follows only explicitly named `CRDT` composite fields.
   pub fn at(self, keys: &[&str]) -> Result<Option<Self>> {
      let mut node = self;
      for key in keys {
         let Some(child) = node.field(key)? else {
            return Ok(None);
         };
         node = child;
      }
      Ok(Some(node))
   }

   /// Reads the resolved value of a `CRRegister`, not its timestamp or `CRDT`
   /// payload.
   pub fn register(self) -> Result<Option<Value<'a>>> {
      Ok(self
         .crdt()?
         .register
         .and_then(|register| register.value)
         .map(|value| self.context.value(value)))
   }

   /// Reads live entries of a native `CRDictionary` snapshot.
   pub fn dictionary(self) -> Result<Vec<(Value<'a>, Value<'a>)>> {
      let Some(set) = self.crdt()?.dictionary else {
         return Ok(Vec::new());
      };
      collection_entries(set, self.context)?
         .into_iter()
         .map(|entry| {
            Ok((
               entry.key,
               entry
                  .value
                  .ok_or_else(|| invalid("dictionary insertion has no register value"))?,
            ))
         })
         .collect()
   }

   /// Reads an ordered set using its sequence tree and timestamp-based
   /// duplicate resolution.
   pub fn ordered_set(self) -> Result<Vec<Value<'a>>> {
      Ok(self
         .ordered_entries(false)?
         .into_iter()
         .map(|entry| entry.key)
         .collect())
   }

   /// Reads a native ordered dictionary in its resolved row/column order.
   pub fn ordered_dictionary(self) -> Result<Vec<(Value<'a>, Value<'a>)>> {
      self
         .ordered_entries(true)?
         .into_iter()
         .map(|entry| {
            Ok((
               entry.key,
               entry
                  .value
                  .ok_or_else(|| invalid("ordered dictionary insertion has no value"))?,
            ))
         })
         .collect()
   }

   fn ordered_entries(self, dictionary: bool) -> Result<Vec<CollectionEntry<'a>>> {
      let Some(ordered) = self.crdt()?.ordered_set else {
         return Ok(Vec::new());
      };
      if ordered.kind.unwrap_or(0) != u64::from(dictionary) {
         return Err(invalid("ordered collection kind mismatch"));
      }
      let entries = ordered
         .set
         .map(|set| collection_entries(set, self.context))
         .transpose()?
         .unwrap_or_default();
      let order = sequence_order(ordered.sequence, entries.len(), self.context)?;
      let mut timestamps = Vec::with_capacity(entries.len());
      for stamp in ordered.timestamps {
         let stamp = stamp?;
         timestamps
            .push((stamp.counter.unwrap_or(0), *self.context.uuid_bytes(stamp.uuid.unwrap_or(0))?));
      }
      if timestamps.len() != entries.len() {
         return Err(invalid("ordered element/timestamp count mismatch"));
      }
      let mut winners = BTreeMap::new();
      for &index in &order {
         match winners.entry(entries[index].key.bytes) {
            Entry::Vacant(entry) => {
               entry.insert(index);
            },
            Entry::Occupied(mut entry) => {
               if timestamps[index] > timestamps[*entry.get()] {
                  entry.insert(index);
               }
            },
         }
      }
      Ok(order
         .into_iter()
         .filter(|&index| winners.get(entries[index].key.bytes) == Some(&index))
         .map(|index| entries[index])
         .collect())
   }

   /// Reads a named register and unwraps its explicit optional-value wrapper.
   pub fn property(self, key: &str) -> Result<Option<Value<'a>>> {
      let Some(node) = self.field(key)? else {
         return Ok(None);
      };
      match node.register()? {
         Some(value) => value.optional(),
         None => Ok(None),
      }
   }
}

/// One schema-tagged `CRValue`, with custom integer-keyed structs kept
/// bounded.
#[derive(Clone, Copy, Debug)]
pub(super) struct Value<'a> {
   /// Encoded `Proto_Value`.
   pub bytes:   &'a [u8],
   pub context: Context<'a>,
}

impl<'a> Value<'a> {
   fn proto(self) -> Result<proto::Value<'a>> {
      proto::Value::parse(self.bytes)
   }

   /// Unwraps the native optional wrapper; absence is distinct from a zero
   /// value.
   pub fn optional(self) -> Result<Option<Self>> {
      match self.proto()?.optional {
         Some(optional) => Ok(optional.wrapped.map(|wrapped| self.context.value(wrapped))),
         None => Ok(Some(self)),
      }
   }

   /// Reads a custom value's integer coding key; omitted keys mean dense
   /// zero-based order.
   pub fn field(self, key: u64) -> Result<Option<Self>> {
      let Some(custom) = self.proto()?.custom else {
         return Ok(None);
      };
      let mut result = None;
      let mut visit = |number, value| {
         if number == key && result.replace(self.context.value(value)).is_some() {
            return Err(invalid("duplicate custom coding key"));
         }
         Ok(())
      };
      if custom.keys.is_present() {
         keyed(custom.keys, custom.values, "custom key/value count mismatch", &mut visit)?;
      } else {
         for (number, value) in (0..).zip(custom.values) {
            visit(number, value?)?;
         }
      }
      Ok(result)
   }

   /// Decodes the native integer alternatives, including signed zigzag
   /// encoding.
   pub fn integer(self) -> Result<Option<i64>> {
      integer(&self.proto()?)
   }

   /// Decodes explicit native floating-point or integer scalar alternatives.
   pub fn number(self) -> Result<Option<f64>> {
      let value = self.proto()?;
      if let Some(float) = float32(value.float)? {
         return Ok(Some(float));
      }
      if let Some(double) = value.double {
         return finite(double).map(Some);
      }
      Ok(integer(&value)?.map(|value| value as f64))
   }

   /// Decodes the archived boolean integer, rejecting values other than zero
   /// and one.
   pub fn boolean(self) -> Result<Option<bool>> {
      match self.integer()? {
         Some(0) => Ok(Some(false)),
         Some(1) => Ok(Some(true)),
         None => Ok(None),
         _ => Err(invalid("boolean is not zero or one")),
      }
   }

   /// Iterates a repeated `CRValue` without allocating an intermediate array.
   pub fn elements(self) -> Result<Elements<'a>> {
      Ok(Elements {
         values:  self
            .proto()?
            .repeated
            .map(|repeated| repeated.values.into_iter()),
         context: self.context,
      })
   }

   /// Resolves Foundation's relative URL representation against its optional
   /// base.
   pub fn url(self) -> Result<Option<String>> {
      self.url_at_depth(0).map(|url| url.map(Into::into))
   }

   fn url_at_depth(self, depth: usize) -> Result<Option<Url>> {
      if depth > 64 {
         return Err(invalid("URL base nesting exceeds supported depth"));
      }
      let Some(relative) = self.field(1)?.map(Self::string).transpose()?.flatten() else {
         return Ok(None);
      };
      let base = self.field(0)?.map(Self::optional).transpose()?.flatten();
      let base = base
         .map(|base| base.url_at_depth(depth + 1))
         .transpose()?
         .flatten();
      let url = match base {
         Some(base) => base.join(relative),
         None => Url::parse(relative),
      }
      .map_err(|error| FreeformDecodeError::invalid(format!("native URL: {error}")))?;
      Ok(Some(url))
   }

   /// Resolves a `CRValue` string constant.
   pub fn string(self) -> Result<Option<&'a str>> {
      self
         .proto()?
         .string
         .map(|index| self.context.string(index))
         .transpose()
   }

   /// Resolves a `CRValue` UUID constant.
   pub fn uuid(self) -> Result<Option<String>> {
      self
         .proto()?
         .uuid
         .map(|index| self.context.uuid(index))
         .transpose()
   }

   /// Returns explicitly tagged opaque data without reinterpretation.
   pub fn data(self) -> Result<Option<&'a [u8]>> {
      Ok(self.proto()?.data)
   }
}

/// Decodes `Proto_Value.sint` (zigzag) or `Proto_Value.uint`, preferring the
/// signed alternative.
fn integer(value: &proto::Value<'_>) -> Result<Option<i64>> {
   if let Some(value) = value.sint {
      return Ok(Some(i64::from_le_bytes(
         ((value >> 1) ^ 0_u64.wrapping_sub(value & 1)).to_le_bytes(),
      )));
   }
   value
      .uint
      .map(|value| i64::try_from(value).map_err(|_| invalid("integer exceeds i64")))
      .transpose()
}

/// Borrowed iteration over `Proto_RepeatedValue.values`.
pub(super) struct Elements<'a> {
   values:  Option<RepeatedIter<'a, &'a [u8]>>,
   context: Context<'a>,
}

impl<'a> Iterator for Elements<'a> {
   type Item = Result<Value<'a>>;

   fn next(&mut self) -> Option<Self::Item> {
      let value = self.values.as_mut()?.next()?;
      Some(value.map(|value| self.context.value(value)))
   }
}

/// Visits parallel key and value arrays in order, rejecting unequal lengths
/// with `mismatch`.
fn keyed<'a, T: Decode<'a>>(
   keys: Varints<'a>,
   values: Repeated<'a, T>,
   mismatch: &str,
   mut visit: impl FnMut(u64, T) -> Result<()>,
) -> Result<()> {
   let mut keys = keys.into_iter();
   for value in values {
      let key = keys.next().transpose()?.ok_or_else(|| invalid(mismatch))?;
      visit(key, value?)?;
   }
   if keys.next().is_some() {
      return Err(invalid(mismatch));
   }
   Ok(())
}

#[derive(Clone, Copy)]
struct CollectionEntry<'a> {
   key:   Value<'a>,
   value: Option<Value<'a>>,
}

fn collection_entries<'a>(
   set: proto::Set<'a>,
   context: Context<'a>,
) -> Result<Vec<CollectionEntry<'a>>> {
   set.insertions
      .into_iter()
      .map(|insertion| {
         let insertion = insertion?;
         let key = insertion
            .key
            .ok_or_else(|| invalid("collection insertion has no key"))?;
         let value = insertion
            .value
            .map(|crdt| context.node(crdt).register())
            .transpose()?
            .flatten();
         Ok(CollectionEntry { key: context.value(key), value })
      })
      .collect()
}

/// Returns visible content indices in native subsequence-tree order.
fn sequence_order(
   sequence: Option<proto::Sequence<'_>>,
   content_len: usize,
   context: Context<'_>,
) -> Result<Vec<usize>> {
   struct Segment {
      content:  Range<usize>,
      children: Vec<usize>,
      visible:  bool,
   }
   let mut segments = Vec::new();
   let mut consumed = 0_usize;
   for subsequence in sequence
      .into_iter()
      .flat_map(|sequence| sequence.subsequences)
   {
      let subsequence = subsequence?;
      let length = usize::try_from(subsequence.length.unwrap_or(0))
         .map_err(|_| invalid("subsequence length overflow"))?;
      let stored = match subsequence.content.unwrap_or(0) {
         0 => length,
         1 => 0,
         _ => return Err(invalid("unknown subsequence content option")),
      };
      let end = consumed
         .checked_add(stored)
         .filter(|&end| end <= content_len)
         .ok_or_else(|| invalid("subsequence exceeds stored content"))?;
      let children = subsequence
         .children
         .into_iter()
         .map(|index| usize::try_from(index?).map_err(|_| invalid("child index overflow")))
         .collect::<Result<Vec<_>>>()?;
      let removed = match (subsequence.added, subsequence.removed) {
         (Some(added), Some(removed)) => version_contains(removed, added, context)?,
         _ => false,
      };
      segments.push(Segment { content: consumed..end, children, visible: !removed });
      consumed = end;
   }
   if consumed != content_len {
      return Err(invalid("subsequence/content count mismatch"));
   }
   if segments.is_empty() {
      return Ok(Vec::new());
   }
   let mut order = Vec::with_capacity(content_len);
   let mut stack = vec![0_usize];
   let mut visited = vec![false; segments.len()];
   while let Some(index) = stack.pop() {
      let segment = segments
         .get(index)
         .ok_or_else(|| invalid("subsequence child out of range"))?;
      if visited[index] {
         return Err(invalid("cyclic or multiply owned subsequence"));
      }
      visited[index] = true;
      if segment.visible {
         order.extend(segment.content.clone());
      }
      stack.extend(segment.children.iter().rev().copied());
   }
   if visited.iter().any(|&visited| !visited) {
      return Err(invalid("unreachable subsequence"));
   }
   Ok(order)
}

fn version_contains(
   version: proto::VersionVector<'_>,
   timestamp: proto::Timestamp,
   context: Context<'_>,
) -> Result<bool> {
   let target = context.uuid_bytes(timestamp.uuid.unwrap_or(0))?;
   let counter = timestamp.counter.unwrap_or(0);
   let mut result = false;
   keyed(
      version.uuids,
      version.index_sets,
      "version UUID/index-set count mismatch",
      |uuid, index_set| {
         let mut ranges = index_set.ranges.into_iter();
         let mut end = 0_u64;
         while let Some(gap) = ranges.next() {
            let start = end
               .checked_add(gap?)
               .ok_or_else(|| invalid("version range overflow"))?;
            let length = ranges
               .next()
               .transpose()?
               .ok_or_else(|| invalid("version range has no length"))?;
            end = start
               .checked_add(length)
               .ok_or_else(|| invalid("version range overflow"))?;
            if context.uuid_bytes(uuid)? == target && (start..end).contains(&counter) {
               result = true;
            }
         }
         Ok(())
      },
   )?;
   Ok(result)
}
