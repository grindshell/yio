//! The values a script computes with.

use std::fmt;
use std::rc::Rc;

use super::eval::{Interp, Scope};
use super::reader::{Error, Form, Pos};

/// A value. Cloning is cheap: everything larger than a word is shared.
///
/// There is no separate "nothing" value. `()` is the empty list, and it is
/// what a form with no useful result evaluates to.
#[derive(Clone)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Str(Rc<str>),
    Bytes(Rc<[u8]>),
    List(Rc<[Value]>),
    /// One column of a table: what `col-def` makes.
    Column(Rc<Column>),
    /// A table for yio to draw: what `col` makes.
    Table(Rc<[Rc<Column>]>),
    Fn(Rc<Function>),
    Native(&'static Builtin),
}

#[derive(PartialEq)]
pub struct Column {
    pub header: Rc<str>,
    pub cells: Rc<[Value]>,
}

/// A function written in a script.
pub struct Function {
    /// For error messages. Anonymous functions have none.
    pub name: Option<String>,
    pub params: Vec<String>,
    pub body: Vec<Form>,
    /// The scope the function was defined in, which its body can see.
    pub scope: Rc<Scope>,
}

/// A function implemented in Rust.
pub struct Builtin {
    pub name: &'static str,
    /// Receives the already evaluated arguments and the position of the
    /// call, for error messages.
    pub call: fn(&mut Interp, &[Value], Pos) -> Result<Value, Error>,
}

impl Value {
    /// The empty value, `()`.
    pub fn nil() -> Value {
        Value::List(Rc::from([]))
    }

    pub fn str(text: &str) -> Value {
        Value::Str(Rc::from(text))
    }

    pub fn list(items: Vec<Value>) -> Value {
        Value::List(Rc::from(items))
    }

    /// Only `false` and `()` count as false.
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Bool(value) => *value,
            Value::List(items) => !items.is_empty(),
            _ => true,
        }
    }

    /// What to call this kind of value in an error message.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Bool(_) => "a boolean",
            Value::Int(_) => "an integer",
            Value::Str(_) => "a string",
            Value::Bytes(_) => "bytes",
            Value::List(items) if items.is_empty() => "the empty value",
            Value::List(_) => "a list",
            Value::Column(_) => "a column",
            Value::Table(_) => "a table",
            Value::Fn(_) | Value::Native(_) => "a function",
        }
    }
}

/// Structural equality. Functions are equal only to themselves.
impl PartialEq for Value {
    fn eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Bytes(a), Value::Bytes(b)) => a == b,
            (Value::List(a), Value::List(b)) => a == b,
            (Value::Column(a), Value::Column(b)) => a == b,
            (Value::Table(a), Value::Table(b)) => a == b,
            (Value::Fn(a), Value::Fn(b)) => Rc::ptr_eq(a, b),
            (Value::Native(a), Value::Native(b)) => std::ptr::eq(*a, *b),
            _ => false,
        }
    }
}

/// Values print the way they would be written in a script, where that is
/// possible.
impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Value::Bool(value) => write!(f, "{value}"),
            Value::Int(value) => write!(f, "{value}"),
            Value::Str(text) => {
                f.write_str("\"")?;
                for c in text.chars() {
                    match c {
                        '\n' => f.write_str("\\n")?,
                        '\t' => f.write_str("\\t")?,
                        '\r' => f.write_str("\\r")?,
                        '"' => f.write_str("\\\"")?,
                        '\\' => f.write_str("\\\\")?,
                        c => write!(f, "{c}")?,
                    }
                }
                f.write_str("\"")
            }
            Value::Bytes(bytes) => write!(f, "<{} bytes>", bytes.len()),
            Value::List(items) => write_list(f, items),
            Value::Column(column) => write_column(f, column),
            Value::Table(columns) => {
                f.write_str("(col")?;
                for column in columns.iter() {
                    f.write_str(" ")?;
                    write_column(f, column)?;
                }
                f.write_str(")")
            }
            Value::Fn(function) => match &function.name {
                Some(name) => write!(f, "<fn {name}>"),
                None => f.write_str("<fn>"),
            },
            Value::Native(builtin) => write!(f, "<fn {}>", builtin.name),
        }
    }
}

fn write_list(f: &mut fmt::Formatter, items: &[Value]) -> fmt::Result {
    f.write_str("(")?;
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            f.write_str(" ")?;
        }
        write!(f, "{item}")?;
    }
    f.write_str(")")
}

fn write_column(f: &mut fmt::Formatter, column: &Column) -> fmt::Result {
    write!(f, "(col-def {} ", Value::Str(Rc::clone(&column.header)))?;
    write_list(f, &column.cells)?;
    f.write_str(")")
}

// A derived `Debug` would follow a function into its scope, and from there
// back into the function.
impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{self}")
    }
}
