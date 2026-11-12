//! Turns the value a script produced into text for the terminal.
//!
//! Every value is first laid out as a block: a list of lines, each made of
//! spans of text in one style. Blocks nest, which is what lets a table sit
//! in a box that sits in a row. Only at the very end are the lines written
//! out, with or without the codes that make a terminal show the styles.

use std::io::IsTerminal;
use std::rc::Rc;
use std::sync::OnceLock;

use super::value::{Column, Value, View, ViewKind};

/// The names `style` takes.
pub const STYLES: [&str; 12] = [
    "bold",
    "dim",
    "italic",
    "underline",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "gray",
];

pub fn is_style(name: &str) -> bool {
    STYLES.contains(&name)
}

const FUNCTION: &str = "a script's result cannot contain a function";

/// How wide a rule is when there is nothing beside it to match.
const LONE_RULE: usize = 40;

/// How wide a bar is when the script does not say.
const BAR_WIDTH: i64 = 20;

/// Renders a script's value. `()` renders as nothing, a string as a line, a
/// table as aligned columns, and a list as each of its items in turn.
/// Styles are shown if the terminal can show them.
///
/// Fails if the value contains a function, which has no useful text form
/// and could not be stored in the cache either.
pub fn render(value: &Value) -> Result<String, String> {
    render_as(value, wants_styles())
}

/// Renders a value, with the styles in it either shown or left out.
pub fn render_as(value: &Value, styles: bool) -> Result<String, String> {
    let mut out = String::new();
    for line in fit(build(value)?) {
        for span in &line {
            let codes = span.style.codes();
            if styles && !codes.is_empty() {
                out.push_str(&format!("\x1b[{codes}m{}\x1b[0m", span.text));
            } else {
                out.push_str(&span.text);
            }
        }
        out.push('\n');
    }
    Ok(out)
}

/// How text is shown. Each part is off unless a `style` turned it on.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Style {
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    /// The terminal's code for the colour.
    color: Option<u8>,
}

impl Style {
    /// The style that the names given to `style` add up to. `View::new`
    /// has checked that each is one of `STYLES`.
    fn named(names: &[Value]) -> Style {
        let mut style = Style::default();
        for name in names {
            let Value::Str(name) = name else { continue };
            match &**name {
                "bold" => style.bold = true,
                "dim" => style.dim = true,
                "italic" => style.italic = true,
                "underline" => style.underline = true,
                "red" => style.color = Some(31),
                "green" => style.color = Some(32),
                "yellow" => style.color = Some(33),
                "blue" => style.color = Some(34),
                "magenta" => style.color = Some(35),
                "cyan" => style.color = Some(36),
                "white" => style.color = Some(37),
                "gray" => style.color = Some(90),
                _ => {}
            }
        }
        style
    }

    /// This style inside `outer`: what the outer one turns on stays on, and
    /// the inner colour wins.
    fn inside(self, outer: Style) -> Style {
        Style {
            bold: self.bold || outer.bold,
            dim: self.dim || outer.dim,
            italic: self.italic || outer.italic,
            underline: self.underline || outer.underline,
            color: self.color.or(outer.color),
        }
    }

    /// The codes that turn this style on, as they go between `ESC [` and
    /// `m`. Empty for plain text.
    fn codes(self) -> String {
        let switches = [(self.bold, 1), (self.dim, 2), (self.italic, 3), (self.underline, 4)];
        let on = switches.into_iter().filter(|(on, _)| *on).map(|(_, code)| code);
        let codes: Vec<String> = on.chain(self.color).map(|code| code.to_string()).collect();
        codes.join(";")
    }
}

/// A run of text in one style.
#[derive(Debug, Clone, PartialEq)]
struct Span {
    text: String,
    style: Style,
}

fn plain(text: impl Into<String>) -> Span {
    Span { text: text.into(), style: Style::default() }
}

/// One line of a block that is being laid out.
enum Line {
    Text(Vec<Span>),
    /// A rule, which is as wide as the block turns out to be.
    Rule(Style),
}

type Block = Vec<Line>;

fn width(spans: &[Span]) -> usize {
    spans.iter().map(|span| span.text.chars().count()).sum()
}

fn widest(lines: &[Vec<Span>]) -> usize {
    lines.iter().map(|line| width(line)).max().unwrap_or(0)
}

fn spaces(count: usize) -> Span {
    plain(" ".repeat(count))
}

/// Settles how wide a block is, and draws its rules to that width.
fn fit(block: Block) -> Vec<Vec<Span>> {
    let text = block.iter().filter_map(|line| match line {
        Line::Text(spans) => Some(width(spans)),
        Line::Rule(_) => None,
    });
    let across = text.max().unwrap_or(LONE_RULE);
    let draw = |line| match line {
        Line::Text(spans) => spans,
        Line::Rule(style) => vec![Span { text: "\u{2500}".repeat(across), style }],
    };
    block.into_iter().map(draw).collect()
}

/// The lines of a value that has to stay on one line, such as a table
/// cell, run together with spaces between.
fn inline(value: &Value) -> Result<Vec<Span>, String> {
    let mut spans = Vec::new();
    for (i, line) in fit(build(value)?).into_iter().enumerate() {
        if i > 0 {
            spans.push(plain(" "));
        }
        spans.extend(line);
    }
    Ok(spans)
}

/// Lays a value out.
fn build(value: &Value) -> Result<Block, String> {
    Ok(match value {
        Value::List(items) => {
            let mut block = Vec::new();
            for item in items.iter() {
                block.extend(build(item)?);
            }
            block
        }
        Value::Str(text) => {
            // A line break at the very end does not start another line.
            let text = text.strip_suffix('\n').unwrap_or(text);
            let lines = text.split('\n').map(|line| line.strip_suffix('\r').unwrap_or(line));
            lines.map(|line| Line::Text(vec![plain(line)])).collect()
        }
        Value::Table(columns) => table(columns)?,
        Value::Column(column) => table(std::slice::from_ref(column))?,
        Value::View(view) => draw(view)?,
        Value::Fn(_) | Value::Native(_) => return Err(FUNCTION.to_string()),
        other => vec![Line::Text(vec![plain(other.to_string())])],
    })
}

/// Draws columns side by side: a row of headers, a rule, then the cells.
/// A column that runs out of cells before the others is padded with blanks.
fn table(columns: &[Rc<Column>]) -> Result<Block, String> {
    let mut cells: Vec<Vec<Vec<Span>>> = Vec::with_capacity(columns.len());
    for column in columns {
        let mut column_cells = vec![vec![plain(column.header.replace(['\r', '\n'], " "))]];
        for value in column.cells.iter() {
            column_cells.push(inline(value)?);
        }
        cells.push(column_cells);
    }
    let widths: Vec<usize> = cells.iter().map(|column| widest(column)).collect();
    let rows = cells.iter().map(Vec::len).max().unwrap_or(0);

    let mut block = Vec::new();
    for row in 0..rows {
        let across = cells.iter().map(|column| column.get(row).cloned().unwrap_or_default());
        block.push(Line::Text(beside(across, &widths)));
        if row == 0 {
            let rules = widths.iter().map(|width| vec![plain("-".repeat(*width))]);
            block.push(Line::Text(beside(rules, &widths)));
        }
    }
    Ok(block)
}

/// Puts the pieces of one line side by side, each padded to its width, with
/// two spaces between. Nothing is added after the last piece that has
/// anything in it, so the line does not end in padding.
fn beside(pieces: impl Iterator<Item = Vec<Span>>, widths: &[usize]) -> Vec<Span> {
    let pieces: Vec<Vec<Span>> = pieces.collect();
    let Some(last) = pieces.iter().rposition(|piece| width(piece) > 0) else {
        return Vec::new();
    };
    let mut line = Vec::new();
    for (i, (piece, wide)) in pieces.into_iter().zip(widths).enumerate().take(last + 1) {
        if i > 0 {
            line.push(spaces(2));
        }
        let short = wide.saturating_sub(width(&piece));
        line.extend(piece);
        if i < last {
            line.push(spaces(short));
        }
    }
    line
}

/// Lays out what one of the drawing built-ins made. `View::new` has checked
/// the parts, so a shape it rules out simply draws as nothing here.
fn draw(view: &View) -> Result<Block, String> {
    let text = |lines: Vec<Vec<Span>>| lines.into_iter().map(Line::Text).collect();
    Ok(match (view.kind, &*view.parts) {
        (ViewKind::Style, [content, names @ ..]) => {
            let style = Style::named(names);
            let restyle = |line| match line {
                Line::Text(mut spans) => {
                    spans.iter_mut().for_each(|span| span.style = span.style.inside(style));
                    Line::Text(spans)
                }
                Line::Rule(inner) => Line::Rule(inner.inside(style)),
            };
            build(content)?.into_iter().map(restyle).collect()
        }
        (ViewKind::Box, [content, title @ ..]) => {
            let lines = fit(build(content)?);
            let title = match title {
                [Value::Str(title)] => Some(title.replace(['\r', '\n'], " ")),
                _ => None,
            };
            let title_width = title.as_ref().map_or(0, |title| title.chars().count());
            // Room for the title, and for a corner piece on each side of it.
            let inner = widest(&lines).max(if title.is_some() { title_width + 2 } else { 0 });
            let top = match &title {
                Some(title) => {
                    format!("\u{250c}\u{2500} {title} {}\u{2510}", "\u{2500}".repeat(inner - title_width - 1))
                }
                None => format!("\u{250c}{}\u{2510}", "\u{2500}".repeat(inner + 2)),
            };
            let mut boxed = vec![vec![plain(top)]];
            for line in lines {
                let short = inner - width(&line);
                let mut spans = vec![plain("\u{2502} ")];
                spans.extend(line);
                spans.push(plain(format!("{} \u{2502}", " ".repeat(short))));
                boxed.push(spans);
            }
            boxed.push(vec![plain(format!("\u{2514}{}\u{2518}", "\u{2500}".repeat(inner + 2)))]);
            text(boxed)
        }
        (ViewKind::Row, items) => {
            let mut blocks = Vec::with_capacity(items.len());
            for item in items {
                blocks.push(fit(build(item)?));
            }
            let widths: Vec<usize> = blocks.iter().map(|block| widest(block)).collect();
            let tall = blocks.iter().map(Vec::len).max().unwrap_or(0);
            let line = |row: usize| {
                let across = blocks.iter().map(|block| block.get(row).cloned().unwrap_or_default());
                beside(across, &widths)
            };
            text((0..tall).map(line).collect())
        }
        (ViewKind::Rule, _) => vec![Line::Rule(Style::default())],
        (ViewKind::Bar, [Value::Int(part), Value::Int(whole), width @ ..]) if *whole > 0 => {
            let across = match width {
                [Value::Int(width)] => *width,
                _ => BAR_WIDTH,
            };
            // In wider integers, so that no part or whole can overflow.
            let (part, whole, across) = ((*part).clamp(0, *whole) as i128, *whole as i128, across as i128);
            let rounded = |of: i128| (part * of * 2 + whole) / (whole * 2);
            let full = rounded(across) as usize;
            let bar = "\u{2588}".repeat(full) + &"\u{2591}".repeat(across as usize - full);
            vec![Line::Text(vec![plain(format!("{bar} {:>3}%", rounded(100)))])]
        }
        (ViewKind::Tree, [label, Value::List(children)]) => {
            let mut lines = fit(build(label)?);
            for (i, child) in children.iter().enumerate() {
                let last = i + 1 == children.len();
                for (j, line) in fit(build(child)?).into_iter().enumerate() {
                    let branch = match (j == 0, last) {
                        (true, false) => "\u{251c}\u{2500} ",
                        (true, true) => "\u{2514}\u{2500} ",
                        (false, false) => "\u{2502}  ",
                        (false, true) => "   ",
                    };
                    let mut spans = vec![plain(branch)];
                    spans.extend(line);
                    lines.push(spans);
                }
            }
            text(lines)
        }
        (ViewKind::Fields, [Value::List(labels), Value::List(values)]) => {
            let side = |items: &Rc<[Value]>| -> Result<Vec<Vec<Span>>, String> {
                items.iter().map(inline).collect()
            };
            let (labels, values) = (side(labels)?, side(values)?);
            let widths = [widest(&labels), widest(&values)];
            let line = |row: usize| {
                let pair = [&labels, &values].map(|side| side.get(row).cloned().unwrap_or_default());
                beside(pair.into_iter(), &widths)
            };
            text((0..labels.len().max(values.len())).map(line).collect())
        }
        _ => Vec::new(),
    })
}

/// Whether the styles in a value should reach the terminal. Decided once.
///
/// `NO_COLOR` turns them off and `CLICOLOR_FORCE` turns them on, as other
/// programs have it. Otherwise they are on when the output is a terminal
/// that understands them, and off when it is a file or a pipe.
fn wants_styles() -> bool {
    static CHOICE: OnceLock<bool> = OnceLock::new();
    *CHOICE.get_or_init(|| {
        let var = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
        if var("NO_COLOR").is_some() {
            false
        } else if var("CLICOLOR_FORCE").is_some_and(|value| value != "0") {
            true
        } else {
            let dumb = var("TERM").is_some_and(|term| term == "dumb");
            !dumb && std::io::stdout().is_terminal() && console::takes_styles()
        }
    })
}

#[cfg(not(windows))]
mod console {
    /// Every terminal worth the name understands the codes.
    pub fn takes_styles() -> bool {
        true
    }
}

/// The Windows console shows the codes as stray characters unless it is
/// told to act on them, and the standard library has no way to tell it.
/// These three functions of the system are all that takes.
#[cfg(windows)]
mod console {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;

    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetConsoleMode(console: *mut c_void, mode: *mut u32) -> i32;
        fn SetConsoleMode(console: *mut c_void, mode: u32) -> i32;
    }

    /// Asks the console that standard output goes to to act on the codes.
    /// False if it will not, which is how the oldest consoles answer.
    pub fn takes_styles() -> bool {
        let console = std::io::stdout().as_raw_handle().cast::<c_void>();
        let mut mode = 0_u32;
        // SAFETY: the handle is this process's standard output, which stays
        // open for as long as the process runs, and `mode` is a `u32` that
        // outlives the call that writes to it.
        if unsafe { GetConsoleMode(console, &mut mode) } == 0 {
            // Not a console but still a terminal: the kind Git Bash runs
            // in, which is a pipe to a program that does understand them.
            return true;
        }
        if mode & ENABLE_VIRTUAL_TERMINAL_PROCESSING != 0 {
            return true;
        }
        // SAFETY: as above. The mode is the one just read, with one more
        // flag set.
        unsafe { SetConsoleMode(console, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0 }
    }
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

    /// Runs a script and renders its value with its styles shown.
    fn styled(source: &str) -> String {
        script::run(source.as_bytes(), Path::new(""), crate::input::Vars::NONE, |outcome| {
            super::render_as(&outcome.value, true)
        })
        .unwrap_or_else(|error| panic!("{source}\nfailed: {error}"))
        .unwrap()
    }

    #[test]
    fn text_keeps_the_spaces_it_ends_with() {
        assert_eq!(rendered("\"name: \"").unwrap(), "name: \n");
        assert_eq!(rendered("\"one\\r\\ntwo\\n\\nfour\"").unwrap(), "one\ntwo\n\nfour\n");
        assert_eq!(rendered("\"\"").unwrap(), "\n");
    }

    #[test]
    fn a_box_goes_around_what_it_holds() {
        assert_eq!(rendered(r#"(box "hello")"#).unwrap(), "┌───────┐\n│ hello │\n└───────┘\n");
        assert_eq!(
            rendered(r#"(box (list "one" "three") "Title")"#).unwrap(),
            "\
┌─ Title ─┐
│ one     │
│ three   │
└─────────┘
"
        );
        // The border follows whichever is wider, the title or what is inside.
        assert_eq!(
            rendered(r#"(box "a longer line" "T")"#).unwrap(),
            "┌─ T ───────────┐\n│ a longer line │\n└───────────────┘\n"
        );
        assert_eq!(rendered("(box ())").unwrap(), "┌──┐\n└──┘\n");
        assert_eq!(
            rendered(r#"(box (col (col-def "File" (list "a.md" "b.md")) (col-def "Size" (list 1 20))) "Lore")"#)
                .unwrap(),
            "\
┌─ Lore ─────┐
│ File  Size │
│ ----  ---- │
│ a.md  1    │
│ b.md  20   │
└────────────┘
"
        );
    }

    #[test]
    fn a_row_puts_things_side_by_side() {
        assert_eq!(
            rendered(r#"(row (list "a" "bbb") "c" (list "d" "e" "f"))"#).unwrap(),
            "a    c  d\nbbb     e\n        f\n"
        );
        assert_eq!(
            rendered(r#"(row (box "a" "L") (box (list "b" "c")))"#).unwrap(),
            "\
┌─ L ─┐  ┌───┐
│ a   │  │ b │
└─────┘  │ c │
         └───┘
"
        );
        // Nothing in it is nothing at all.
        assert_eq!(rendered("(row () ())").unwrap(), "");
    }

    #[test]
    fn a_rule_is_as_wide_as_what_it_is_among() {
        assert_eq!(rendered(r#"(list "Heading" (rule) "x")"#).unwrap(), "Heading\n───────\nx\n");
        assert_eq!(
            rendered(r#"(box (list "ab" (rule) "abcd"))"#).unwrap(),
            "┌──────┐\n│ ab   │\n│ ──── │\n│ abcd │\n└──────┘\n"
        );
        // Each side of a row is measured on its own.
        assert_eq!(rendered(r#"(row (list "ab" (rule)) (list "wider" (rule)))"#).unwrap(), "ab  wider\n──  ─────\n");
        // With nothing to match, it picks a width.
        assert_eq!(rendered("(rule)").unwrap(), format!("{}\n", "─".repeat(40)));
    }

    #[test]
    fn a_bar_shows_a_share() {
        assert_eq!(rendered("(bar 7 10 10)").unwrap(), "███████░░░  70%\n");
        assert_eq!(rendered("(bar 1 3 6)").unwrap(), "██░░░░  33%\n");
        assert_eq!(rendered("(bar 0 5 4)").unwrap(), "░░░░   0%\n");
        assert_eq!(rendered("(bar 5 5 4)").unwrap(), "████ 100%\n");
        assert_eq!(rendered("(bar 1 2)").unwrap(), format!("{}{}  50%\n", "█".repeat(10), "░".repeat(10)));
        // More than the whole is all of it, and less than none is none.
        assert_eq!(rendered("(bar 9 5 4)").unwrap(), "████ 100%\n");
        assert_eq!(rendered("(bar -3 5 4)").unwrap(), "░░░░   0%\n");
        assert_eq!(
            rendered("(bar 9223372036854775807 9223372036854775807 200)").unwrap(),
            format!("{} 100%\n", "█".repeat(200))
        );
    }

    #[test]
    fn a_tree_draws_branches() {
        let source = r#"
            (tree "docs" (list
              "cache.md"
              (tree "lore" (list "a.md" (tree "old" (list "x.md")) "b.md"))
              (list "two" "lines")
              "yio.md"))"#;
        assert_eq!(
            rendered(source).unwrap(),
            "\
docs
├─ cache.md
├─ lore
│  ├─ a.md
│  ├─ old
│  │  └─ x.md
│  └─ b.md
├─ two
│  lines
└─ yio.md
"
        );
        assert_eq!(rendered(r#"(tree "empty" ())"#).unwrap(), "empty\n");
    }

    #[test]
    fn fields_line_up_their_values() {
        assert_eq!(
            rendered(r#"(fields (list "Files" "Drafts" "Size" "Done") (list 12 "3 of 12" () (bar 1 4 8)))"#)
                .unwrap(),
            "Files   12\nDrafts  3 of 12\nSize\nDone    ██░░░░░░  25%\n"
        );
        // A side that runs out is blank.
        assert_eq!(rendered(r#"(fields (list "A") (list 1 2))"#).unwrap(), "A  1\n   2\n");
    }

    /// The example under "Layout and style" in the reference shows a script
    /// and what it draws, so the two have to agree.
    #[test]
    fn the_layout_example_in_the_api_doc_is_what_it_draws() {
        // Git on Windows may check the document out with CRLF line endings.
        let doc = include_str!("../../docs/scripting-api.md").replace("\r\n", "\n");
        let section = doc.split("### Layout and style").nth(1).unwrap();
        let mut blocks = section.split("```");
        let source = blocks.nth(1).unwrap().strip_prefix("lisp\n").unwrap();
        let drawn = blocks.nth(1).unwrap().strip_prefix('\n').unwrap();
        assert_eq!(rendered(source).unwrap(), drawn);
    }

    #[test]
    fn styles_show_only_when_asked_for() {
        assert_eq!(rendered(r#"(style "hi" "bold" "red")"#).unwrap(), "hi\n");
        assert_eq!(styled(r#"(style "hi" "bold" "red")"#), "\x1b[1;31mhi\x1b[0m\n");
        assert_eq!(styled(r#"(style "hi" "dim" "italic" "underline" "gray")"#), "\x1b[2;3;4;90mhi\x1b[0m\n");
        // The later colour wins, and unstyled text has no codes at all.
        assert_eq!(styled(r#"(style "hi" "red" "green")"#), "\x1b[32mhi\x1b[0m\n");
        assert_eq!(styled(r#"(list "plain" (box "x"))"#), rendered(r#"(list "plain" (box "x"))"#).unwrap());
    }

    #[test]
    fn a_style_reaches_everything_inside_it() {
        // What is inside keeps its own colour and gains the rest.
        assert_eq!(
            styled(r#"(style (list "a" (style "b" "green")) "red" "bold")"#),
            "\x1b[1;31ma\x1b[0m\n\x1b[1;32mb\x1b[0m\n"
        );
        assert_eq!(styled(r#"(style (rule) "dim")"#), format!("\x1b[2m{}\x1b[0m\n", "─".repeat(40)));
        // Codes take no room: the columns line up as they would without.
        let source = r#"(col (col-def "State" (list (style "ok" "green") "broken")) (col-def "N" (list 1 2)))"#;
        assert_eq!(
            styled(source),
            "State   N\n------  -\n\x1b[32mok\x1b[0m      1\nbroken  2\n"
        );
        assert_eq!(rendered(source).unwrap(), "State   N\n------  -\nok      1\nbroken  2\n");
    }

    #[test]
    fn functions_cannot_be_rendered() {
        let message = "a script's result cannot contain a function";
        assert_eq!(rendered("(box +)").unwrap_err(), message);
        assert_eq!(rendered("(row 1 (tree \"a\" (list trim)))").unwrap_err(), message);
        assert_eq!(rendered("(fields (list len) ())").unwrap_err(), message);
        assert_eq!(rendered("(fn (x) x)").unwrap_err(), message);
        assert_eq!(rendered("(list 1 +)").unwrap_err(), message);
        assert_eq!(rendered("(col (col-def \"F\" (list trim)))").unwrap_err(), message);
    }
}
