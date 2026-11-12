//! Reads the text of a `.yio` file into forms.
//!
//! The input is bytes rather than a string because a bytes literal holds raw
//! data, which is how `.yio/cache.yio` carries its payload. Everything else
//! in a file must be UTF-8.

use std::fmt;

/// A 1-based position in the source. Columns count characters, not bytes.
/// The payload of a bytes literal does not advance the position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pos {
    pub line: usize,
    pub column: usize,
}

/// One syntactic element and where it starts.
#[derive(Debug, Clone, PartialEq)]
pub struct Form {
    pub kind: FormKind,
    pub pos: Pos,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FormKind {
    Symbol(String),
    Int(i64),
    Bool(bool),
    Str(String),
    /// `#<length>:<raw bytes>`.
    Bytes(Vec<u8>),
    /// `(...)`. The empty list `()` is the empty value.
    List(Vec<Form>),
}

#[derive(Debug, PartialEq)]
pub struct Error {
    pub pos: Pos,
    pub message: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}:{}: {}", self.pos.line, self.pos.column, self.message)
    }
}

/// How deeply lists may nest. Everything that walks forms afterwards
/// recurses, including dropping them, so unbounded nesting in a file would
/// overflow the stack rather than report an error.
const MAX_DEPTH: usize = 256;

/// Reads every top-level form in `source`.
pub fn read(source: &[u8]) -> Result<Vec<Form>, Error> {
    let mut reader = Reader { source, at: 0, line: 1, column: 1 };
    let mut top = Vec::new();
    // Lists still open, innermost last, each with where it began.
    let mut open: Vec<(Pos, Vec<Form>)> = Vec::new();

    loop {
        reader.skip_blank();
        let pos = reader.pos();
        let Some(byte) = reader.peek() else {
            break;
        };
        let kind = match byte {
            b'(' => {
                if open.len() == MAX_DEPTH {
                    let message = format!("lists are nested more than {MAX_DEPTH} deep");
                    return Err(error(pos, &message));
                }
                reader.bump();
                open.push((pos, Vec::new()));
                continue;
            }
            b')' => {
                reader.bump();
                let Some((start, items)) = open.pop() else {
                    return Err(error(pos, "unexpected `)`"));
                };
                let list = Form { kind: FormKind::List(items), pos: start };
                open.last_mut().map_or(&mut top, |(_, items)| items).push(list);
                continue;
            }
            b'"' => FormKind::Str(reader.string(pos)?),
            b'#' => FormKind::Bytes(reader.bytes(pos)?),
            byte if is_symbol_byte(byte) => reader.atom(pos)?,
            _ => {
                let found = reader.char_at(reader.at);
                return Err(error(pos, &format!("unexpected character {found:?}")));
            }
        };
        let form = Form { kind, pos };
        open.last_mut().map_or(&mut top, |(_, items)| items).push(form);
    }

    match open.last() {
        Some((start, _)) => Err(error(*start, "`(` is never closed")),
        None => Ok(top),
    }
}

fn error(pos: Pos, message: &str) -> Error {
    Error { pos, message: message.to_string() }
}

/// The characters a symbol or number may contain.
fn is_symbol_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"-_?!*/%<>=+".contains(&byte)
}

struct Reader<'a> {
    source: &'a [u8],
    /// Index of the next byte.
    at: usize,
    line: usize,
    column: usize,
}

impl Reader<'_> {
    fn pos(&self) -> Pos {
        Pos { line: self.line, column: self.column }
    }

    fn peek(&self) -> Option<u8> {
        self.source.get(self.at).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.at += 1;
        if byte == b'\n' {
            self.line += 1;
            self.column = 1;
        } else if byte & 0xC0 != 0x80 {
            // Not a UTF-8 continuation byte, so it starts a character.
            self.column += 1;
        }
        Some(byte)
    }

    /// The character starting at byte `index`, for error messages.
    fn char_at(&self, index: usize) -> char {
        let end = (index + 4).min(self.source.len());
        let text = String::from_utf8_lossy(&self.source[index..end]);
        text.chars().next().unwrap_or(char::REPLACEMENT_CHARACTER)
    }

    /// Skips whitespace and `--` comments.
    fn skip_blank(&mut self) {
        loop {
            match self.peek() {
                Some(b' ' | b'\t' | b'\r' | b'\n') => {
                    self.bump();
                }
                Some(b'-') if self.source[self.at..].starts_with(b"--") => {
                    while self.peek().is_some_and(|byte| byte != b'\n') {
                        self.bump();
                    }
                }
                _ => return,
            }
        }
    }

    /// Reads a string literal. The opening quote is the next byte.
    fn string(&mut self, start: Pos) -> Result<String, Error> {
        let unclosed = || error(start, "string is never closed");
        self.bump();
        let mut bytes = Vec::new();
        loop {
            let pos = self.pos();
            match self.bump().ok_or_else(unclosed)? {
                b'"' => break,
                b'\\' => {
                    let escaped = self.at;
                    bytes.push(match self.bump().ok_or_else(unclosed)? {
                        b'n' => b'\n',
                        b't' => b'\t',
                        b'r' => b'\r',
                        b'"' => b'"',
                        b'\\' => b'\\',
                        _ => {
                            let found = self.char_at(escaped);
                            return Err(error(pos, &format!("unknown escape `\\{found}`")));
                        }
                    });
                }
                byte => bytes.push(byte),
            }
        }
        String::from_utf8(bytes).map_err(|_| error(start, "string is not valid UTF-8"))
    }

    /// Reads a bytes literal, `#<length>:<raw bytes>`. The `#` is the next
    /// byte.
    fn bytes(&mut self, start: Pos) -> Result<Vec<u8>, Error> {
        self.bump();
        let digits = self.at;
        while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            self.bump();
        }
        if self.at == digits {
            return Err(error(start, "expected a length after `#`, as in `#3:abc`"));
        }
        let text = std::str::from_utf8(&self.source[digits..self.at]).expect("digits are ASCII");
        let length: usize = text
            .parse()
            .map_err(|_| error(start, &format!("bytes length `{text}` is too large")))?;
        if self.peek() != Some(b':') {
            return Err(error(self.pos(), "expected `:` after the bytes length"));
        }
        self.bump();

        let remaining = self.source.len() - self.at;
        if length > remaining {
            return Err(error(
                start,
                &format!("bytes literal declares {length} bytes but only {remaining} follow"),
            ));
        }
        let payload = self.source[self.at..self.at + length].to_vec();
        self.at += length;
        Ok(payload)
    }

    /// Reads a symbol, integer, or boolean.
    fn atom(&mut self, start: Pos) -> Result<FormKind, Error> {
        let begin = self.at;
        while self.peek().is_some_and(is_symbol_byte) {
            self.bump();
        }
        let text = std::str::from_utf8(&self.source[begin..self.at]).expect("symbols are ASCII");

        let digits = text.strip_prefix('-').unwrap_or(text);
        if digits.starts_with(|c: char| c.is_ascii_digit()) {
            if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(error(start, &format!("`{text}` is not a valid integer")));
            }
            return text
                .parse()
                .map(FormKind::Int)
                .map_err(|_| error(start, &format!("integer `{text}` is out of range")));
        }
        Ok(match text {
            "true" => FormKind::Bool(true),
            "false" => FormKind::Bool(false),
            _ => FormKind::Symbol(text.to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads `source` and drops positions, leaving a compact description.
    fn shape(source: &[u8]) -> String {
        fn describe(form: &Form) -> String {
            match &form.kind {
                FormKind::Symbol(name) => name.clone(),
                FormKind::Int(value) => format!("int:{value}"),
                FormKind::Bool(value) => format!("bool:{value}"),
                FormKind::Str(text) => format!("{text:?}"),
                FormKind::Bytes(bytes) => format!("bytes:{bytes:?}"),
                FormKind::List(items) => {
                    let items: Vec<String> = items.iter().map(describe).collect();
                    format!("({})", items.join(" "))
                }
            }
        }
        let forms = read(source).unwrap();
        forms.iter().map(describe).collect::<Vec<_>>().join(" ")
    }

    fn failure(source: &[u8]) -> String {
        read(source).unwrap_err().to_string()
    }

    #[test]
    fn reads_atoms() {
        assert_eq!(shape(b"list-dir is-empty? <= - + % a--b _x"), "list-dir is-empty? <= - + % a--b _x");
        assert_eq!(shape(b"0 42 -7 -9223372036854775808"), "int:0 int:42 int:-7 int:-9223372036854775808");
        assert_eq!(shape(b"true false truthy return"), "bool:true bool:false truthy return");
        assert_eq!(shape(b"-x +5"), "-x +5");
    }

    #[test]
    fn reads_strings() {
        assert_eq!(shape(br#""" "lore" "a b""#), r#""" "lore" "a b""#);
        assert_eq!(shape(br#""tab\there" "q\"q" "back\\slash" "cr\r""#), r#""tab\there" "q\"q" "back\\slash" "cr\r""#);
        assert_eq!(shape("\"two\nlines\" \"caf\u{e9} \u{1f600}\"".as_bytes()), "\"two\\nlines\" \"caf\u{e9} \u{1f600}\"");
        // Delimiters and comment markers mean nothing inside a string.
        assert_eq!(shape(br#""(not -- a comment)""#), r#""(not -- a comment)""#);
    }

    #[test]
    fn reads_lists() {
        assert_eq!(shape(b"()"), "()");
        assert_eq!(shape(b"(a (b () (c 1)) \"d\")"), "(a (b () (c int:1)) \"d\")");
        assert_eq!(shape(b"(a)(b)x\"y\"(c)"), "(a) (b) x \"y\" (c)");
        assert_eq!(shape(b""), "");
    }

    #[test]
    fn skips_comments() {
        assert_eq!(shape(b"-- leading\n(a -- trailing (\n b)\n-- last, no newline"), "(a b)");
        assert_eq!(shape(b"(a)--right after\n1"), "(a) int:1");
        assert_eq!(shape(b"--"), "");
    }

    #[test]
    fn reads_bytes_literals() {
        assert_eq!(shape(b"#0:"), "bytes:[]");
        assert_eq!(shape(b"#3:abc"), "bytes:[97, 98, 99]");
        // The payload is taken by length, whatever it contains.
        assert_eq!(shape(b"(cache #6:)\n\xff\"(-)"), "(cache bytes:[41, 10, 255, 34, 40, 45])");
        assert_eq!(shape(b"#2:ab#1:c x"), "bytes:[97, 98] bytes:[99] x");
    }

    #[test]
    fn reads_the_design_sketch() {
        let source = br#"
-- list-dir lists the files in a directory
(var lore-files (list-dir "lore"))

(if (is-empty lore-files)
  return)

(fn read-status (file)
  -- () is an empty value
  (var output ())
  output)

(col
  (col-def
    "File path"
    ((fn (files) files) lore-files))
  (col-def
    "Status"
    (map lore-files read-status)))
"#;
        assert_eq!(
            shape(source),
            "(var lore-files (list-dir \"lore\")) \
             (if (is-empty lore-files) return) \
             (fn read-status (file) (var output ()) output) \
             (col (col-def \"File path\" ((fn (files) files) lore-files)) \
             (col-def \"Status\" (map lore-files read-status)))"
        );
    }

    #[test]
    fn records_positions() {
        let forms = read("(a\r\n  \"\u{e9}\u{e9}\" b)\n  c".as_bytes()).unwrap();
        let pos = |form: &Form| (form.pos.line, form.pos.column);
        assert_eq!(pos(&forms[0]), (1, 1));
        let FormKind::List(items) = &forms[0].kind else {
            panic!("expected a list");
        };
        assert_eq!(pos(&items[0]), (1, 2));
        assert_eq!(pos(&items[1]), (2, 3));
        // The two-byte characters each count as one column.
        assert_eq!(pos(&items[2]), (2, 8));
        assert_eq!(pos(&forms[1]), (3, 3));
    }

    #[test]
    fn rejects_bad_input() {
        let cases: [(&[u8], &str); 17] = [
            (b"(a))", "1:4: unexpected `)`"),
            (b")", "1:1: unexpected `)`"),
            (b"(a\n  (b c)\n  (d", "3:3: `(` is never closed"),
            (b"(a \"open", "1:4: string is never closed"),
            (b"\"trailing\\", "1:1: string is never closed"),
            (b"\"bad \\q\"", "1:6: unknown escape `\\q`"),
            (b"\"\xff\"", "1:1: string is not valid UTF-8"),
            (b"12ab", "1:1: `12ab` is not a valid integer"),
            (b"(x -3-4)", "1:4: `-3-4` is not a valid integer"),
            (b"9223372036854775808", "1:1: integer `9223372036854775808` is out of range"),
            (b"'quoted", "1:1: unexpected character '\\''"),
            (b"(a [b])", "1:4: unexpected character '['"),
            ("caf\u{e9}".as_bytes(), "1:4: unexpected character '\u{e9}'"),
            (b"#:abc", "1:1: expected a length after `#`, as in `#3:abc`"),
            (b"(cache #3abc)", "1:10: expected `:` after the bytes length"),
            (b"#5:abc", "1:1: bytes literal declares 5 bytes but only 3 follow"),
            (b"#99999999999999999999999:", "1:1: bytes length `99999999999999999999999` is too large"),
        ];
        for (source, expected) in cases {
            assert_eq!(failure(source), expected, "{:?}", String::from_utf8_lossy(source));
        }
    }

    #[test]
    fn limits_nesting_depth() {
        let nested = |depth: usize| {
            let mut source = vec![b'('; depth];
            source.extend(std::iter::repeat_n(b')', depth));
            source
        };
        assert_eq!(read(&nested(MAX_DEPTH)).unwrap().len(), 1);
        assert_eq!(
            failure(&nested(MAX_DEPTH + 1)),
            "1:257: lists are nested more than 256 deep"
        );
        // Far past the limit, the error still arrives instead of a crash.
        assert!(read(&nested(1_000_000)).is_err());
    }
}
