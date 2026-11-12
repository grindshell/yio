//! Functions every script can call.
//!
//! Only the pure core is here so far: arithmetic, comparison, `not`, and
//! `list`. None of these looks at anything outside its arguments.

use std::cmp::Ordering;

use super::eval::{Interp, count};
use super::reader::{Error, Pos};
use super::value::{Builtin, Value};

/// Finds a built-in by name.
pub fn lookup(name: &str) -> Option<&'static Builtin> {
    BUILTINS.iter().find(|builtin| builtin.name == name)
}

static BUILTINS: [Builtin; 13] = [
    Builtin { name: "+", call: |_, args, pos| fold("+", args, pos, i64::checked_add) },
    Builtin { name: "*", call: |_, args, pos| fold("*", args, pos, i64::checked_mul) },
    Builtin { name: "-", call: subtract },
    Builtin { name: "/", call: |_, args, pos| divide("/", args, pos, i64::checked_div) },
    Builtin { name: "%", call: |_, args, pos| divide("%", args, pos, i64::checked_rem) },
    Builtin { name: "=", call: equal },
    Builtin { name: "<", call: |_, args, pos| compare("<", args, pos, Ordering::is_lt) },
    Builtin { name: ">", call: |_, args, pos| compare(">", args, pos, Ordering::is_gt) },
    Builtin { name: "<=", call: |_, args, pos| compare("<=", args, pos, Ordering::is_le) },
    Builtin { name: ">=", call: |_, args, pos| compare(">=", args, pos, Ordering::is_ge) },
    Builtin { name: "not", call: not },
    Builtin { name: "list", call: |_, args, _| Ok(Value::list(args.to_vec())) },
    Builtin { name: "is-empty", call: is_empty },
];

fn error(pos: Pos, message: String) -> Error {
    Error { pos, message }
}

/// Checks that a call has exactly `expected` arguments.
fn arity(name: &str, args: &[Value], expected: usize, pos: Pos) -> Result<(), Error> {
    if args.len() == expected {
        return Ok(());
    }
    let expected = count(expected, "argument");
    Err(error(pos, format!("`{name}` takes {expected} but was given {}", args.len())))
}

fn integer(name: &str, value: &Value, pos: Pos) -> Result<i64, Error> {
    match value {
        Value::Int(n) => Ok(*n),
        other => Err(error(
            pos,
            format!("`{name}` works on integers, but was given {}", other.type_name()),
        )),
    }
}

/// Combines one or more integers left to right.
fn fold(
    name: &str,
    args: &[Value],
    pos: Pos,
    combine: fn(i64, i64) -> Option<i64>,
) -> Result<Value, Error> {
    let Some((first, rest)) = args.split_first() else {
        return Err(error(pos, format!("`{name}` takes at least 1 argument but was given 0")));
    };
    let mut total = integer(name, first, pos)?;
    for value in rest {
        total = combine(total, integer(name, value, pos)?)
            .ok_or_else(|| error(pos, format!("`{name}` overflowed")))?;
    }
    Ok(Value::Int(total))
}

/// `(- n)` negates; `(- a b ...)` subtracts left to right.
fn subtract(_: &mut Interp, args: &[Value], pos: Pos) -> Result<Value, Error> {
    match args {
        [only] => integer("-", only, pos)?
            .checked_neg()
            .map(Value::Int)
            .ok_or_else(|| error(pos, "`-` overflowed".to_string())),
        _ => fold("-", args, pos, i64::checked_sub),
    }
}

/// `/` and `%`. Division truncates toward zero.
fn divide(
    name: &str,
    args: &[Value],
    pos: Pos,
    apply: fn(i64, i64) -> Option<i64>,
) -> Result<Value, Error> {
    arity(name, args, 2, pos)?;
    let left = integer(name, &args[0], pos)?;
    let right = integer(name, &args[1], pos)?;
    if right == 0 {
        return Err(error(pos, "division by zero".to_string()));
    }
    apply(left, right)
        .map(Value::Int)
        .ok_or_else(|| error(pos, format!("`{name}` overflowed")))
}

/// Any two values can be compared for equality. Values of different kinds
/// are simply not equal.
fn equal(_: &mut Interp, args: &[Value], pos: Pos) -> Result<Value, Error> {
    arity("=", args, 2, pos)?;
    Ok(Value::Bool(args[0] == args[1]))
}

/// Orders two integers, or two strings.
fn compare(
    name: &str,
    args: &[Value],
    pos: Pos,
    accept: fn(Ordering) -> bool,
) -> Result<Value, Error> {
    arity(name, args, 2, pos)?;
    let ordering = match (&args[0], &args[1]) {
        (Value::Int(a), Value::Int(b)) => a.cmp(b),
        (Value::Str(a), Value::Str(b)) => a.cmp(b),
        (a, b) => {
            return Err(error(
                pos,
                format!("`{name}` cannot compare {} with {}", a.type_name(), b.type_name()),
            ));
        }
    };
    Ok(Value::Bool(accept(ordering)))
}

fn not(_: &mut Interp, args: &[Value], pos: Pos) -> Result<Value, Error> {
    arity("not", args, 1, pos)?;
    Ok(Value::Bool(!args[0].is_truthy()))
}

/// Whether a list or a string has nothing in it.
fn is_empty(_: &mut Interp, args: &[Value], pos: Pos) -> Result<Value, Error> {
    arity("is-empty", args, 1, pos)?;
    match &args[0] {
        Value::List(items) => Ok(Value::Bool(items.is_empty())),
        Value::Str(text) => Ok(Value::Bool(text.is_empty())),
        other => Err(error(
            pos,
            format!("`is-empty` works on lists and strings, but was given {}", other.type_name()),
        )),
    }
}
