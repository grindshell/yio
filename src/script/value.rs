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
    /// Something else for yio to draw, built out of other values: what
    /// `box`, `row`, `style`, and the like make.
    View(Rc<View>),
    Fn(Rc<Function>),
    Native(&'static Builtin),
}

#[derive(PartialEq)]
pub struct Column {
    pub header: Rc<str>,
    pub cells: Rc<[Value]>,
}

/// The kinds of thing a script can ask to have drawn, besides a table.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ViewKind {
    /// A value with its text in a style: bold, a colour.
    Style,
    /// A value with a border around it, and maybe a title.
    Box,
    /// Values side by side.
    Row,
    /// A line as wide as what it sits among.
    Rule,
    /// How much of a whole is done, as a bar.
    Bar,
    /// A label with what is under it drawn as branches.
    Tree,
    /// Labels, each with a value beside it.
    Fields,
}

impl ViewKind {
    pub const ALL: [ViewKind; 7] = [
        ViewKind::Style,
        ViewKind::Box,
        ViewKind::Row,
        ViewKind::Rule,
        ViewKind::Bar,
        ViewKind::Tree,
        ViewKind::Fields,
    ];

    /// The built-in that makes it.
    pub fn name(self) -> &'static str {
        match self {
            ViewKind::Style => "style",
            ViewKind::Box => "box",
            ViewKind::Row => "row",
            ViewKind::Rule => "rule",
            ViewKind::Bar => "bar",
            ViewKind::Tree => "tree",
            ViewKind::Fields => "fields",
        }
    }
}

/// The widest a `bar` may be asked to be.
pub const MAX_BAR_WIDTH: i64 = 200;

/// One thing to draw. `parts` are the arguments its built-in was given,
/// which `View::new` has checked, so that drawing can rely on their shape.
#[derive(PartialEq)]
pub struct View {
    pub kind: ViewKind,
    pub parts: Rc<[Value]>,
}

impl View {
    /// Checks the arguments of the built-in for `kind`. The same check runs
    /// on a view read back from the cache, so a damaged one is not drawn.
    pub fn new(kind: ViewKind, parts: Vec<Value>) -> Result<View, String> {
        let name = kind.name();
        let count = |allowed: &str| {
            format!("`{name}` takes {allowed} but was given {}", parts.len())
        };
        let wrong = |wanted: &str, found: &Value| {
            format!("`{name}` needs {wanted}, but was given {}", found.type_name())
        };
        match (kind, parts.as_slice()) {
            (ViewKind::Style, [_, names @ ..]) if !names.is_empty() => {
                for style in names {
                    match style {
                        Value::Str(style) if super::render::is_style(style) => {}
                        Value::Str(style) => {
                            let known = super::render::STYLES.join("`, `");
                            return Err(format!(
                                "`style` does not know the style `{style}`; the styles are `{known}`"
                            ));
                        }
                        other => return Err(wrong("the name of a style", other)),
                    }
                }
            }
            (ViewKind::Style, _) => return Err(count("a value and at least 1 style")),
            (ViewKind::Box, [_]) | (ViewKind::Box, [_, Value::Str(_)]) => {}
            (ViewKind::Box, [_, title]) => return Err(wrong("a string for its title", title)),
            (ViewKind::Box, _) => return Err(count("1 or 2 arguments")),
            (ViewKind::Row, []) => return Err(count("at least 1 argument")),
            (ViewKind::Row, _) => {}
            (ViewKind::Rule, []) => {}
            (ViewKind::Rule, _) => return Err(count("0 arguments")),
            (ViewKind::Bar, [part, whole, width @ ..]) if width.len() <= 1 => {
                let integer = |value: &Value| match value {
                    Value::Int(n) => Ok(*n),
                    other => Err(wrong("an integer", other)),
                };
                integer(part)?;
                if integer(whole)? <= 0 {
                    return Err("`bar` needs a whole that is more than 0".to_string());
                }
                if let [width] = width
                    && !(1..=MAX_BAR_WIDTH).contains(&integer(width)?)
                {
                    return Err(format!("`bar` needs a width from 1 to {MAX_BAR_WIDTH}"));
                }
            }
            (ViewKind::Bar, _) => return Err(count("2 or 3 arguments")),
            (ViewKind::Tree, [_, Value::List(_)]) => {}
            (ViewKind::Tree, [_, children]) => {
                return Err(wrong("a list of what is under the label", children));
            }
            (ViewKind::Tree, _) => return Err(count("2 arguments")),
            (ViewKind::Fields, [Value::List(_), Value::List(_)]) => {}
            (ViewKind::Fields, [labels, values]) => {
                let other = if matches!(labels, Value::List(_)) { values } else { labels };
                return Err(wrong("a list of labels and a list of values", other));
            }
            (ViewKind::Fields, _) => return Err(count("2 arguments")),
        }
        Ok(View { kind, parts: Rc::from(parts) })
    }
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
            Value::View(view) => match view.kind {
                ViewKind::Style => "styled text",
                ViewKind::Box => "a box",
                ViewKind::Row => "a row",
                ViewKind::Rule => "a rule",
                ViewKind::Bar => "a bar",
                ViewKind::Tree => "a tree",
                ViewKind::Fields => "fields",
            },
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
            (Value::View(a), Value::View(b)) => a == b,
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
            Value::View(view) => {
                write!(f, "({}", view.kind.name())?;
                for part in view.parts.iter() {
                    write!(f, " {part}")?;
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
