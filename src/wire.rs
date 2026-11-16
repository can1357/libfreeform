//! Borrowed protobuf decoding shared by the `PencilKit`, native-archive, and
//! Coherence decoders.
//!
//! [`message!`] declares a borrowed view of one message schema. Its generated
//! `parse` scans the encoded bytes once: singular fields keep their last
//! occurrence, declared fields with the wrong wire type are errors, and
//! undeclared fields are skipped. Repeated fields stay borrowed and decode
//! while iterated, so recursive schemas cost nothing until traversed. Truncated
//! input reports [`FreeformDecodeErrorKind::Incomplete`], never `Invalid`.
//!
//! # Example
//!
//! ```ignore
//! wire::message! {
//!    /// `Proto_Timestamp`: one replica's Lamport counter.
//!    pub struct Timestamp {
//!       uuid: Option<u64> = 1,
//!       counter: Option<u64> = 2,
//!    }
//! }
//!
//! let stamp = Timestamp::parse(bytes)?;
//! let counter = stamp.counter.unwrap_or(0);
//! ```
//!
//! [`FreeformDecodeErrorKind::Incomplete`]: crate::types::FreeformDecodeErrorKind::Incomplete

use std::{fmt, marker::PhantomData};

use crate::types::FreeformDecodeError;

type Result<T, E = FreeformDecodeError> = std::result::Result<T, E>;

/// Declares a borrowed message view and its single-pass `parse`.
///
/// Each field is `name: Type = number`, where `Type` is `Option<T>` for a
/// singular field, [`Repeated<'a, T>`] for a repeated one, or [`Varints<'a>`]
/// for repeated varints. `T` is any [`Decode`] type, including other declared
/// messages, which are parsed eagerly when singular. Declarations without a
/// lifetime hold no borrowed fields.
macro_rules! message {
   (@id $name:ident $field:ident $number:literal) => {
      &$crate::wire::FieldId {
         number: $number,
         path:   concat!(stringify!($name), ".", stringify!($field)),
      }
   };
   (@emit [$($meta:tt)*] $vis:vis $name:ident [$($generics:tt)*] $lt:lifetime {
      $($(#[$field_meta:meta])* $field:ident: $ty:ty = $number:literal),* $(,)?
   }) => {
      $($meta)*
      #[derive(Clone, Copy, Debug)]
      $vis struct $name $($generics)* {
         $($(#[$field_meta])* pub $field: $ty,)*
      }

      impl<$lt> $name $($generics)* {
         /// Decodes every declared field in one pass, skipping undeclared ones.
         pub fn parse(
            data: &$lt [u8],
         ) -> ::core::result::Result<Self, $crate::types::FreeformDecodeError> {
            let mut message = Self {
               $($field: $crate::wire::Slot::empty(
                  data,
                  $crate::wire::message!(@id $name $field $number),
               ),)*
            };
            for field in $crate::wire::Fields::new(data) {
               let field = field?;
               match field.number {
                  $($number => $crate::wire::accept(
                     &mut message.$field,
                     field.value,
                     $crate::wire::message!(@id $name $field $number),
                  )?,)*
                  _ => {},
               }
            }
            Ok(message)
         }
      }

      impl<$lt> $crate::wire::Decode<$lt> for $name $($generics)* {
         fn decode(
            value: $crate::wire::Value<$lt>,
         ) -> ::core::result::Result<Self, $crate::types::FreeformDecodeError> {
            Self::parse($crate::wire::Decode::decode(value)?)
         }
      }
   };
   ($(#[$meta:meta])* $vis:vis struct $name:ident<$lt:lifetime> { $($body:tt)* }) => {
      $crate::wire::message!(@emit [$(#[$meta])*] $vis $name [<$lt>] $lt { $($body)* });
   };
   ($(#[$meta:meta])* $vis:vis struct $name:ident { $($body:tt)* }) => {
      $crate::wire::message!(@emit [$(#[$meta])*] $vis $name [] 'wire { $($body)* });
   };
}
pub(crate) use message;

/// One protobuf wire value; numeric interpretation belongs to its schema.
#[derive(Clone, Copy, Debug)]
pub enum Value<'a> {
   Varint(u64),
   Fixed64(u64),
   Bytes(&'a [u8]),
   Fixed32(u32),
}

/// A field number and its bounded value.
#[derive(Clone, Copy, Debug)]
pub struct Field<'a> {
   pub number: u32,
   pub value:  Value<'a>,
}

/// Fields of a bounded message in wire order, stopping after the first
/// malformed one. Schema-free walks use this directly; declared messages use
/// [`message!`].
pub struct Fields<'a> {
   remaining: &'a [u8],
   failed:    bool,
}

impl<'a> Fields<'a> {
   /// Walks `data` without allocating or interpreting nested bytes.
   pub const fn new(data: &'a [u8]) -> Self {
      Self { remaining: data, failed: false }
   }
}

impl<'a> Iterator for Fields<'a> {
   type Item = Result<Field<'a>>;

   fn next(&mut self) -> Option<Self::Item> {
      if self.failed || self.remaining.is_empty() {
         return None;
      }
      let result = field(&mut self.remaining);
      self.failed = result.is_err();
      Some(result)
   }
}

/// A Rust type that one occurrence of a declared field decodes into.
pub trait Decode<'a>: Sized {
   /// Decodes one occurrence, rejecting wire types the schema does not allow.
   fn decode(value: Value<'a>) -> Result<Self>;
}

impl Decode<'_> for u64 {
   fn decode(value: Value<'_>) -> Result<Self> {
      match value {
         Value::Varint(value) => Ok(value),
         _ => Err(invalid("expected a varint")),
      }
   }
}

impl Decode<'_> for f32 {
   fn decode(value: Value<'_>) -> Result<Self> {
      match value {
         Value::Fixed32(bits) => Ok(Self::from_bits(bits)),
         _ => Err(invalid("expected a fixed32 float")),
      }
   }
}

impl Decode<'_> for f64 {
   fn decode(value: Value<'_>) -> Result<Self> {
      match value {
         Value::Fixed64(bits) => Ok(Self::from_bits(bits)),
         _ => Err(invalid("expected a fixed64 double")),
      }
   }
}

impl<'a> Decode<'a> for &'a [u8] {
   fn decode(value: Value<'a>) -> Result<Self> {
      match value {
         Value::Bytes(bytes) => Ok(bytes),
         _ => Err(invalid("expected a length-delimited field")),
      }
   }
}

impl<'a> Decode<'a> for &'a str {
   fn decode(value: Value<'a>) -> Result<Self> {
      std::str::from_utf8(Decode::decode(value)?).map_err(|_| invalid("invalid UTF-8 string"))
   }
}

/// A declared field's number and its `Message.field` name for errors.
#[derive(Debug)]
pub struct FieldId {
   pub number: u32,
   pub path:   &'static str,
}

/// Storage for one declared field, filled by a [`message!`] `parse`.
///
/// `Option<T>` keeps the last occurrence; [`Repeated`] and [`Varints`] keep
/// every occurrence borrowed in the enclosing message.
pub trait Slot<'a> {
   /// The slot before any occurrence of `field` within `data`.
   fn empty(data: &'a [u8], field: &'static FieldId) -> Self;

   /// Records one occurrence of the field.
   fn accept(&mut self, value: Value<'a>) -> Result<()>;
}

/// Records one occurrence for a [`message!`] `parse`, naming the field on
/// failure.
pub fn accept<'a>(
   slot: &mut impl Slot<'a>,
   value: Value<'a>,
   field: &'static FieldId,
) -> Result<()> {
   slot.accept(value).map_err(within(field))
}

impl<'a, T: Decode<'a>> Slot<'a> for Option<T> {
   fn empty(_data: &'a [u8], _field: &'static FieldId) -> Self {
      None
   }

   fn accept(&mut self, value: Value<'a>) -> Result<()> {
      *self = Some(T::decode(value)?);
      Ok(())
   }
}

/// Every occurrence of a repeated field; each decodes as `T` only while
/// iterated.
pub struct Repeated<'a, T> {
   data:  &'a [u8],
   field: &'static FieldId,
   item:  PhantomData<fn() -> T>,
}

impl<'a, T: Decode<'a>> Repeated<'a, T> {
   /// Decodes the `index`th occurrence, rejecting malformed earlier ones.
   pub fn get(self, index: usize) -> Result<Option<T>> {
      let mut items = self.into_iter();
      for _ in 0..index {
         if items.next().transpose()?.is_none() {
            return Ok(None);
         }
      }
      items.next().transpose()
   }
}

impl<'a, T: Decode<'a>> Slot<'a> for Repeated<'a, T> {
   fn empty(data: &'a [u8], field: &'static FieldId) -> Self {
      Self { data, field, item: PhantomData }
   }

   fn accept(&mut self, _value: Value<'a>) -> Result<()> {
      Ok(())
   }
}

impl<'a, T: Decode<'a>> IntoIterator for Repeated<'a, T> {
   type IntoIter = RepeatedIter<'a, T>;
   type Item = Result<T>;

   fn into_iter(self) -> Self::IntoIter {
      RepeatedIter { fields: Fields::new(self.data), field: self.field, item: PhantomData }
   }
}

impl<T> Clone for Repeated<'_, T> {
   fn clone(&self) -> Self {
      *self
   }
}

impl<T> Copy for Repeated<'_, T> {}

impl<T> fmt::Debug for Repeated<'_, T> {
   fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
      f.debug_struct("Repeated")
         .field("field", &self.field.path)
         .finish_non_exhaustive()
   }
}

/// Decoded occurrences of a [`Repeated`] field, in wire order.
pub struct RepeatedIter<'a, T> {
   fields: Fields<'a>,
   field:  &'static FieldId,
   item:   PhantomData<fn() -> T>,
}

impl<'a, T: Decode<'a>> Iterator for RepeatedIter<'a, T> {
   type Item = Result<T>;

   fn next(&mut self) -> Option<Self::Item> {
      self.fields.find_map(|field| match field {
         Ok(field) if field.number != self.field.number => None,
         Ok(field) => Some(T::decode(field.value).map_err(within(self.field))),
         Err(error) => Some(Err(error)),
      })
   }
}

/// A repeated varint field, accepting packed runs and unpacked occurrences
/// alike.
#[derive(Clone, Copy, Debug)]
pub struct Varints<'a> {
   data:    &'a [u8],
   field:   &'static FieldId,
   present: bool,
}

impl Varints<'_> {
   /// Whether the field occurred at all, even as an empty packed run.
   pub const fn is_present(self) -> bool {
      self.present
   }
}

impl<'a> Slot<'a> for Varints<'a> {
   fn empty(data: &'a [u8], field: &'static FieldId) -> Self {
      Self { data, field, present: false }
   }

   fn accept(&mut self, value: Value<'a>) -> Result<()> {
      match value {
         Value::Varint(_) | Value::Bytes(_) => {
            self.present = true;
            Ok(())
         },
         _ => Err(invalid("expected a varint or packed varints")),
      }
   }
}

impl<'a> IntoIterator for Varints<'a> {
   type IntoIter = VarintsIter<'a>;
   type Item = Result<u64>;

   fn into_iter(self) -> Self::IntoIter {
      VarintsIter { fields: Fields::new(self.data), run: &[], field: self.field }
   }
}

/// Values of a [`Varints`] field, flattening packed runs in wire order.
pub struct VarintsIter<'a> {
   fields: Fields<'a>,
   run:    &'a [u8],
   field:  &'static FieldId,
}

impl Iterator for VarintsIter<'_> {
   type Item = Result<u64>;

   fn next(&mut self) -> Option<Self::Item> {
      loop {
         if !self.run.is_empty() {
            let result = varint(&mut self.run).map_err(within(self.field));
            if result.is_err() {
               self.run = &[];
            }
            return Some(result);
         }
         let field = match self.fields.next()? {
            Ok(field) => field,
            Err(error) => return Some(Err(error)),
         };
         if field.number != self.field.number {
            continue;
         }
         match field.value {
            Value::Varint(value) => return Some(Ok(value)),
            Value::Bytes(run) => self.run = run,
            _ => return Some(Err(within(self.field)(invalid("expected a varint")))),
         }
      }
   }
}

fn field<'a>(input: &mut &'a [u8]) -> Result<Field<'a>> {
   let tag = varint(input)?;
   let number = u32::try_from(tag >> 3)
      .ok()
      .filter(|number| (1..=0x1fff_ffff).contains(number))
      .ok_or_else(|| invalid("invalid field number"))?;
   let value = match tag & 7 {
      0 => Value::Varint(varint(input)?),
      1 => Value::Fixed64(u64::from_le_bytes(*take(input, 8)?.first_chunk().expect("eight bytes"))),
      2 => {
         let length = usize::try_from(varint(input)?)
            .map_err(|_| invalid("field length exceeds address space"))?;
         Value::Bytes(take(input, length)?)
      },
      5 => Value::Fixed32(u32::from_le_bytes(*take(input, 4)?.first_chunk().expect("four bytes"))),
      _ => return Err(invalid("unsupported wire type")),
   };
   Ok(Field { number, value })
}

/// Takes an exact bounded slice, advancing the input only on success.
fn take<'a>(input: &mut &'a [u8], length: usize) -> Result<&'a [u8]> {
   let (value, rest) = input
      .split_at_checked(length)
      .ok_or_else(|| FreeformDecodeError::incomplete("protobuf: truncated field"))?;
   *input = rest;
   Ok(value)
}

/// Reads one unsigned varint, rejecting truncation and values wider than 64
/// bits.
fn varint(input: &mut &[u8]) -> Result<u64> {
   let mut result = 0_u64;
   for shift in (0..64).step_by(7) {
      let (&byte, rest) = input
         .split_first()
         .ok_or_else(|| FreeformDecodeError::incomplete("protobuf: truncated varint"))?;
      *input = rest;
      if shift == 63 && byte > 1 {
         return Err(invalid("varint exceeds 64 bits"));
      }
      result |= u64::from(byte & 0x7f) << shift;
      if byte & 0x80 == 0 {
         return Ok(result);
      }
   }
   Err(invalid("varint exceeds 64 bits"))
}

/// Prefixes an error with the declared field it occurred in, keeping its kind.
fn within(field: &'static FieldId) -> impl Fn(FreeformDecodeError) -> FreeformDecodeError {
   move |error| FreeformDecodeError {
      message: format!("{}: {}", field.path, error.message),
      ..error
   }
}

fn invalid(message: &str) -> FreeformDecodeError {
   FreeformDecodeError::invalid(format!("protobuf: {message}"))
}

#[cfg(test)]
mod tests {
   use super::*;
   use crate::types::FreeformDecodeErrorKind;

   message! {
      struct Inner {
         value: Option<u64> = 1,
      }
   }

   message! {
      struct Outer<'a> {
         inner: Option<Inner> = 1,
         ids: Varints<'a> = 2,
      }
   }

   #[test]
   fn singular_fields_keep_the_last_occurrence_and_varints_flatten_packed_runs() {
      let data = [0x10, 1, 0x12, 3, 2, 0xac, 2, 0x10, 4, 0x0a, 2, 0x08, 7, 0x0a, 2, 0x08, 9];
      let outer = Outer::parse(&data).unwrap();
      assert_eq!(outer.inner.unwrap().value, Some(9));
      let ids = outer.ids.into_iter().collect::<Result<Vec<_>>>().unwrap();
      assert_eq!(ids, [1, 2, 300, 4]);
   }

   #[test]
   fn nested_errors_keep_their_kind_and_name_the_field() {
      let truncated = Outer::parse(&[0x0a, 1, 0x08]).unwrap_err();
      assert_eq!(truncated.kind, FreeformDecodeErrorKind::Incomplete);
      assert!(truncated.message.starts_with("Outer.inner: "));
      let mistyped = Outer::parse(&[0x0d, 0, 0, 0, 0]).unwrap_err();
      assert_eq!(mistyped.kind, FreeformDecodeErrorKind::Invalid);
      assert!(mistyped.message.starts_with("Outer.inner: "));
   }
}
