//! Functions every script can call.
//!
//! A built-in that looks at anything outside its arguments does so through
//! `crate::input` and records what it saw with `Interp::record`. That record
//! is what lets a script's value be reused later without running it again,
//! so a built-in must never read the outside world any other way. There is
//! deliberately no clock, randomness, network access, or way to run another
//! program here: a value that depended on one could not be replayed.

use std::cmp::Ordering;
use std::rc::Rc;

use super::eval::{Interp, count};
use super::reader::{Error, Pos};
use super::value::{Builtin, Column, Value};
use crate::input::{self, PathKind};

/// Finds a built-in by name.
pub fn lookup(name: &str) -> Option<&'static Builtin> {
    BUILTINS.iter().find(|builtin| builtin.name == name)
}

static BUILTINS: &[Builtin] = &[
    // Arithmetic and comparison.
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
    // The outside world. Each of these records an input.
    Builtin { name: "list-dir", call: list_dir },
    Builtin { name: "read-file", call: read_file },
    Builtin { name: "file-size", call: file_size },
    Builtin { name: "is-file", call: |interp, args, pos| is_kind("is-file", PathKind::File, interp, args, pos) },
    Builtin { name: "is-dir", call: |interp, args, pos| is_kind("is-dir", PathKind::Dir, interp, args, pos) },
    Builtin { name: "env", call: env },
    // Paths, as text.
    Builtin { name: "path-join", call: path_join },
    Builtin { name: "base-name", call: base_name },
    Builtin { name: "extension", call: extension },
    // Text.
    Builtin { name: "split", call: split },
    Builtin { name: "join", call: join },
    Builtin { name: "lines", call: lines },
    Builtin { name: "trim", call: trim },
    Builtin { name: "starts-with", call: |_, args, pos| affix("starts-with", args, pos, |text, part| text.starts_with(part)) },
    Builtin { name: "ends-with", call: |_, args, pos| affix("ends-with", args, pos, |text, part| text.ends_with(part)) },
    Builtin { name: "contains", call: contains },
    Builtin { name: "replace", call: replace },
    Builtin { name: "str", call: concatenate },
    // Lists.
    Builtin { name: "list", call: |_, args, _| Ok(Value::list(args.to_vec())) },
    Builtin { name: "is-empty", call: is_empty },
    Builtin { name: "len", call: len },
    Builtin { name: "first", call: first },
    Builtin { name: "rest", call: rest },
    Builtin { name: "nth", call: nth },
    Builtin { name: "append", call: append },
    Builtin { name: "map", call: map },
    Builtin { name: "filter", call: filter },
    // Tables.
    Builtin { name: "col-def", call: col_def },
    Builtin { name: "col", call: col },
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

/// The error for an argument of the wrong kind. `wanted` reads like "a list".
fn wrong(name: &str, wanted: &str, found: &Value, pos: Pos) -> Error {
    error(pos, format!("`{name}` needs {wanted}, but was given {}", found.type_name()))
}

fn integer(name: &str, value: &Value, pos: Pos) -> Result<i64, Error> {
    match value {
        Value::Int(n) => Ok(*n),
        other => Err(wrong(name, "an integer", other, pos)),
    }
}

fn string<'a>(name: &str, value: &'a Value, pos: Pos) -> Result<&'a str, Error> {
    match value {
        Value::Str(text) => Ok(text),
        other => Err(wrong(name, "a string", other, pos)),
    }
}

fn items<'a>(name: &str, value: &'a Value, pos: Pos) -> Result<&'a Rc<[Value]>, Error> {
    match value {
        Value::List(items) => Ok(items),
        other => Err(wrong(name, "a list", other, pos)),
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

/// `(list-dir)` or `(list-dir path)`: what is in a directory, sorted by
/// name. Each entry is spelled so that it can be passed straight to another
/// built-in: `(list-dir "lore")` gives `"lore/a.md"`, not `"a.md"`.
fn list_dir(interp: &mut Interp, args: &[Value], pos: Pos) -> Result<Value, Error> {
    let shown = match args {
        [] => "",
        [path] => string("list-dir", path, pos)?,
        _ => {
            let given = args.len();
            return Err(error(pos, format!("`list-dir` takes at most 1 argument but was given {given}")));
        }
    };
    let (names, input) = input::list_dir(&interp.resolve(shown)).map_err(|cause| {
        let shown = if shown.is_empty() { "." } else { shown };
        error(pos, format!("could not list `{shown}`: {cause}"))
    })?;
    interp.record(input);

    let prefix = shown.trim_end_matches('/');
    let entries = names.iter().map(|name| match shown {
        "" => Value::str(name),
        _ => Value::str(&format!("{prefix}/{name}")),
    });
    Ok(Value::list(entries.collect()))
}

/// `(read-file path)`: the contents of a file, which must be UTF-8.
fn read_file(interp: &mut Interp, args: &[Value], pos: Pos) -> Result<Value, Error> {
    arity("read-file", args, 1, pos)?;
    let shown = string("read-file", &args[0], pos)?;
    let (bytes, input) = input::read_file(&interp.resolve(shown))
        .map_err(|cause| error(pos, format!("could not read `{shown}`: {cause}")))?;
    interp.record(input);
    match String::from_utf8(bytes) {
        Ok(text) => Ok(Value::str(&text)),
        Err(_) => Err(error(pos, format!("`{shown}` is not valid UTF-8"))),
    }
}

/// `(file-size path)`: the size of a file in bytes.
fn file_size(interp: &mut Interp, args: &[Value], pos: Pos) -> Result<Value, Error> {
    arity("file-size", args, 1, pos)?;
    let shown = string("file-size", &args[0], pos)?;
    let (size, input) = input::file_size(&interp.resolve(shown))
        .map_err(|cause| error(pos, format!("could not get the size of `{shown}`: {cause}")))?;
    interp.record(input);
    i64::try_from(size)
        .map(Value::Int)
        .map_err(|_| error(pos, format!("`{shown}` is too large to measure")))
}

/// `(is-file path)` and `(is-dir path)`. A path that does not exist is
/// neither.
fn is_kind(
    name: &str,
    wanted: PathKind,
    interp: &mut Interp,
    args: &[Value],
    pos: Pos,
) -> Result<Value, Error> {
    arity(name, args, 1, pos)?;
    let shown = string(name, &args[0], pos)?;
    let (kind, input) = input::path_kind(&interp.resolve(shown));
    interp.record(input);
    Ok(Value::Bool(kind == wanted))
}

/// `(env name)`: an environment variable, or `()` if it is not set.
fn env(interp: &mut Interp, args: &[Value], pos: Pos) -> Result<Value, Error> {
    arity("env", args, 1, pos)?;
    let (value, input) = input::env_var(string("env", &args[0], pos)?, interp.vars());
    interp.record(input);
    Ok(value.map_or_else(Value::nil, |value| Value::str(&value)))
}

/// `(path-join part...)`: joins with `/`. A part that starts with `/`
/// replaces what came before it, and empty parts are skipped.
fn path_join(_: &mut Interp, args: &[Value], pos: Pos) -> Result<Value, Error> {
    let mut path = String::new();
    for part in args {
        let part = string("path-join", part, pos)?;
        if part.is_empty() {
            continue;
        }
        if part.starts_with('/') {
            path.clear();
        } else if !path.is_empty() && !path.ends_with('/') {
            path.push('/');
        }
        path.push_str(part);
    }
    Ok(Value::str(&path))
}

/// The last component of a path.
fn last_component(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['/', '\\']);
    trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed)
}

/// `(base-name path)`: the last component, such as `a.md` for `lore/a.md`.
fn base_name(_: &mut Interp, args: &[Value], pos: Pos) -> Result<Value, Error> {
    arity("base-name", args, 1, pos)?;
    Ok(Value::str(last_component(string("base-name", &args[0], pos)?)))
}

/// `(extension path)`: what follows the last dot of the base name, without
/// the dot. Empty when there is none, and for names like `.gitignore`.
fn extension(_: &mut Interp, args: &[Value], pos: Pos) -> Result<Value, Error> {
    arity("extension", args, 1, pos)?;
    let name = last_component(string("extension", &args[0], pos)?);
    let extension = match name.rfind('.') {
        Some(dot) if dot > 0 => &name[dot + 1..],
        _ => "",
    };
    Ok(Value::str(extension))
}

/// `(split text separator)`
fn split(_: &mut Interp, args: &[Value], pos: Pos) -> Result<Value, Error> {
    arity("split", args, 2, pos)?;
    let text = string("split", &args[0], pos)?;
    let separator = string("split", &args[1], pos)?;
    if separator.is_empty() {
        return Err(error(pos, "`split` needs a separator that is not empty".to_string()));
    }
    Ok(Value::list(text.split(separator).map(Value::str).collect()))
}

/// `(join list separator)`
fn join(_: &mut Interp, args: &[Value], pos: Pos) -> Result<Value, Error> {
    arity("join", args, 2, pos)?;
    let separator = string("join", &args[1], pos)?;
    let mut joined = String::new();
    for (i, item) in items("join", &args[0], pos)?.iter().enumerate() {
        if i > 0 {
            joined.push_str(separator);
        }
        joined.push_str(string("join", item, pos)?);
    }
    Ok(Value::str(&joined))
}

/// `(lines text)`: the lines of a string, without their line endings.
fn lines(_: &mut Interp, args: &[Value], pos: Pos) -> Result<Value, Error> {
    arity("lines", args, 1, pos)?;
    let text = string("lines", &args[0], pos)?;
    Ok(Value::list(text.lines().map(Value::str).collect()))
}

/// `(trim text)`: without the whitespace at either end.
fn trim(_: &mut Interp, args: &[Value], pos: Pos) -> Result<Value, Error> {
    arity("trim", args, 1, pos)?;
    Ok(Value::str(string("trim", &args[0], pos)?.trim()))
}

/// `(starts-with text prefix)` and `(ends-with text suffix)`.
fn affix(
    name: &str,
    args: &[Value],
    pos: Pos,
    test: fn(&str, &str) -> bool,
) -> Result<Value, Error> {
    arity(name, args, 2, pos)?;
    let text = string(name, &args[0], pos)?;
    let part = string(name, &args[1], pos)?;
    Ok(Value::Bool(test(text, part)))
}

/// `(contains text part)` or `(contains list item)`.
fn contains(_: &mut Interp, args: &[Value], pos: Pos) -> Result<Value, Error> {
    arity("contains", args, 2, pos)?;
    match &args[0] {
        Value::Str(text) => {
            Ok(Value::Bool(text.contains(string("contains", &args[1], pos)?)))
        }
        Value::List(items) => Ok(Value::Bool(items.contains(&args[1]))),
        other => Err(wrong("contains", "a string or a list", other, pos)),
    }
}

/// `(replace text from to)`: every occurrence.
fn replace(_: &mut Interp, args: &[Value], pos: Pos) -> Result<Value, Error> {
    arity("replace", args, 3, pos)?;
    let text = string("replace", &args[0], pos)?;
    let from = string("replace", &args[1], pos)?;
    let to = string("replace", &args[2], pos)?;
    if from.is_empty() {
        return Err(error(pos, "`replace` needs text to look for that is not empty".to_string()));
    }
    Ok(Value::str(&text.replace(from, to)))
}

/// `(str value...)`: the values as text, run together. Strings are used as
/// they are, `()` adds nothing, and anything else is written the way a
/// script would write it.
fn concatenate(_: &mut Interp, args: &[Value], _: Pos) -> Result<Value, Error> {
    let mut text = String::new();
    for value in args {
        match value {
            Value::Str(part) => text.push_str(part),
            Value::List(items) if items.is_empty() => {}
            other => text.push_str(&other.to_string()),
        }
    }
    Ok(Value::str(&text))
}

/// Whether a list or a string has nothing in it.
fn is_empty(_: &mut Interp, args: &[Value], pos: Pos) -> Result<Value, Error> {
    arity("is-empty", args, 1, pos)?;
    match &args[0] {
        Value::List(items) => Ok(Value::Bool(items.is_empty())),
        Value::Str(text) => Ok(Value::Bool(text.is_empty())),
        other => Err(wrong("is-empty", "a list or a string", other, pos)),
    }
}

/// `(len value)`: items in a list, characters in a string, or bytes.
fn len(_: &mut Interp, args: &[Value], pos: Pos) -> Result<Value, Error> {
    arity("len", args, 1, pos)?;
    let length = match &args[0] {
        Value::List(items) => items.len(),
        Value::Str(text) => text.chars().count(),
        Value::Bytes(bytes) => bytes.len(),
        other => return Err(wrong("len", "a list, a string, or bytes", other, pos)),
    };
    Ok(Value::Int(length as i64))
}

/// `(first list)`: the first item, or `()` for an empty list.
fn first(_: &mut Interp, args: &[Value], pos: Pos) -> Result<Value, Error> {
    arity("first", args, 1, pos)?;
    let items = items("first", &args[0], pos)?;
    Ok(items.first().cloned().unwrap_or_else(Value::nil))
}

/// `(rest list)`: everything after the first item.
fn rest(_: &mut Interp, args: &[Value], pos: Pos) -> Result<Value, Error> {
    arity("rest", args, 1, pos)?;
    let items = items("rest", &args[0], pos)?;
    Ok(Value::list(items.iter().skip(1).cloned().collect()))
}

/// `(nth list index)`: the item at an index, counting from zero. Unlike
/// `first`, an index outside the list is an error.
fn nth(_: &mut Interp, args: &[Value], pos: Pos) -> Result<Value, Error> {
    arity("nth", args, 2, pos)?;
    let items = items("nth", &args[0], pos)?;
    let index = integer("nth", &args[1], pos)?;
    usize::try_from(index).ok().and_then(|index| items.get(index)).cloned().ok_or_else(|| {
        let length = count(items.len(), "item");
        error(pos, format!("`nth` index {index} is outside a list of {length}"))
    })
}

/// `(append list item...)`: a new list with the items added at the end.
fn append(_: &mut Interp, args: &[Value], pos: Pos) -> Result<Value, Error> {
    let Some((list, added)) = args.split_first() else {
        return Err(error(pos, "`append` takes a list and the items to add".to_string()));
    };
    let mut all = items("append", list, pos)?.to_vec();
    all.extend_from_slice(added);
    Ok(Value::list(all))
}

/// `(map list function)`: the results of calling the function on each item.
fn map(interp: &mut Interp, args: &[Value], pos: Pos) -> Result<Value, Error> {
    arity("map", args, 2, pos)?;
    let items = Rc::clone(items("map", &args[0], pos)?);
    let mut results = Vec::with_capacity(items.len());
    for item in items.iter() {
        results.push(interp.call(&args[1], std::slice::from_ref(item), pos)?);
    }
    Ok(Value::list(results))
}

/// `(filter list function)`: the items for which the function gives a true
/// value.
fn filter(interp: &mut Interp, args: &[Value], pos: Pos) -> Result<Value, Error> {
    arity("filter", args, 2, pos)?;
    let items = Rc::clone(items("filter", &args[0], pos)?);
    let mut kept = Vec::new();
    for item in items.iter() {
        if interp.call(&args[1], std::slice::from_ref(item), pos)?.is_truthy() {
            kept.push(item.clone());
        }
    }
    Ok(Value::list(kept))
}

/// `(col-def header cells)`: one column of a table.
fn col_def(_: &mut Interp, args: &[Value], pos: Pos) -> Result<Value, Error> {
    arity("col-def", args, 2, pos)?;
    let header = string("col-def", &args[0], pos)?;
    let cells = items("col-def", &args[1], pos)?;
    Ok(Value::Column(Rc::new(Column { header: Rc::from(header), cells: Rc::clone(cells) })))
}

/// `(col column...)`: a table made of columns from `col-def`.
fn col(_: &mut Interp, args: &[Value], pos: Pos) -> Result<Value, Error> {
    if args.is_empty() {
        return Err(error(pos, "`col` takes at least 1 column".to_string()));
    }
    let mut columns = Vec::with_capacity(args.len());
    for value in args {
        match value {
            Value::Column(column) => columns.push(Rc::clone(column)),
            other => return Err(wrong("col", "columns made with `col-def`", other, pos)),
        }
    }
    Ok(Value::Table(Rc::from(columns)))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::input::{self, Freshness, Input};
    use crate::script;
    use crate::testing::{SECOND, Scratch};

    /// What a script produced, with its value printed.
    struct Ran {
        value: String,
        inputs: Vec<Input>,
    }

    fn run(source: &str, base: &Path) -> Result<Ran, script::Error> {
        script::run(source.as_bytes(), base, &[], |outcome| Ran {
            value: outcome.value.to_string(),
            inputs: outcome.inputs,
        })
    }

    /// The value of a script that touches nothing outside itself.
    fn value(source: &str) -> String {
        let ran = run(source, Path::new("")).unwrap_or_else(|error| panic!("{source}\nfailed: {error}"));
        assert_eq!(ran.inputs, [], "{source}");
        ran.value
    }

    fn failure(source: &str) -> String {
        match run(source, Path::new("")) {
            Ok(ran) => panic!("{source}\nshould fail but gave {}", ran.value),
            Err(error) => error.to_string(),
        }
    }

    fn run_in(scratch: &Scratch, source: &str) -> Ran {
        run(source, scratch.root()).unwrap_or_else(|error| panic!("{source}\nfailed: {error}"))
    }

    /// A short description of an input, with the scratch directory removed
    /// from its path.
    fn describe(scratch: &Scratch, input: &Input) -> String {
        let root = format!("{}/", scratch.root().to_string_lossy().replace('\\', "/"));
        let relative = |path: &str| path.strip_prefix(&root).unwrap_or(path).to_string();
        match input {
            Input::File { path, .. } => format!("file {}", relative(path)),
            Input::Dir { path, .. } => format!("dir {}", relative(path)),
            Input::Kind { path, kind } => format!("kind {} {kind:?}", relative(path)),
            Input::Size { path, size } => format!("size {} {size}", relative(path)),
            Input::Env { name, hash } => format!("env {name} set={}", hash.is_some()),
        }
    }

    fn inputs(scratch: &Scratch, outcome: &Ran) -> Vec<String> {
        outcome.inputs.iter().map(|input| describe(scratch, input)).collect()
    }

    #[test]
    fn text() {
        assert_eq!(value(r#"(split "a,b,,c" ",")"#), r#"("a" "b" "" "c")"#);
        assert_eq!(value(r#"(join (list "a" "b" "c") ", ")"#), r#""a, b, c""#);
        assert_eq!(value(r#"(join () ", ")"#), r#""""#);
        assert_eq!(value(r#"(lines "one\r\ntwo\n\nfour\n")"#), r#"("one" "two" "" "four")"#);
        assert_eq!(value(r#"(trim "  padded\t\n")"#), r#""padded""#);
        assert_eq!(value(r#"(list (starts-with "status: ok" "status:") (starts-with "x" "status:"))"#), "(true false)");
        assert_eq!(value(r#"(list (ends-with "a.md" ".md") (ends-with "a.md" ".txt"))"#), "(true false)");
        assert_eq!(value(r#"(list (contains "haystack" "st") (contains "haystack" "x"))"#), "(true false)");
        assert_eq!(value(r#"(replace "a-b-c" "-" "+")"#), r#""a+b+c""#);
        assert_eq!(value(r#"(str "n=" 3 " ok=" true " " () (list 1 "x"))"#), r#""n=3 ok=true (1 \"x\")""#);
        assert_eq!(value("(str)"), r#""""#);
        assert_eq!(value(r#"(list (len "caf\u00e9") (len "") (len (list 1 2 3)) (len ()) (len #4:abcd))"#.replace("\\u00e9", "\u{e9}").as_str()), "(4 0 3 0 4)");
        assert_eq!(value(r#"(list (is-empty "") (is-empty "a") (is-empty ()) (is-empty (list 1)))"#), "(true false true false)");
    }

    #[test]
    fn paths() {
        assert_eq!(value(r#"(path-join "lore" "a.md")"#), r#""lore/a.md""#);
        assert_eq!(value(r#"(path-join "lore/" "sub" "" "a.md")"#), r#""lore/sub/a.md""#);
        assert_eq!(value(r#"(path-join "lore" "/etc" "hosts")"#), r#""/etc/hosts""#);
        assert_eq!(value(r#"(path-join)"#), r#""""#);
        assert_eq!(value(r#"(list (base-name "lore/sub/a.md") (base-name "a.md") (base-name "lore/sub/") (base-name "c:\\x\\y.txt"))"#), r#"("a.md" "a.md" "sub" "y.txt")"#);
        assert_eq!(value(r#"(list (extension "lore/a.md") (extension "a.tar.gz") (extension "Makefile") (extension ".gitignore") (extension "dir.d/file"))"#), r#"("md" "gz" "" "" "")"#);
    }

    #[test]
    fn lists() {
        assert_eq!(value("(list)"), "()");
        assert_eq!(value("(list (first (list 1 2 3)) (first ()))"), "(1 ())");
        assert_eq!(value("(list (rest (list 1 2 3)) (rest (list 1)) (rest ()))"), "((2 3) () ())");
        assert_eq!(value("(nth (list \"a\" \"b\") 1)"), "\"b\"");
        assert_eq!(value("(append (list 1) 2 3)"), "(1 2 3)");
        assert_eq!(value("(append () 1)"), "(1)");
        assert_eq!(value("(list (contains (list 1 2) 2) (contains (list 1 2) 3) (contains (list (list 1)) (list 1)))"), "(true false true)");
        // Lists are values: adding to one leaves the original alone.
        assert_eq!(value("(var a (list 1)) (var b (append a 2)) (list a b)"), "((1) (1 2))");
    }

    #[test]
    fn map_and_filter() {
        assert_eq!(value("(map (list 1 2 3) (fn (n) (* n n)))"), "(1 4 9)");
        assert_eq!(value("(map () (fn (n) n))"), "()");
        assert_eq!(value("(fn double (n) (* n 2)) (map (list 1 2) double)"), "(2 4)");
        // A built-in can be passed just like a script's own function.
        assert_eq!(value(r#"(map (list " a " "b ") trim)"#), r#"("a" "b")"#);
        assert_eq!(value("(filter (list 1 2 3 4) (fn (n) (= (% n 2) 0)))"), "(2 4)");
        assert_eq!(value(r#"(filter (list "a.md" "b.txt") (fn (f) (ends-with f ".md")))"#), r#"("a.md")"#);
        // The function can change variables around it.
        assert_eq!(value("(var total 0) (map (list 1 2 3) (fn (n) (set total (+ total n)))) total"), "6");
        // `return` inside the function only leaves that function.
        assert_eq!(value("(map (list 1 2) (fn (n) (return (* n 10)) 0))"), "(10 20)");
    }

    #[test]
    fn tables() {
        assert_eq!(
            value(r#"(col (col-def "Name" (list "a" "b")) (col-def "Size" (list 1 2)))"#),
            r#"(col (col-def "Name" ("a" "b")) (col-def "Size" (1 2)))"#
        );
        assert_eq!(value(r#"(col-def "Empty" ())"#), r#"(col-def "Empty" ())"#);
    }

    #[test]
    fn rejects_bad_calls() {
        let cases = [
            (r#"(split "a" "")"#, "`split` needs a separator that is not empty"),
            (r#"(split 1 ",")"#, "`split` needs a string, but was given an integer"),
            (r#"(join (list "a" 1) ",")"#, "`join` needs a string, but was given an integer"),
            (r#"(join "a" ",")"#, "`join` needs a list, but was given a string"),
            (r#"(replace "a" "" "b")"#, "`replace` needs text to look for that is not empty"),
            (r#"(contains 1 2)"#, "`contains` needs a string or a list, but was given an integer"),
            (r#"(len 5)"#, "`len` needs a list, a string, or bytes, but was given an integer"),
            (r#"(is-empty 5)"#, "`is-empty` needs a list or a string, but was given an integer"),
            (r#"(nth (list 1 2) 2)"#, "`nth` index 2 is outside a list of 2 items"),
            (r#"(nth (list 1) -1)"#, "`nth` index -1 is outside a list of 1 item"),
            (r#"(first "abc")"#, "`first` needs a list, but was given a string"),
            (r#"(append)"#, "`append` takes a list and the items to add"),
            (r#"(map "abc" (fn (c) c))"#, "`map` needs a list, but was given a string"),
            (r#"(map (list 1) 5)"#, "an integer is not a function"),
            (r#"(map (list 1) (fn () 1))"#, "`this function` takes 0 arguments but was given 1"),
            (r#"(map (list 1))"#, "`map` takes 2 arguments but was given 1"),
            (r#"(col)"#, "`col` takes at least 1 column"),
            (r#"(col (list 1))"#, "`col` needs columns made with `col-def`, but was given a list"),
            (r#"(col-def 1 ())"#, "`col-def` needs a string, but was given an integer"),
            (r#"(col-def "h" "cells")"#, "`col-def` needs a list, but was given a string"),
            (r#"(list-dir "a" "b")"#, "`list-dir` takes at most 1 argument but was given 2"),
            (r#"(read-file 1)"#, "`read-file` needs a string, but was given an integer"),
            (r#"(env)"#, "`env` takes 1 argument but was given 0"),
        ];
        for (source, expected) in cases {
            assert_eq!(failure(source), format!("1:1: {expected}"), "{source}");
        }
    }

    #[test]
    fn an_error_inside_a_mapped_function_points_into_it() {
        let source = "(map (list 1 2)\n  (fn (n)\n    (+ n \"x\")))";
        assert_eq!(failure(source), "3:5: `+` needs an integer, but was given a string");
    }

    #[test]
    fn reading_files_records_them() {
        let scratch = Scratch::new("builtins-read");
        scratch.write("notes/a.txt", "hello\nworld\n");

        let outcome = run_in(&scratch, r#"(lines (read-file "notes/a.txt"))"#);
        assert_eq!(outcome.value, r#"("hello" "world")"#);
        assert_eq!(inputs(&scratch, &outcome), ["file notes/a.txt"]);

        // Reading the same file twice is one input.
        let outcome = run_in(&scratch, r#"(read-file "notes/a.txt") (read-file "notes/a.txt")"#);
        assert_eq!(inputs(&scratch, &outcome), ["file notes/a.txt"]);

        let outcome = run_in(&scratch, r#"(file-size "notes/a.txt")"#);
        assert_eq!(outcome.value, "12");
        assert_eq!(inputs(&scratch, &outcome), ["size notes/a.txt 12"]);
    }

    #[test]
    fn listing_a_directory_records_it() {
        let scratch = Scratch::new("builtins-list");
        scratch.write("lore/b.md", "b");
        scratch.write("lore/a.md", "a");
        scratch.write("lore/sub/c.md", "c");

        let outcome = run_in(&scratch, r#"(list-dir "lore")"#);
        assert_eq!(outcome.value, r#"("lore/a.md" "lore/b.md" "lore/sub")"#);
        assert_eq!(inputs(&scratch, &outcome), ["dir lore"]);

        // A trailing slash does not double up.
        let outcome = run_in(&scratch, r#"(list-dir "lore/")"#);
        assert_eq!(outcome.value, r#"("lore/a.md" "lore/b.md" "lore/sub")"#);

        // With no argument, the script's own directory, as bare names.
        let outcome = run_in(&scratch, "(list-dir)");
        assert_eq!(outcome.value, r#"("lore")"#);
        assert_eq!(inputs(&scratch, &outcome), ["dir ."]);
    }

    #[test]
    fn asking_what_a_path_is_records_the_answer() {
        let scratch = Scratch::new("builtins-kind");
        scratch.write("lore/a.md", "a");
        let source = r#"(list (is-file "lore/a.md") (is-dir "lore/a.md") (is-dir "lore") (is-file "nope") (is-dir "nope"))"#;
        let outcome = run_in(&scratch, source);
        assert_eq!(outcome.value, "(true false true false false)");
        assert_eq!(
            inputs(&scratch, &outcome),
            ["kind lore/a.md File", "kind lore Dir", "kind nope Missing"]
        );
    }

    #[test]
    fn reading_the_environment_records_it() {
        let scratch = Scratch::new("builtins-env");
        let outcome = run_in(&scratch, r#"(list (is-empty (env "PATH")) (env "YIO_TEST_SURELY_UNSET_VARIABLE"))"#);
        assert_eq!(outcome.value, "(false ())");
        assert_eq!(
            inputs(&scratch, &outcome),
            ["env PATH set=true", "env YIO_TEST_SURELY_UNSET_VARIABLE set=false"]
        );
    }

    #[test]
    fn missing_files_are_errors_that_name_the_path() {
        let scratch = Scratch::new("builtins-missing");
        for (source, expected) in [
            (r#"(read-file "nope.txt")"#, "1:1: could not read `nope.txt`: "),
            (r#"(list-dir "nope")"#, "1:1: could not list `nope`: "),
            (r#"(file-size "nope.txt")"#, "1:1: could not get the size of `nope.txt`: "),
            (r#"(file-size ".")"#, "1:1: could not get the size of `.`: not a file"),
        ] {
            let error = run(source, scratch.root()).err().unwrap().to_string();
            assert!(error.starts_with(expected), "{source} gave {error}");
        }
        scratch.write("binary", "");
        std::fs::write(scratch.path("binary"), [0xff, 0xfe]).unwrap();
        let error = run(r#"(read-file "binary")"#, scratch.root()).err().unwrap();
        assert_eq!(error.to_string(), "1:1: `binary` is not valid UTF-8");
    }

    /// The script from the project's design sketch, with its elided parts
    /// filled in.
    const SKETCH: &str = r#"
-- list-dir lists the files/subdirectories in a given directory
(var lore-files (list-dir "lore"))

-- is-empty is a built-in function
(if (is-empty lore-files)
  return)

(fn read-status (file)
  -- () is an empty value
  (var output ())
  (map (lines (read-file file))
    (fn (line)
      (if (starts-with line "status:")
        (set output (trim (replace line "status:" ""))))))
  output)

(col
  (col-def
    "File path"
    -- Immediately execute an anonymous function
    ((fn (files) files) lore-files))
  (col-def
    "File size"
    (map lore-files file-size))
  (col-def
    "Status"
    (map lore-files read-status)))
"#;

    #[test]
    fn the_design_sketch_runs() {
        let scratch = Scratch::new("builtins-sketch");
        scratch.write("lore/a.md", "status: draft\nbody\n");
        scratch.write("lore/b.md", "no status here\n");

        let outcome = run_in(&scratch, SKETCH);
        assert_eq!(
            outcome.value,
            r#"(col (col-def "File path" ("lore/a.md" "lore/b.md")) (col-def "File size" (19 15)) (col-def "Status" ("draft" ())))"#
        );
        assert_eq!(
            inputs(&scratch, &outcome),
            [
                "dir lore",
                "size lore/a.md 19",
                "size lore/b.md 15",
                "file lore/a.md",
                "file lore/b.md",
            ]
        );
    }

    #[test]
    fn the_design_sketch_stops_early_when_there_is_nothing_to_show() {
        let scratch = Scratch::new("builtins-sketch-empty");
        std::fs::create_dir(scratch.path("lore")).unwrap();
        let outcome = run_in(&scratch, SKETCH);
        assert_eq!(outcome.value, "()");
        assert_eq!(inputs(&scratch, &outcome), ["dir lore"]);
    }

    #[test]
    fn recorded_inputs_say_when_a_script_must_run_again() {
        let scratch = Scratch::new("builtins-fresh");
        scratch.write("lore/a.md", "status: draft\n");
        let as_of = input::now();
        let outcome = run_in(&scratch, SKETCH);
        let later = as_of + 100 * SECOND;
        let fresh = || input::check(&outcome.inputs, as_of, later, &[]);
        assert_ne!(fresh(), Freshness::Dirty);

        // A file the script read changes.
        scratch.write("lore/a.md", "status: final\n");
        assert_eq!(fresh(), Freshness::Dirty);
        scratch.write("lore/a.md", "status: draft\n");
        assert_ne!(fresh(), Freshness::Dirty);

        // A file appears in a directory the script listed.
        scratch.write("lore/b.md", "status: new\n");
        assert_eq!(fresh(), Freshness::Dirty);
        std::fs::remove_file(scratch.path("lore/b.md")).unwrap();
        assert_ne!(fresh(), Freshness::Dirty);

        // A file the script never looked at does not matter.
        scratch.write("other/c.md", "irrelevant");
        assert_ne!(fresh(), Freshness::Dirty);
    }
}
