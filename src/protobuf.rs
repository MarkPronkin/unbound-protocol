//! Small, bounded protobuf wire codec for One property envelopes. This is not a
//! generated vendor API. Unknown fields are parsed and can be inspected.
use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value<'a> {
    Varint(u64),
    Fixed64(&'a [u8]),
    Bytes(&'a [u8]),
    Fixed32(&'a [u8]),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field<'a> {
    pub number: u32,
    pub value: Value<'a>,
}

fn varint(data: &[u8], pos: &mut usize) -> Result<u64> {
    let mut out = 0u64;
    for i in 0..10 {
        let byte = *data
            .get(*pos)
            .ok_or(Error::Invalid("truncated protobuf varint"))?;
        *pos += 1;
        if i == 9 && byte > 1 {
            return Err(Error::Invalid("protobuf varint overflow"));
        }
        out |= u64::from(byte & 0x7f) << (i * 7);
        if byte & 0x80 == 0 {
            return Ok(out);
        }
    }
    Err(Error::Invalid("protobuf varint overflow"))
}
fn take<'a>(data: &'a [u8], pos: &mut usize, len: usize) -> Result<&'a [u8]> {
    let end = pos
        .checked_add(len)
        .ok_or(Error::Invalid("protobuf length overflow"))?;
    let bytes = data
        .get(*pos..end)
        .ok_or(Error::Invalid("truncated protobuf field"))?;
    *pos = end;
    Ok(bytes)
}
pub fn fields(data: &[u8]) -> Result<Vec<Field<'_>>> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
        if out.len() >= 4096 {
            return Err(Error::TooLarge {
                size: out.len() + 1,
                limit: 4096,
            });
        }
        let tag = varint(data, &mut pos)?;
        let number = tag >> 3;
        if number == 0 || number > 0x1fff_ffff {
            return Err(Error::Invalid("protobuf field number"));
        }
        let value = match tag & 7 {
            0 => Value::Varint(varint(data, &mut pos)?),
            1 => Value::Fixed64(take(data, &mut pos, 8)?),
            2 => {
                let len = usize::try_from(varint(data, &mut pos)?)
                    .map_err(|_| Error::Invalid("protobuf length"))?;
                Value::Bytes(take(data, &mut pos, len)?)
            }
            5 => Value::Fixed32(take(data, &mut pos, 4)?),
            _ => return Err(Error::Invalid("unsupported protobuf wire type")),
        };
        out.push(Field {
            number: number as u32,
            value,
        });
    }
    Ok(out)
}
fn put_varint(mut n: u64, out: &mut Vec<u8>) {
    while n >= 128 {
        out.push((n as u8 & 0x7f) | 0x80);
        n >>= 7;
    }
    out.push(n as u8);
}
pub fn get_request() -> Vec<u8> {
    vec![0x18, 0]
}
pub fn set_request(value: u64) -> Vec<u8> {
    let mut inner = vec![0x08];
    put_varint(value, &mut inner);
    let mut out = vec![0x1a];
    put_varint(inner.len() as u64, &mut out);
    out.extend(inner);
    out
}

/// Returns response field 4, rejecting duplicates and malformed outer fields.
/// Other outer fields remain available through `fields` for protocol research.
pub fn response_payload(data: &[u8]) -> Result<&[u8]> {
    let mut result = None;
    for field in fields(data)? {
        if field.number == 4 {
            if result.is_some() {
                return Err(Error::Invalid("duplicate property response"));
            }
            if let Value::Bytes(bytes) = field.value {
                result = Some(bytes);
            } else {
                return Err(Error::Invalid("property response wire type"));
            }
        }
    }
    result.ok_or(Error::Invalid("missing property response field 4"))
}
pub fn numeric_response(data: &[u8]) -> Result<u64> {
    let inner = response_payload(data)?;
    let mut result = None;
    for field in fields(inner)? {
        if field.number == 2 {
            if result.is_some() {
                return Err(Error::Invalid("duplicate property value"));
            }
            if let Value::Varint(value) = field.value {
                result = Some(value);
            } else {
                return Err(Error::Invalid("numeric property wire type"));
            }
        }
    }
    // Protobuf scalar zero may be omitted entirely.
    if inner.is_empty() {
        Ok(0)
    } else {
        result.ok_or(Error::Invalid("missing numeric property"))
    }
}
pub fn string_response(data: &[u8]) -> Result<String> {
    let inner = response_payload(data)?;
    let mut result = None;
    for field in fields(inner)? {
        if field.number == 2 {
            if result.is_some() {
                return Err(Error::Invalid("duplicate property value"));
            }
            if let Value::Bytes(value) = field.value {
                result = Some(value);
            } else {
                return Err(Error::Invalid("string property wire type"));
            }
        }
    }
    let bytes = if inner.is_empty() {
        &[][..]
    } else {
        result.ok_or(Error::Invalid("missing string property"))?
    };
    String::from_utf8(bytes.to_vec()).map_err(|_| Error::Invalid("property string is not UTF-8"))
}
pub fn acknowledgement(data: &[u8]) -> Result<()> {
    if response_payload(data)?.is_empty() {
        Ok(())
    } else {
        Err(Error::Invalid("unexpected setter acknowledgement"))
    }
}
