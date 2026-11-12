//! Stores a script's value as bytes, and reads it back.
//!
//! This is how the result of a script is kept in the cache file, so that
//! while its inputs are unchanged the value can be shown again without
//! running the script. Each value is one tag byte followed by its payload,
//! in the encoding of `crate::bytes`.

use std::rc::Rc;

use super::value::{Column, Value};
use crate::bytes::{Reader, put_bytes, put_len, put_str, put_u8, put_u64};

const BOOL: u8 = 1;
const INT: u8 = 2;
const STR: u8 = 3;
const BYTES: u8 = 4;
const LIST: u8 = 5;
const COLUMN: u8 = 6;
const TABLE: u8 = 7;

/// How deeply values may nest and still be stored. A value nested further
/// is simply not cached, and its script runs every time.
const MAX_DEPTH: usize = 256;

/// Encodes a value. Fails for a value that cannot be stored: one containing
/// a function, or nested too deeply.
pub fn encode(value: &Value) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    put(&mut out, value, 0)?;
    Ok(out)
}

fn put(out: &mut Vec<u8>, value: &Value, depth: usize) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err("the value is nested too deeply to store".to_string());
    }
    match value {
        Value::Bool(value) => {
            put_u8(out, BOOL);
            put_u8(out, u8::from(*value));
        }
        Value::Int(value) => {
            put_u8(out, INT);
            put_u64(out, *value as u64);
        }
        Value::Str(text) => {
            put_u8(out, STR);
            put_str(out, text);
        }
        Value::Bytes(bytes) => {
            put_u8(out, BYTES);
            put_bytes(out, bytes);
        }
        Value::List(items) => {
            put_u8(out, LIST);
            put_items(out, items, depth)?;
        }
        Value::Column(column) => {
            put_u8(out, COLUMN);
            put_column(out, column, depth)?;
        }
        Value::Table(columns) => {
            put_u8(out, TABLE);
            put_len(out, columns.len());
            for column in columns.iter() {
                put_column(out, column, depth)?;
            }
        }
        Value::Fn(_) | Value::Native(_) => {
            return Err("a function cannot be stored".to_string());
        }
    }
    Ok(())
}

fn put_items(out: &mut Vec<u8>, items: &[Value], depth: usize) -> Result<(), String> {
    put_len(out, items.len());
    items.iter().try_for_each(|item| put(out, item, depth + 1))
}

fn put_column(out: &mut Vec<u8>, column: &Column, depth: usize) -> Result<(), String> {
    put_str(out, &column.header);
    put_items(out, &column.cells, depth)
}

/// Decodes what `encode` produced.
pub fn decode(bytes: &[u8]) -> Result<Value, String> {
    let mut reader = Reader::new(bytes);
    let value = take(&mut reader, 0)?;
    if !reader.is_empty() {
        return Err("cache has data after a script's value".to_string());
    }
    Ok(value)
}

fn take(reader: &mut Reader, depth: usize) -> Result<Value, String> {
    if depth > MAX_DEPTH {
        return Err("cache nests a script's value too deeply".to_string());
    }
    Ok(match reader.u8()? {
        BOOL => Value::Bool(reader.u8()? != 0),
        INT => Value::Int(reader.u64()? as i64),
        STR => Value::str(reader.str()?),
        BYTES => Value::Bytes(Rc::from(reader.bytes()?)),
        LIST => Value::List(take_items(reader, depth)?),
        COLUMN => Value::Column(take_column(reader, depth)?),
        TABLE => {
            let mut columns = Vec::new();
            for _ in 0..reader.u32()? {
                columns.push(take_column(reader, depth)?);
            }
            Value::Table(Rc::from(columns))
        }
        _ => return Err("cache has an unknown kind of value".to_string()),
    })
}

fn take_items(reader: &mut Reader, depth: usize) -> Result<Rc<[Value]>, String> {
    let mut items = Vec::new();
    for _ in 0..reader.u32()? {
        items.push(take(reader, depth + 1)?);
    }
    Ok(Rc::from(items))
}

fn take_column(reader: &mut Reader, depth: usize) -> Result<Rc<Column>, String> {
    let header = Rc::from(reader.str()?);
    Ok(Rc::new(Column { header, cells: take_items(reader, depth)? }))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::script;

    /// Runs a script, stores its value, reads it back, and prints both.
    fn round_trip(source: &str) -> (String, Result<String, String>) {
        script::run(source.as_bytes(), Path::new(""), &[], |outcome| {
            let stored = encode(&outcome.value);
            let restored = stored.map(|bytes| {
                let value = decode(&bytes).unwrap();
                assert_eq!(value, outcome.value, "{source}");
                value.to_string()
            });
            (outcome.value.to_string(), restored)
        })
        .unwrap()
    }

    #[test]
    fn every_kind_of_value_survives_a_round_trip() {
        for source in [
            "()",
            "true",
            "false",
            "0",
            "-1",
            "9223372036854775807",
            "(- -9223372036854775807 1)",
            "\"\"",
            "\"caf\u{e9}\\n\\\"quoted\\\"\"",
            "#0:",
            "#4:\x00\x01\x02\x7f",
            "(list 1 \"two\" (list 3 ()) true #1:x)",
            "(col-def \"Header\" (list 1 \"x\" ()))",
            "(col (col-def \"A\" (list 1 2)) (col-def \"B\" ()))",
            "(list \"title\" (col (col-def \"A\" (list (list 1 2)))))",
        ] {
            let (original, restored) = round_trip(source);
            assert_eq!(restored.as_ref(), Ok(&original), "{source}");
        }
    }

    #[test]
    fn a_function_cannot_be_stored() {
        for source in ["(fn (x) x)", "+", "(list 1 (list trim))", "(col (col-def \"F\" (list len)))"] {
            assert_eq!(round_trip(source).1, Err("a function cannot be stored".to_string()), "{source}");
        }
    }

    #[test]
    fn a_value_nested_too_deeply_is_not_stored() {
        // Built by a loop, not by recursion, so the evaluator's own depth
        // limit does not come into it.
        let nested = |levels: usize| {
            let items = vec!["0"; levels].join(" ");
            format!("(var x 1) (map (list {items}) (fn (i) (set x (list x)))) x")
        };
        assert!(round_trip(&nested(MAX_DEPTH)).1.is_ok());
        assert_eq!(
            round_trip(&nested(MAX_DEPTH + 1)).1,
            Err("the value is nested too deeply to store".to_string())
        );
    }

    #[test]
    fn damaged_bytes_are_rejected_without_panicking() {
        let bytes = script::run(
            br#"(list 1 "two" (col (col-def "A" (list true #2:xy))))"#,
            Path::new(""),
            &[],
            |outcome| encode(&outcome.value).unwrap(),
        )
        .unwrap();
        for length in 0..bytes.len() {
            assert!(decode(&bytes[..length]).is_err(), "prefix of {length} bytes");
        }
        let mut extra = bytes.clone();
        extra.push(0);
        assert_eq!(decode(&extra).unwrap_err(), "cache has data after a script's value");
        assert_eq!(decode(&[99]).unwrap_err(), "cache has an unknown kind of value");

        // A list that claims to hold itself, without end.
        let mut deep = Vec::new();
        for _ in 0..MAX_DEPTH + 2 {
            deep.extend_from_slice(&[LIST, 1, 0, 0, 0]);
        }
        assert_eq!(decode(&deep).unwrap_err(), "cache nests a script's value too deeply");
    }
}
