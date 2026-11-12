//! Turns the value a script produced into text for the terminal.

use std::rc::Rc;

use super::value::{Column, Value};

/// Renders a script's value. `()` renders as nothing, a string as a line, a
/// table as aligned columns, and a list as each of its items in turn.
///
/// Fails if the value contains a function, which has no useful text form
/// and could not be stored in the cache either.
pub fn render(value: &Value) -> Result<String, String> {
    let mut out = String::new();
    write(&mut out, value)?;
    Ok(out)
}

fn write(out: &mut String, value: &Value) -> Result<(), String> {
    match value {
        Value::List(items) => {
            for item in items.iter() {
                write(out, item)?;
            }
        }
        Value::Table(columns) => table(out, columns)?,
        Value::Column(column) => table(out, std::slice::from_ref(column))?,
        Value::Fn(_) | Value::Native(_) => {
            return Err("a script's result cannot contain a function".to_string());
        }
        Value::Str(text) => line(out, text),
        other => line(out, &other.to_string()),
    }
    Ok(())
}

fn line(out: &mut String, text: &str) {
    out.push_str(text);
    if !text.ends_with('\n') {
        out.push('\n');
    }
}

/// The text of one table cell. Line breaks would tear the row apart, so
/// they become spaces.
fn cell(value: &Value) -> Result<String, String> {
    let text = match value {
        Value::Str(text) => text.to_string(),
        Value::List(items) if items.is_empty() => String::new(),
        Value::Fn(_) | Value::Native(_) => {
            return Err("a script's result cannot contain a function".to_string());
        }
        other => other.to_string(),
    };
    Ok(text.replace(['\r', '\n'], " "))
}

/// Draws columns side by side: a row of headers, a rule, then the cells.
/// A column that runs out of cells before the others is padded with blanks.
fn table(out: &mut String, columns: &[Rc<Column>]) -> Result<(), String> {
    let mut texts: Vec<Vec<String>> = Vec::with_capacity(columns.len());
    for column in columns {
        let mut cells = vec![column.header.replace(['\r', '\n'], " ")];
        for value in column.cells.iter() {
            cells.push(cell(value)?);
        }
        texts.push(cells);
    }
    let width = |cells: &Vec<String>| cells.iter().map(|text| text.chars().count()).max();
    let widths: Vec<usize> = texts.iter().map(|cells| width(cells).unwrap_or(0)).collect();
    let rows = texts.iter().map(Vec::len).max().unwrap_or(0);

    for row in 0..rows {
        let cells = texts.iter().map(|cells| cells.get(row).map_or("", String::as_str));
        row_line(out, cells, &widths);
        if row == 0 {
            let rules: Vec<String> = widths.iter().map(|width| "-".repeat(*width)).collect();
            row_line(out, rules.iter().map(String::as_str), &widths);
        }
    }
    Ok(())
}

fn row_line<'a>(out: &mut String, cells: impl Iterator<Item = &'a str>, widths: &[usize]) {
    let mut line = String::new();
    for (text, width) in cells.zip(widths) {
        if !line.is_empty() {
            line.push_str("  ");
        }
        line.push_str(text);
        line.extend(std::iter::repeat_n(' ', width.saturating_sub(text.chars().count())));
    }
    out.push_str(line.trim_end());
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::script;

    /// Runs a script and renders its value.
    fn rendered(source: &str) -> Result<String, String> {
        script::run(source.as_bytes(), Path::new(""), crate::input::Vars::NONE, |outcome| super::render(&outcome.value))
            .unwrap_or_else(|error| panic!("{source}\nfailed: {error}"))
    }

    #[test]
    fn simple_values() {
        assert_eq!(rendered("()").unwrap(), "");
        assert_eq!(rendered("\"hello\"").unwrap(), "hello\n");
        assert_eq!(rendered("\"already ended\\n\"").unwrap(), "already ended\n");
        assert_eq!(rendered("42").unwrap(), "42\n");
        assert_eq!(rendered("true").unwrap(), "true\n");
    }

    #[test]
    fn a_list_renders_each_item() {
        assert_eq!(rendered("(list \"a\" \"b\" (list \"c\" 4) ())").unwrap(), "a\nb\nc\n4\n");
    }

    #[test]
    fn a_table_is_aligned() {
        let source = r#"
            (col
              (col-def "File path" (list "lore/a.md" "lore/long-name.md"))
              (col-def "Size" (list 19 1500))
              (col-def "Status" (list "draft" ())))"#;
        assert_eq!(
            rendered(source).unwrap(),
            "\
File path          Size  Status
-----------------  ----  ------
lore/a.md          19    draft
lore/long-name.md  1500
"
        );
    }

    #[test]
    fn uneven_columns_are_padded() {
        let source = r#"(col (col-def "A" (list 1 2 3)) (col-def "B" (list "x")) (col-def "C" ()))"#;
        assert_eq!(rendered(source).unwrap(), "A  B  C\n-  -  -\n1  x\n2\n3\n");
    }

    #[test]
    fn cells_stay_on_one_line_and_count_characters() {
        let source = "(col (col-def \"Name\" (list \"two\\nlines\" \"caf\u{e9}\")) (col-def \"N\" (list 1 2)))";
        assert_eq!(
            rendered(source).unwrap(),
            "Name       N\n---------  -\ntwo lines  1\ncaf\u{e9}       2\n"
        );
    }

    #[test]
    fn a_table_can_sit_among_other_output() {
        let source = r#"(list "Lore" (col (col-def "File" (list "a"))) "done")"#;
        assert_eq!(rendered(source).unwrap(), "Lore\nFile\n----\na\ndone\n");
        assert_eq!(rendered(r#"(col-def "Only" (list 1))"#).unwrap(), "Only\n----\n1\n");
    }

    #[test]
    fn functions_cannot_be_rendered() {
        let message = "a script's result cannot contain a function";
        assert_eq!(rendered("(fn (x) x)").unwrap_err(), message);
        assert_eq!(rendered("(list 1 +)").unwrap_err(), message);
        assert_eq!(rendered("(col (col-def \"F\" (list trim)))").unwrap_err(), message);
    }
}
