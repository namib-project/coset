// Copyright 2021 Google LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//
////////////////////////////////////////////////////////////////////////////////

//! Common types.

use crate::{
    cbor::{reader::DecoderError, values::Value, writer::EncoderError},
    iana,
    iana::{EnumI64, WithPrivateRange},
    util::{cbor_type_error, AsCborValue},
};
use alloc::{boxed::Box, string::String, vec::Vec};
use core::{cmp::Ordering, convert::TryInto};

#[cfg(test)]
mod tests;

/// Error type for failures in encoding or decoding COSE types.
pub enum CoseError {
    /// CBOR decoding failure.
    DecodeFailed(DecoderError),
    /// CBOR encoding failure.
    EncodeFailed(EncoderError),
    /// Unexpected CBOR type encountered (got, want).
    UnexpectedType(&'static str, &'static str),
    /// Unrecognized value in IANA-controlled range (with no private range).
    UnregisteredIanaValue,
    /// Unrecognized value in neither IANA-controlled range nor private range.
    UnregisteredIanaNonPrivateValue,
}

impl core::convert::From<DecoderError> for CoseError {
    fn from(e: DecoderError) -> Self {
        CoseError::DecodeFailed(e)
    }
}

impl core::convert::From<EncoderError> for CoseError {
    fn from(e: EncoderError) -> Self {
        CoseError::EncodeFailed(e)
    }
}

impl core::fmt::Debug for CoseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CoseError::DecodeFailed(de) => write!(f, "decode CBOR failure: {:?}", de),
            CoseError::EncodeFailed(ee) => write!(f, "encode CBOR failure: {:?}", ee),
            CoseError::UnexpectedType(got, want) => write!(f, "got {}, expected {}", got, want),
            CoseError::UnregisteredIanaValue => write!(f, "expected recognized IANA value"),
            CoseError::UnregisteredIanaNonPrivateValue => {
                write!(f, "expected value in IANA or private use range")
            }
        }
    }
}

/// Extension trait that adds serialization/deserialization methods.
pub trait CborSerializable: AsCborValue {
    /// Create an object instance from serialized CBOR data in a slice.
    fn from_slice(slice: &[u8]) -> Result<Self, CoseError> {
        let value = sk_cbor::reader::read(slice).map_err(CoseError::DecodeFailed)?;
        Self::from_cbor_value(value)
    }

    /// Serialize this object to a vector, consuming it along the way.
    fn to_vec(self) -> Result<Vec<u8>, CoseError> {
        let mut data = Vec::new();
        sk_cbor::writer::write(self.to_cbor_value()?, &mut data)?;
        Ok(data)
    }
}

/// Extension trait that adds tagged serialization/deserialization methods.
pub trait TaggedCborSerializable: AsCborValue {
    /// The associated tag value.
    const TAG: u64;

    /// Create an object instance from serialized CBOR data in a slice, expecting an initial
    /// tag value.
    fn from_tagged_slice(slice: &[u8]) -> Result<Self, CoseError> {
        match sk_cbor::reader::read(slice)? {
            Value::Tag(t, v) if t == Self::TAG => Self::from_cbor_value(*v),
            v => cbor_type_error(&v, "tag"),
        }
    }

    /// Serialize this object to a vector, including initial tag, consuming the object along the
    /// way.
    fn to_tagged_vec(self) -> Result<Vec<u8>, CoseError> {
        let mut data = Vec::new();
        sk_cbor::writer::write(
            Value::Tag(Self::TAG, Box::new(self.to_cbor_value()?)),
            &mut data,
        )?;
        Ok(data)
    }
}

/// Algorithm identifier.
pub type Algorithm = crate::RegisteredLabelWithPrivate<iana::Algorithm>;

impl Default for Algorithm {
    fn default() -> Self {
        Algorithm::Assigned(iana::Algorithm::Reserved)
    }
}

/// A COSE label may be either a signed integer value or a string.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Label {
    Int(i64),
    Text(String),
}

impl CborSerializable for Label {}

/// Manual implementation of [`Ord`] to ensure that CBOR canonical ordering is respected.
///
/// Note that this uses the ordering given by RFC 8949 section 4.2.1 (lexicographic ordering of
/// encoded form), which is *different* from the canonical ordering defined in RFC 7049 section 3.9
/// (where the primary sorting criterion is the length of the encoded form)
impl Ord for Label {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Label::Int(i1), Label::Int(i2)) => match (i1.signum(), i2.signum()) {
                (-1, -1) => (-i1).cmp(&(-i2)),
                (-1, 0) => Ordering::Greater,
                (-1, 1) => Ordering::Greater,
                (0, -1) => Ordering::Less,
                (0, 0) => Ordering::Equal,
                (0, 1) => Ordering::Less,
                (1, -1) => Ordering::Less,
                (1, 0) => Ordering::Greater,
                (1, 1) => i1.cmp(i2),
                (_, _) => unreachable!(), // safe: all possibilies covered
            },
            (Label::Int(_i1), Label::Text(_t2)) => Ordering::Less,
            (Label::Text(_t1), Label::Int(_i2)) => Ordering::Greater,
            (Label::Text(t1), Label::Text(t2)) => t1.len().cmp(&t2.len()).then(t1.cmp(t2)),
        }
    }
}

impl PartialOrd for Label {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl AsCborValue for Label {
    fn from_cbor_value(value: Value) -> Result<Self, CoseError> {
        match value {
            Value::Unsigned(u) => Ok(Label::Int(
                u.try_into()
                    .map_err(|_e| CoseError::UnexpectedType("u64", "u63"))?,
            )),
            Value::Negative(i) => Ok(Label::Int(i)),
            Value::TextString(t) => Ok(Label::Text(t)),
            v => cbor_type_error(&v, "int/tstr"),
        }
    }
    fn to_cbor_value(self) -> Result<Value, CoseError> {
        Ok(match self {
            Label::Int(i) if i < 0 => Value::Negative(i),
            Label::Int(i) => Value::Unsigned(i as u64), // safe: i64 value that is >=0 fits in u64
            Label::Text(t) => Value::TextString(t),
        })
    }
}

/// A COSE label which can be either a signed integer value or a string, but
/// where the allowed integer values are governed by IANA.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RegisteredLabel<T: EnumI64> {
    Assigned(T),
    Text(String),
}

impl<T: EnumI64> CborSerializable for RegisteredLabel<T> {}

/// Manual implementation of [`Ord`] to ensure that CBOR canonical ordering is respected.
impl<T: EnumI64> Ord for RegisteredLabel<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (RegisteredLabel::Assigned(i1), RegisteredLabel::Assigned(i2)) => {
                Label::Int(i1.to_i64()).cmp(&Label::Int(i2.to_i64()))
            }
            (RegisteredLabel::Assigned(_i1), RegisteredLabel::Text(_t2)) => Ordering::Less,
            (RegisteredLabel::Text(_t1), RegisteredLabel::Assigned(_i2)) => Ordering::Greater,
            (RegisteredLabel::Text(t1), RegisteredLabel::Text(t2)) => {
                t1.len().cmp(&t2.len()).then(t1.cmp(t2))
            }
        }
    }
}

impl<T: EnumI64> PartialOrd for RegisteredLabel<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: EnumI64> AsCborValue for RegisteredLabel<T> {
    fn from_cbor_value(value: Value) -> Result<Self, CoseError> {
        match value {
            Value::Unsigned(u) => {
                let i: i64 = u
                    .try_into()
                    .map_err(|_e| CoseError::UnexpectedType("u64", "u63"))?;
                if let Some(a) = T::from_i64(i) {
                    Ok(RegisteredLabel::Assigned(a))
                } else {
                    Err(CoseError::UnregisteredIanaValue)
                }
            }
            Value::Negative(i) => {
                if let Some(a) = T::from_i64(i) {
                    Ok(RegisteredLabel::Assigned(a))
                } else {
                    Err(CoseError::UnregisteredIanaValue)
                }
            }
            Value::TextString(t) => Ok(RegisteredLabel::Text(t)),
            v => cbor_type_error(&v, "int/tstr"),
        }
    }
    fn to_cbor_value(self) -> Result<Value, CoseError> {
        Ok(match self {
            RegisteredLabel::Assigned(e) => {
                let e64 = e.to_i64();
                if e64 >= 0 {
                    Value::Unsigned(e64 as u64) // safe: i64 value that is >= 0 fits in u64
                } else {
                    Value::Negative(e64)
                }
            }
            RegisteredLabel::Text(t) => Value::TextString(t),
        })
    }
}

/// A COSE label which can be either a signed integer value or a string, and
/// where the allowed integer values are governed by IANA but include a private
/// use range.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RegisteredLabelWithPrivate<T: EnumI64 + WithPrivateRange> {
    PrivateUse(i64),
    Assigned(T),
    Text(String),
}

impl<T: EnumI64 + WithPrivateRange> CborSerializable for RegisteredLabelWithPrivate<T> {}

/// Manual implementation of [`Ord`] to ensure that CBOR canonical ordering is respected.
impl<T: EnumI64 + WithPrivateRange> Ord for RegisteredLabelWithPrivate<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        use RegisteredLabelWithPrivate::{Assigned, PrivateUse, Text};
        match (self, other) {
            (Assigned(i1), Assigned(i2)) => Label::Int(i1.to_i64()).cmp(&Label::Int(i2.to_i64())),
            (Assigned(i1), PrivateUse(i2)) => Label::Int(i1.to_i64()).cmp(&Label::Int(*i2)),
            (PrivateUse(i1), Assigned(i2)) => Label::Int(*i1).cmp(&Label::Int(i2.to_i64())),
            (PrivateUse(i1), PrivateUse(i2)) => Label::Int(*i1).cmp(&Label::Int(*i2)),
            (Assigned(_i1), Text(_t2)) => Ordering::Less,
            (PrivateUse(_i1), Text(_t2)) => Ordering::Less,
            (Text(_t1), Assigned(_i2)) => Ordering::Greater,
            (Text(_t1), PrivateUse(_i2)) => Ordering::Greater,
            (Text(t1), Text(t2)) => t1.len().cmp(&t2.len()).then(t1.cmp(t2)),
        }
    }
}

impl<T: EnumI64 + WithPrivateRange> PartialOrd for RegisteredLabelWithPrivate<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: EnumI64 + WithPrivateRange> AsCborValue for RegisteredLabelWithPrivate<T> {
    fn from_cbor_value(value: Value) -> Result<Self, CoseError> {
        match value {
            Value::Unsigned(u) => {
                let i = u
                    .try_into()
                    .map_err(|_e| CoseError::UnexpectedType("u64", "u63"))?;
                if let Some(a) = T::from_i64(i) {
                    Ok(RegisteredLabelWithPrivate::Assigned(a))
                } else if T::is_private(i) {
                    Ok(RegisteredLabelWithPrivate::PrivateUse(i))
                } else {
                    Err(CoseError::UnregisteredIanaNonPrivateValue)
                }
            }
            Value::Negative(i) => {
                if let Some(a) = T::from_i64(i) {
                    Ok(RegisteredLabelWithPrivate::Assigned(a))
                } else if T::is_private(i) {
                    Ok(RegisteredLabelWithPrivate::PrivateUse(i))
                } else {
                    Err(CoseError::UnregisteredIanaNonPrivateValue)
                }
            }
            Value::TextString(t) => Ok(RegisteredLabelWithPrivate::Text(t)),
            v => cbor_type_error(&v, "int/tstr"),
        }
    }
    fn to_cbor_value(self) -> Result<Value, CoseError> {
        Ok(match self {
            RegisteredLabelWithPrivate::PrivateUse(i) => {
                if i >= 0 {
                    Value::Unsigned(i as u64) // safe: i64 value that is >=0 fits in u64
                } else {
                    Value::Negative(i)
                }
            }
            RegisteredLabelWithPrivate::Assigned(i) => {
                let i = i.to_i64();
                if i >= 0 {
                    Value::Unsigned(i as u64) // safe: i64 value that is >=0 fits in u64
                } else {
                    Value::Negative(i)
                }
            }
            RegisteredLabelWithPrivate::Text(t) => Value::TextString(t),
        })
    }
}
