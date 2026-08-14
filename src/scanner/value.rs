use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::ScalarValue;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueType {
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    F32,
    F64,
}

impl ValueType {
    pub const fn byte_width(self) -> usize {
        match self {
            Self::I8 | Self::U8 => 1,
            Self::I16 | Self::U16 => 2,
            Self::I32 | Self::U32 | Self::F32 => 4,
            Self::I64 | Self::U64 | Self::F64 => 8,
        }
    }

    pub fn decode(self, bytes: &[u8]) -> Result<ScalarValue, ValueError> {
        let expected = self.byte_width();
        if bytes.len() != expected {
            return Err(ValueError::WrongWidth {
                value_type: self,
                expected,
                actual: bytes.len(),
            });
        }

        Ok(match self {
            Self::I8 => ScalarValue::Signed(i8::from_le_bytes([bytes[0]]) as i64),
            Self::I16 => ScalarValue::Signed(i16::from_le_bytes(copy_array(bytes)) as i64),
            Self::I32 => ScalarValue::Signed(i32::from_le_bytes(copy_array(bytes)) as i64),
            Self::I64 => ScalarValue::Signed(i64::from_le_bytes(copy_array(bytes))),
            Self::U8 => ScalarValue::Unsigned(u8::from_le_bytes([bytes[0]]) as u64),
            Self::U16 => ScalarValue::Unsigned(u16::from_le_bytes(copy_array(bytes)) as u64),
            Self::U32 => ScalarValue::Unsigned(u32::from_le_bytes(copy_array(bytes)) as u64),
            Self::U64 => ScalarValue::Unsigned(u64::from_le_bytes(copy_array(bytes))),
            Self::F32 => ScalarValue::Float(f32::from_le_bytes(copy_array(bytes)) as f64),
            Self::F64 => ScalarValue::Float(f64::from_le_bytes(copy_array(bytes))),
        })
    }

    pub fn encode(self, value: ScalarValue) -> Result<Vec<u8>, ValueError> {
        match (self, value) {
            (Self::I8, ScalarValue::Signed(value)) => i8::try_from(value)
                .map(|value| value.to_le_bytes().to_vec())
                .map_err(|_| ValueError::OutOfRange(self)),
            (Self::I16, ScalarValue::Signed(value)) => i16::try_from(value)
                .map(|value| value.to_le_bytes().to_vec())
                .map_err(|_| ValueError::OutOfRange(self)),
            (Self::I32, ScalarValue::Signed(value)) => i32::try_from(value)
                .map(|value| value.to_le_bytes().to_vec())
                .map_err(|_| ValueError::OutOfRange(self)),
            (Self::I64, ScalarValue::Signed(value)) => Ok(value.to_le_bytes().to_vec()),
            (Self::U8, ScalarValue::Unsigned(value)) => u8::try_from(value)
                .map(|value| value.to_le_bytes().to_vec())
                .map_err(|_| ValueError::OutOfRange(self)),
            (Self::U16, ScalarValue::Unsigned(value)) => u16::try_from(value)
                .map(|value| value.to_le_bytes().to_vec())
                .map_err(|_| ValueError::OutOfRange(self)),
            (Self::U32, ScalarValue::Unsigned(value)) => u32::try_from(value)
                .map(|value| value.to_le_bytes().to_vec())
                .map_err(|_| ValueError::OutOfRange(self)),
            (Self::U64, ScalarValue::Unsigned(value)) => Ok(value.to_le_bytes().to_vec()),
            (Self::F32, ScalarValue::Float(value)) => Ok((value as f32).to_le_bytes().to_vec()),
            (Self::F64, ScalarValue::Float(value)) => Ok(value.to_le_bytes().to_vec()),
            (value_type, value) => Err(ValueError::KindMismatch {
                value_type,
                actual: scalar_kind(value),
            }),
        }
    }
}

fn copy_array<const N: usize>(bytes: &[u8]) -> [u8; N] {
    let mut array = [0_u8; N];
    array.copy_from_slice(bytes);
    array
}

fn scalar_kind(value: ScalarValue) -> &'static str {
    match value {
        ScalarValue::Signed(_) => "signed",
        ScalarValue::Unsigned(_) => "unsigned",
        ScalarValue::Float(_) => "float",
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ValueError {
    #[error("{value_type:?} requires {expected} bytes, got {actual}")]
    WrongWidth {
        value_type: ValueType,
        expected: usize,
        actual: usize,
    },
    #[error("value is outside the representable range for {0:?}")]
    OutOfRange(ValueType),
    #[error("{value_type:?} cannot encode a {actual} scalar")]
    KindMismatch {
        value_type: ValueType,
        actual: &'static str,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_widths_round_trip() {
        let cases = [
            (ValueType::I8, ScalarValue::Signed(-7)),
            (ValueType::I16, ScalarValue::Signed(-700)),
            (ValueType::I32, ScalarValue::Signed(-70_000)),
            (ValueType::I64, ScalarValue::Signed(-7_000_000_000)),
            (ValueType::U8, ScalarValue::Unsigned(7)),
            (ValueType::U16, ScalarValue::Unsigned(700)),
            (ValueType::U32, ScalarValue::Unsigned(70_000)),
            (ValueType::U64, ScalarValue::Unsigned(7_000_000_000)),
        ];

        for (value_type, value) in cases {
            let encoded = value_type.encode(value).expect("value should encode");
            assert_eq!(value_type.decode(&encoded).unwrap(), value);
        }
    }

    #[test]
    fn float_types_decode_little_endian_values() {
        let f32_value = ValueType::F32
            .decode(&12.5_f32.to_le_bytes())
            .expect("f32 should decode");
        let f64_value = ValueType::F64
            .decode(&(-8.25_f64).to_le_bytes())
            .expect("f64 should decode");

        assert_eq!(f32_value, ScalarValue::Float(12.5));
        assert_eq!(f64_value, ScalarValue::Float(-8.25));
    }

    #[test]
    fn narrow_integer_overflow_is_rejected() {
        assert_eq!(
            ValueType::I8.encode(ScalarValue::Signed(128)).unwrap_err(),
            ValueError::OutOfRange(ValueType::I8)
        );
    }
}
