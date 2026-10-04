//! Integer-only, little-endian v1 records. Floats are not an ABI value type.
use reprise_diag::Note;
use reprise_doc::expr::{Dim, Value};
use reprise_geom::{Fixed, Length};

use crate::codes;

pub const VALUE_BYTES: usize = 12;
pub const MAX_ARGUMENTS: usize = 256;
pub const MAX_INTERVALS: usize = 64;

pub(crate) fn bad() -> Note {
    Note::warning(codes::RESULT, "malformed integer ABI record")
}

pub fn read_i32(bytes: &[u8], at: usize) -> Result<i32, Note> {
    let end = at.checked_add(4).ok_or_else(bad)?;
    let word: [u8; 4] = bytes
        .get(at..end)
        .ok_or_else(bad)?
        .try_into()
        .map_err(|_| bad())?;
    Ok(i32::from_le_bytes(word))
}

pub fn push_i32(bytes: &mut Vec<u8>, value: i32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

pub fn encode_value(value: Value) -> Vec<u8> {
    let (tag, a, b) = match value {
        Value::Length(v) => (1, v.0, 0),
        Value::Number(v) => (2, v.0, 0),
        Value::Percentage(v) => (3, v.0, 0),
        Value::Ratio { num, den } => (4, num.0, den.0),
    };
    let mut bytes = Vec::with_capacity(VALUE_BYTES);
    for v in [tag, a, b] {
        push_i32(&mut bytes, v);
    }
    bytes
}

pub fn decode_value(bytes: &[u8], expected: Dim) -> Result<Value, Note> {
    if bytes.len() != VALUE_BYTES {
        return Err(bad());
    }
    let a = read_i32(bytes, 4)?;
    let b = read_i32(bytes, 8)?;
    let v = match read_i32(bytes, 0)? {
        1 if b == 0 => Value::Length(Length(a)),
        2 if b == 0 => Value::Number(Fixed(a)),
        3 if b == 0 => Value::Percentage(Fixed(a)),
        4 if b != 0 => Value::Ratio {
            num: Fixed(a),
            den: Fixed(b),
        },
        _ => return Err(bad()),
    };
    if v.dim() != expected {
        return Err(bad());
    }
    Ok(v)
}

pub fn encode_arguments(args: &[Value]) -> Result<Vec<u8>, Note> {
    if args.len() > MAX_ARGUMENTS {
        return Err(Note::warning(codes::LIMIT, "too many ABI arguments"));
    }
    let mut bytes = Vec::with_capacity(4 + args.len() * VALUE_BYTES);
    push_i32(&mut bytes, args.len() as i32);
    for arg in args {
        bytes.extend_from_slice(&encode_value(*arg));
    }
    Ok(bytes)
}
