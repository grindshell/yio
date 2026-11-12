//! Loads a `yio.md` file into a [`Command`] tree.
//!
//! This is a line-based reader for the subset of Markdown that `yio.md` uses,
//! not a general Markdown parser.

use std::io;
use std::path::{Component, Path, PathBuf};

use crate::command::{
    Command, Flag, FlagType, Param, check_var_name, is_boolean, same_ignoring_case,
};

pub const FILE_NAME: &str = "yio.md";

/// How deep embeds may nest. Cycles are caught by path, so this only stops
/// loops that paths cannot reveal, such as a symlink to a parent directory.
const MAX_DEPTH: usize = 32;

/// Why a file could not be provided.
pub enum ReadError {
    Io(io::Error),
    /// Something that had to happen before the file was read failed, such as
    /// a plugin. The message stands on its own.
    Failed(String),
}

impl From<io::Error> for ReadError {
    fn from(error: io::Error) -> Self {
        ReadError::Io(error)
    }
}

/// Provides the text of a `yio.md`. Paths are relative to the current
/// directory. This is called exactly once for each file that is loaded,
/// just before it is needed.
pub type Read<'r> = &'r dyn Fn(&Path) -> Result<String, ReadError>;

/// Loads the `yio.md` at `file`, whose contents are `source`, along with
/// every file it embeds. Error messages name the file and 1-based line.
pub fn load(file: &Path, source: &str, read: Read) -> Result<Command, String> {
    let mut open = vec![normalize(file)];
    load_source(source, &mut open, read)
}

/// Loads a single file with nothing available to embed.
#[cfg(test)]
pub fn parse(source: &str) -> Result<Command, String> {
    let read = |_: &Path| Err(ReadError::Io(io::Error::from(io::ErrorKind::NotFound)));
    load(Path::new(FILE_NAME), source, &read)
}

/// Loads the file at the top of `open`, the chain of files being loaded.
fn load_source(source: &str, open: &mut Vec<PathBuf>, read: Read) -> Result<Command, String> {
    let file = open.last().expect("the file being loaded is open").clone();
    read_lines(source, &file, open, read).map_err(|error| match error {
        Error::Line(line_no, message) => format!("{}: line {line_no}: {message}", show(&file)),
        Error::Embedded(message) => message,
    })
}

enum Error {
    /// A problem in the file being read, at a 1-based line.
    Line(usize, String),
    /// A problem in an embedded file. The message already names that file.
    Embedded(String),
}

fn err(line_no: usize, message: &str) -> Error {
    Error::Line(line_no, message.to_string())
}

fn read_lines(
    source: &str,
    file: &Path,
    open: &mut Vec<PathBuf>,
    read: Read,
) -> Result<Command, Error> {
    let lines: Vec<&str> = source.lines().collect();
    let dir = file.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut loader = Loader::new(dir, open, read);
    let mut i = 0;
    while i < lines.len() {
        let line_no = i + 1;
        let line = lines[i];
        let text = line.trim();

        if text.is_empty() {
            loader.end_paragraph()?;
            i += 1;
        } else if indentation(line) >= 4 {
            // Markdown treats this as an indented code block: literal text,
            // never a script, heading, or marker.
            loader.paragraph_line(line_no, text);
            i += 1;
        } else if text.starts_with("<!--") {
            // An HTML comment hides everything through the line that closes
            // it, so a commented-out code block must not become a script.
            loader.end_paragraph()?;
            let closes = |j: usize| {
                let rest = if j == i { &text[2..] } else { lines[j] };
                rest.contains("-->")
            };
            let end = (i..lines.len())
                .find(|&j| closes(j))
                .ok_or_else(|| err(line_no, "comment is never closed"))?;
            i = end + 1;
        } else if let Some(fence) = Fence::open(line) {
            loader.end_paragraph()?;
            let start = i + 1;
            let end = (start..lines.len())
                .find(|&j| fence.closes(lines[j]))
                .ok_or_else(|| err(line_no, "code block is never closed"))?;
            loader.script(line_no, &fence, &lines[start..end])?;
            i = end + 1;
        } else if is_thematic_break(text) {
            loader.end_paragraph()?;
            i += 1;
        } else if let Some((level, title)) = heading(line) {
            loader.end_paragraph()?;
            loader.heading(line_no, level, title)?;
            i += 1;
        } else if text.starts_with('>') {
            loader.end_paragraph()?;
            let mut parts = Vec::new();
            while i < lines.len() && lines[i].trim().starts_with('>') {
                parts.push(lines[i].trim()[1..].trim());
                i += 1;
            }
            loader.short_help(&parts.join(" "));
        } else if text == "**FLAGS**" {
            loader.end_paragraph()?;
            i = loader.flags(line_no, &lines, i + 1)?;
        } else {
            loader.paragraph_line(line_no, text);
            i += 1;
        }
    }
    loader.end_paragraph()?;
    Ok(loader.finish())
}

/// A command whose heading is still in scope.
struct Open {
    level: usize,
    command: Command,
    /// Short and long help of an embedded file, used if the heading turns
    /// out to have none of its own.
    embedded_help: (Option<String>, Option<String>),
}

impl Open {
    fn new(level: usize, command: Command) -> Self {
        Open { level, command, embedded_help: (None, None) }
    }

    fn close(self) -> Command {
        let mut command = self.command;
        let (short, long) = self.embedded_help;
        command.short_help = command.short_help.or(short);
        command.long_help = command.long_help.or(long);
        command
    }
}

struct Loader<'a, 'c> {
    /// Commands still open. The root is at level 1 and is always present.
    stack: Vec<Open>,
    paragraph: Vec<&'a str>,
    paragraph_line: usize,
    seen_h1: bool,
    /// Directory of the file being read, relative to the current directory.
    /// Empty for the current directory itself.
    dir: PathBuf,
    /// The chain of files being loaded, ending with the one being read.
    open: &'c mut Vec<PathBuf>,
    read: Read<'c>,
}

impl<'a, 'c> Loader<'a, 'c> {
    fn new(dir: PathBuf, open: &'c mut Vec<PathBuf>, read: Read<'c>) -> Self {
        let mut loader = Loader {
            stack: Vec::new(),
            paragraph: Vec::new(),
            paragraph_line: 0,
            seen_h1: false,
            dir,
            open,
            read,
        };
        let root = loader.located(Command::root());
        loader.stack.push(Open::new(1, root));
        loader
    }

    /// Records which directory `command` was defined in.
    fn located(&self, mut command: Command) -> Command {
        command.dir = (!self.dir.as_os_str().is_empty()).then(|| self.dir.clone());
        command
    }

    fn top(&mut self) -> &mut Open {
        self.stack.last_mut().expect("root is always present")
    }

    fn current(&mut self) -> &mut Command {
        &mut self.top().command
    }

    fn in_command(&self) -> bool {
        self.stack.len() > 1
    }

    /// Closes every open command at `level` or deeper, attaching each to the
    /// nearest shallower heading.
    fn close_to(&mut self, level: usize) {
        while self.stack.len() > 1 && self.stack[self.stack.len() - 1].level >= level {
            let done = self.stack.pop().expect("checked above").close();
            self.current().subcommands.push(done);
        }
    }

    fn finish(mut self) -> Command {
        self.close_to(0);
        self.stack.pop().expect("root is always present").close()
    }

    /// Loads the `yio.md` that `target` links to and places its commands
    /// under the current one, as if they had been written here.
    fn embed(&mut self, line_no: usize, target: &str) -> Result<(), Error> {
        let file = normalize(&self.dir.join(target));
        let shown = show(&file);
        if self.open.contains(&file) {
            return Err(err(
                line_no,
                &format!("`{shown}` is already being loaded; embeds cannot form a cycle"),
            ));
        }
        if self.open.len() >= MAX_DEPTH {
            return Err(err(
                line_no,
                &format!("embeds are nested more than {MAX_DEPTH} files deep"),
            ));
        }
        let source = (self.read)(&file).map_err(|error| match error {
            ReadError::Io(cause) => err(line_no, &format!("could not read `{shown}`: {cause}")),
            ReadError::Failed(message) => Error::Embedded(message),
        })?;

        self.open.push(file);
        let embedded = load_source(&source, self.open, self.read);
        self.open.pop();
        let embedded = embedded.map_err(Error::Embedded)?;

        let top = self.top();
        for command in embedded.subcommands {
            if top.command.subcommands.iter().any(|c| c.name == command.name) {
                return Err(err(
                    line_no,
                    &format!("command `{}` from `{shown}` is already defined here", command.name),
                ));
            }
            top.command.subcommands.push(command);
        }
        let (short, long) = &mut top.embedded_help;
        *short = short.take().or(embedded.short_help);
        *long = long.take().or(embedded.long_help);
        Ok(())
    }

    fn heading(&mut self, line_no: usize, level: usize, title: &str) -> Result<(), Error> {
        if level == 1 {
            if self.seen_h1 || self.in_command() {
                return Err(err(
                    line_no,
                    "only one H1 is allowed, and it must come before any command",
                ));
            }
            self.seen_h1 = true;
            return Ok(());
        }
        self.close_to(level);
        let command = command_heading(title).map_err(|m| err(line_no, &m))?;
        let parent = self.current();
        if parent.subcommands.iter().any(|c| c.name == command.name) {
            return Err(err(
                line_no,
                &format!("command `{}` is defined twice", command.name),
            ));
        }
        let command = self.located(command);
        self.stack.push(Open::new(level, command));
        Ok(())
    }

    fn short_help(&mut self, text: &str) {
        match &mut self.current().short_help {
            Some(help) => {
                help.push(' ');
                help.push_str(text);
            }
            help => *help = Some(text.to_string()),
        }
    }

    fn paragraph_line(&mut self, line_no: usize, text: &'a str) {
        if self.paragraph.is_empty() {
            self.paragraph_line = line_no;
        }
        self.paragraph.push(text);
    }

    fn end_paragraph(&mut self) -> Result<(), Error> {
        if self.paragraph.is_empty() {
            return Ok(());
        }
        let text = self.paragraph.join("\n");
        self.paragraph.clear();
        if let Some(target) = embed_target(&text) {
            return self.embed(self.paragraph_line, target);
        }
        match &mut self.current().long_help {
            Some(help) => {
                help.push_str("\n\n");
                help.push_str(&text);
            }
            help => *help = Some(text),
        }
        Ok(())
    }

    fn script(&mut self, line_no: usize, fence: &Fence, body: &[&str]) -> Result<(), Error> {
        if !self.in_command() {
            return Err(err(line_no, "code block is outside of a command"));
        }
        let language = fence.info.split_whitespace().next().unwrap_or("");
        if !(language.is_empty() || language == "bash") {
            return Err(err(
                line_no,
                &format!("code block language `{language}` is not supported; only `bash` is"),
            ));
        }
        // Markdown removes the fence's own indentation from each line of the
        // block. Keeping it would break heredoc terminators, among others.
        let mut script = String::new();
        for line in body {
            let spaces = line.bytes().take(fence.indent).take_while(|b| *b == b' ');
            script.push_str(&line[spaces.count()..]);
            script.push('\n');
        }
        self.current().scripts.push(script);
        Ok(())
    }

    /// Reads the list that follows a `**FLAGS**` marker, starting at
    /// `lines[i]`. Returns the index of the first line after the list.
    fn flags(&mut self, marker_line: usize, lines: &[&str], mut i: usize) -> Result<usize, Error> {
        if !self.in_command() {
            return Err(err(marker_line, "`**FLAGS**` is outside of a command"));
        }
        let mut drafts: Vec<FlagDraft> = Vec::new();
        let mut top_indent = None;
        loop {
            let mut next = i;
            while next < lines.len() && lines[next].trim().is_empty() {
                next += 1;
            }
            let Some((indent, item)) = lines.get(next).and_then(|line| bullet(line)) else {
                break;
            };
            let line_no = next + 1;
            i = next + 1;
            if indent <= *top_indent.get_or_insert(indent) {
                drafts.push(FlagDraft::new(line_no, item)?);
            } else {
                let draft = drafts.last_mut().expect("first item is always top-level");
                draft.set(line_no, item)?;
            }
        }
        if drafts.is_empty() {
            return Err(err(marker_line, "`**FLAGS**` must be followed by a list"));
        }

        let command = self.current();
        for draft in drafts {
            let line_no = draft.line_no;
            let flag = draft.finish()?;
            let names = command.params.iter().map(|p| &p.name);
            let mut names = names.chain(command.flags.iter().map(|f| &f.name));
            let clash = if let Some(existing) =
                names.find(|name| name.eq_ignore_ascii_case(&flag.name))
            {
                if *existing != flag.name {
                    return Err(err(line_no, &same_ignoring_case(&flag.name, existing)));
                }
                Some(flag.name.clone())
            } else if let Some(c) = flag.short
                && command.flags.iter().any(|f| f.short == Some(c))
            {
                Some(format!("-{c}"))
            } else if let Some(name) = &flag.long
                && command.flags.iter().any(|f| f.long.as_ref() == Some(name))
            {
                Some(format!("--{name}"))
            } else {
                None
            };
            if let Some(clash) = clash {
                return Err(err(
                    line_no,
                    &format!("`{clash}` is already used by command `{}`", command.name),
                ));
            }
            command.flags.push(flag);
        }
        Ok(i)
    }
}

/// A flag being read from a `**FLAGS**` list.
struct FlagDraft {
    line_no: usize,
    flag: Flag,
    has_flags: bool,
}

impl FlagDraft {
    fn new(line_no: usize, item: &str) -> Result<Self, Error> {
        let name = unquote(item);
        check_var_name(name).map_err(|message| err(line_no, &message))?;
        Ok(FlagDraft {
            line_no,
            flag: Flag {
                name: name.to_string(),
                short: None,
                long: None,
                ty: FlagType::default(),
                default: None,
                description: None,
            },
            has_flags: false,
        })
    }

    fn set(&mut self, line_no: usize, item: &str) -> Result<(), Error> {
        let (key, value) = item
            .split_once(':')
            .ok_or_else(|| err(line_no, &format!("expected `key: value`, found `{item}`")))?;
        let value = value.trim();
        match key.trim() {
            "flags" => {
                let (short, long) = flag_names(value).map_err(|m| err(line_no, &m))?;
                self.flag.short = short;
                self.flag.long = long;
                self.has_flags = true;
            }
            "type" => {
                self.flag.ty = match unquote(value) {
                    "string" => FlagType::String,
                    "boolean" => FlagType::Boolean,
                    other => {
                        return Err(err(
                            line_no,
                            &format!("unknown flag type `{other}`; expected `string` or `boolean`"),
                        ));
                    }
                }
            }
            "default" => self.flag.default = Some(unquote(value).to_string()),
            "description" => self.flag.description = Some(value.to_string()),
            other => {
                return Err(err(
                    line_no,
                    &format!(
                        "unknown flag option `{other}`; expected `flags`, `type`, `default`, or `description`"
                    ),
                ));
            }
        }
        Ok(())
    }

    fn finish(self) -> Result<Flag, Error> {
        let name = &self.flag.name;
        if !self.has_flags {
            return Err(err(
                self.line_no,
                &format!("flag `{name}` is missing its `flags` option"),
            ));
        }
        if self.flag.ty == FlagType::Boolean
            && let Some(default) = &self.flag.default
            && !is_boolean(default)
        {
            return Err(err(
                self.line_no,
                &format!("boolean flag `{name}` has default `{default}`; expected `true` or `false`"),
            ));
        }
        Ok(self.flag)
    }
}

/// Parses the value of a `flags` option, such as `-j --jobs`.
fn flag_names(value: &str) -> Result<(Option<char>, Option<String>), String> {
    let mut short = None;
    let mut long = None;
    let tokens = value
        .split(|c: char| c.is_whitespace() || c == ',' || c == '`')
        .filter(|token| !token.is_empty());
    for token in tokens {
        if let Some(name) = token.strip_prefix("--") {
            let valid = !name.is_empty()
                && !name.starts_with('-')
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
            if !valid {
                return Err(format!("`{token}` is not a valid long flag"));
            }
            if long.replace(name.to_string()).is_some() {
                return Err("a flag can have only one long form".to_string());
            }
        } else if let Some(name) = token.strip_prefix('-') {
            let mut chars = name.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if c.is_ascii_alphanumeric() => {
                    if short.replace(c).is_some() {
                        return Err("a flag can have only one short form".to_string());
                    }
                }
                _ => return Err(format!("`{token}` is not a valid short flag")),
            }
        } else {
            return Err(format!("`{token}` is not a flag; expected `-x` or `--name`"));
        }
    }
    if short.is_none() && long.is_none() {
        return Err("`flags` needs at least one of `-x` or `--name`".to_string());
    }
    Ok((short, long))
}

/// Parses the text of an H2-or-deeper heading: a command name followed by
/// `(REQUIRED)` and `[OPTIONAL]` arguments.
fn command_heading(title: &str) -> Result<Command, String> {
    let mut tokens = title.split_whitespace();
    let name = tokens.next().ok_or("heading has no command name")?;
    if name.starts_with(['-', '(', '[']) {
        return Err(format!("`{name}` is not a valid command name"));
    }
    let mut params: Vec<Param> = Vec::new();
    for token in tokens {
        let (inner, required) =
            if let Some(inner) = token.strip_prefix('(').and_then(|t| t.strip_suffix(')')) {
                (inner, true)
            } else if let Some(inner) = token.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
                (inner, false)
            } else {
                return Err(format!(
                    "unexpected `{token}` after command `{name}`; expected `(ARG)` or `[ARG]`"
                ));
            };
        check_var_name(inner)?;
        if let Some(existing) = params.iter().find(|p| p.name.eq_ignore_ascii_case(inner)) {
            return Err(if existing.name == inner {
                format!("argument `{inner}` is defined twice")
            } else {
                same_ignoring_case(inner, &existing.name)
            });
        }
        if required && params.last().is_some_and(|p| !p.required) {
            return Err(format!(
                "required argument `({inner})` cannot follow an optional one"
            ));
        }
        params.push(Param {
            name: inner.to_string(),
            required,
        });
    }
    Ok(Command {
        name: name.to_string(),
        params,
        ..Command::default()
    })
}

/// An opening code fence.
struct Fence<'a> {
    ch: char,
    len: usize,
    info: &'a str,
    /// Columns the opening fence is indented by, at most three.
    indent: usize,
}

impl<'a> Fence<'a> {
    /// The caller has already ruled out lines indented four or more columns.
    fn open(line: &'a str) -> Option<Self> {
        let rest = line.trim_start();
        let ch = rest.chars().next().filter(|c| matches!(c, '`' | '~'))?;
        let len = rest.chars().take_while(|c| *c == ch).count();
        let info = rest[len..].trim();
        // Three backticks followed by more backticks is inline code, not a fence.
        if len < 3 || (ch == '`' && info.contains('`')) {
            return None;
        }
        Some(Fence { ch, len, info, indent: indentation(line) })
    }

    fn closes(&self, line: &str) -> bool {
        let text = line.trim();
        indentation(line) < 4 && text.len() >= self.len && text.chars().all(|c| c == self.ch)
    }
}

/// Width of a line's leading whitespace, with tabs advancing to the next
/// multiple of four.
fn indentation(line: &str) -> usize {
    let mut width = 0;
    for c in line.chars() {
        match c {
            ' ' => width += 1,
            '\t' => width += 4 - width % 4,
            _ => break,
        }
    }
    width
}

/// Splits an ATX heading into its level and title. The caller has already
/// ruled out lines indented four or more columns.
fn heading(line: &str) -> Option<(usize, &str)> {
    let rest = line.trim_start();
    let level = rest.chars().take_while(|c| *c == '#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let rest = &rest[level..];
    if !(rest.is_empty() || rest.starts_with(char::is_whitespace)) {
        return None;
    }
    let title = rest.trim();
    // Drop an optional closing run of `#`.
    let open = title.trim_end_matches('#');
    let title = if open.is_empty() || open.ends_with(char::is_whitespace) {
        open.trim_end()
    } else {
        title
    };
    Some((level, title))
}

/// Splits a list item into its indentation and text.
fn bullet(line: &str) -> Option<(usize, &str)> {
    let rest = line.trim_start();
    let indent = line.len() - rest.len();
    let item = rest.strip_prefix(['-', '*', '+'])?;
    item.starts_with([' ', '\t']).then(|| (indent, item.trim()))
}

fn is_thematic_break(text: &str) -> bool {
    let mut marks = text.chars().filter(|c| !c.is_whitespace());
    match marks.next() {
        Some(first @ ('-' | '*' | '_')) => {
            let rest = marks.clone().count();
            rest >= 2 && marks.all(|c| c == first)
        }
        _ => false,
    }
}

/// If a paragraph is nothing but a relative link to a file named `yio.md`,
/// returns the link target. A lone link to anything else, such as a README,
/// is ordinary help text.
fn embed_target(paragraph: &str) -> Option<&str> {
    let inner = paragraph.strip_prefix('[')?.strip_suffix(')')?;
    let (label, target) = inner.split_once("](")?;
    let lone = !label.contains(']') && !target.contains(['(', ')', ' ', '\n']);
    let relative = !target.contains("://") && !target.starts_with('/');
    let named = target.rsplit('/').next() == Some(FILE_NAME);
    (lone && relative && named).then_some(target)
}

/// Resolves `.` and `..` in a relative path without touching the file
/// system, so that one file reached two ways compares equal.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        let after_name = matches!(out.components().next_back(), Some(Component::Normal(_)));
        match component {
            Component::CurDir => {}
            Component::ParentDir if after_name => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// A path as written in a `yio.md`, with forward slashes on every platform.
fn show(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

/// Strips one pair of surrounding backticks.
fn unquote(text: &str) -> &str {
    let text = text.trim();
    text.strip_prefix('`')
        .and_then(|rest| rest.strip_suffix('`'))
        .unwrap_or(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = "\
# Tasks

> Project tasks

## build (TARGET) [PROFILE]

> Build a target

Longer description
on two lines.

**FLAGS**
- JOBS
  - flags: `-j --jobs`
  - default: 4
  - description: Number of parallel jobs
- RELEASE
  - flags: --release
  - type: boolean

```bash
echo \"$TARGET\"
```

### clean

> Remove build output

```
rm -rf out/
```

#### deep

## test
";

    #[test]
    fn builds_the_command_tree() {
        let root = parse(EXAMPLE).unwrap();
        assert_eq!(root.short_help.as_deref(), Some("Project tasks"));
        let names: Vec<_> = root.subcommands.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["build", "test"]);

        let build = &root.subcommands[0];
        assert_eq!(build.short_help.as_deref(), Some("Build a target"));
        assert_eq!(
            build.long_help.as_deref(),
            Some("Longer description\non two lines.")
        );
        assert_eq!(build.scripts, ["echo \"$TARGET\"\n"]);
        assert_eq!(
            build.params,
            [
                Param { name: "TARGET".into(), required: true },
                Param { name: "PROFILE".into(), required: false },
            ]
        );

        let clean = &build.subcommands[0];
        assert_eq!(clean.name, "clean");
        assert_eq!(clean.scripts, ["rm -rf out/\n"]);
        assert_eq!(clean.subcommands[0].name, "deep");
        assert!(root.subcommands[1].scripts.is_empty());
    }

    #[test]
    fn keeps_every_code_block_in_order() {
        let root = parse("## a\n```\none\n```\nBetween.\n~~~bash\ntwo\n~~~\n").unwrap();
        assert_eq!(root.subcommands[0].scripts, ["one\n", "two\n"]);
    }

    #[test]
    fn reads_flags() {
        let root = parse(EXAMPLE).unwrap();
        assert_eq!(
            root.subcommands[0].flags,
            [
                Flag {
                    name: "JOBS".into(),
                    short: Some('j'),
                    long: Some("jobs".into()),
                    ty: FlagType::String,
                    default: Some("4".into()),
                    description: Some("Number of parallel jobs".into()),
                },
                Flag {
                    name: "RELEASE".into(),
                    short: None,
                    long: Some("release".into()),
                    ty: FlagType::Boolean,
                    default: None,
                    description: None,
                },
            ]
        );
    }

    #[test]
    fn flags_list_may_follow_a_blank_line() {
        let root = parse("## a\n\n**FLAGS**\n\n- X\n  - flags: -x\n\nHelp text.\n").unwrap();
        let a = &root.subcommands[0];
        assert_eq!(a.flags[0].short, Some('x'));
        assert_eq!(a.long_help.as_deref(), Some("Help text."));
    }

    #[test]
    fn skipped_heading_levels_nest_under_the_nearest_shallower_heading() {
        let root = parse("## a\n#### b\n### c\n").unwrap();
        let a = &root.subcommands[0];
        let names: Vec<_> = a.subcommands.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["b", "c"]);
    }

    #[test]
    fn rejects_bad_input() {
        let cases = [
            ("## a\n**FLAGS**\n- X\n  - type: boolean\n", "line 3: flag `X` is missing"),
            ("## a\n**FLAGS**\n- X\n  - flags: -x\n  - kind: y\n", "line 5: unknown flag option `kind`"),
            ("## a\n**FLAGS**\n- X\n  - flags: -x\n  - type: boolean\n  - default: yes\n", "expected `true` or `false`"),
            ("## a\n**FLAGS**\n- X\n  - flags: -x\n- Y\n  - flags: -x\n", "line 5: `-x` is already used"),
            ("## a [X] (Y)\n", "cannot follow an optional one"),
            ("## a b\n", "line 1: unexpected `b`"),
            ("## a\n## a\n", "line 2: command `a` is defined twice"),
            ("## a\n```json\n{}\n```\n", "line 2: code block language `json`"),
            ("## a\n```\nnever closed\n", "line 2: code block is never closed"),
            ("```\necho\n```\n", "outside of a command"),
            ("## a\n<!--\n```\necho\n```\n", "line 2: comment is never closed"),
            ("## a (Name) (NAME)\n", "`NAME` differs from `Name` only by case"),
            ("## a (X) (X)\n", "argument `X` is defined twice"),
            ("## a (Jobs)\n**FLAGS**\n- JOBS\n  - flags: -j\n", "line 3: `JOBS` differs from `Jobs` only by case"),
            ("## a [yio_version]\n", "`yio_version` differs from `YIO_VERSION` only by case"),
            ("## a (WSLENV)\n", "`WSLENV` is reserved"),
            ("## a\n**FLAGS**\n- wslenv\n  - flags: -w\n", "line 3: `wslenv` is reserved"),
            ("## a (1X)\n", "`1X` is not a valid name"),
            ("## a\n\n[docs](docs/yio.md)\n", "yio.md: line 3: could not read `docs/yio.md`"),
        ];
        for (source, expected) in cases {
            let error = parse(source).unwrap_err();
            assert!(error.contains(expected), "{source:?} gave {error:?}");
        }
    }

    #[test]
    fn commented_out_blocks_are_not_scripts() {
        let source = "\
## a

<!--
```bash
echo hidden
```
-->

<!-- one line --> ignored
```bash
echo shown
```

## b
<!-- ## c -->
";
        let root = parse(source).unwrap();
        assert_eq!(root.subcommands[0].scripts, ["echo shown\n"]);
        assert_eq!(root.subcommands[0].long_help, None);
        let names: Vec<_> = root.subcommands.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["a", "b"]);
    }

    #[test]
    fn fence_indentation_is_removed_from_the_script() {
        let source = "## a\n  ```bash\n  cat <<EOF\n    hi\n  EOF\n ok\n   ```\n";
        let root = parse(source).unwrap();
        assert_eq!(root.subcommands[0].scripts, ["cat <<EOF\n  hi\nEOF\nok\n"]);
    }

    #[test]
    fn four_space_indented_fences_and_headings_are_literal_text() {
        let source = "## a\n\nExample:\n\n    ```bash\n    echo no\n    ```\n    ## not-a-command\n\t```\n";
        let root = parse(source).unwrap();
        let a = &root.subcommands[0];
        assert!(a.scripts.is_empty());
        assert!(a.subcommands.is_empty());
        assert!(a.long_help.as_deref().unwrap().contains("echo no"));
    }

    #[test]
    fn a_builtin_name_can_be_reused_exactly() {
        let root = parse("## a\n**FLAGS**\n- YIO_VERBOSE\n  - flags: --loud\n  - type: boolean\n").unwrap();
        assert_eq!(root.subcommands[0].flags[0].name, "YIO_VERBOSE");
    }

    #[test]
    fn only_a_lone_relative_link_to_a_yio_md_embeds() {
        let source = "\
## a

See [docs](docs/yio.md) for more.

[site](https://example.com/yio.md)

[readme](README.md)

[rooted](/etc/yio.md)
";
        let root = parse(source).unwrap();
        let help = root.subcommands[0].long_help.as_deref().unwrap();
        assert_eq!(help.split("\n\n").count(), 4);
    }

    /// Loads the first of `files`, serving the rest by path for embedding.
    fn load_files(files: &[(&str, &str)]) -> Result<Command, String> {
        let read = |path: &Path| {
            let wanted = show(path);
            let found = files.iter().find(|(name, _)| *name == wanted);
            found
                .map(|(_, source)| source.to_string())
                .ok_or_else(|| ReadError::Io(io::Error::from(io::ErrorKind::NotFound)))
        };
        load(Path::new(files[0].0), files[0].1, &read)
    }

    fn names(command: &Command) -> Vec<&str> {
        command.subcommands.iter().map(|c| c.name.as_str()).collect()
    }

    #[test]
    fn embedded_commands_nest_under_the_embedding_heading() {
        let root = load_files(&[
            ("yio.md", "## docs\n\n[docs](docs/yio.md)\n\n### local\n\n## other\n"),
            ("docs/yio.md", "# Docs\n\n> Documentation tasks\n\nMore.\n\n## build (OUT)\n```\necho\n```\n### fast\n## serve\n"),
        ])
        .unwrap();
        assert_eq!(names(&root), ["docs", "other"]);

        let docs = &root.subcommands[0];
        assert_eq!(names(docs), ["build", "serve", "local"]);
        assert_eq!(docs.short_help.as_deref(), Some("Documentation tasks"));
        assert_eq!(docs.long_help.as_deref(), Some("More."));
        assert_eq!(docs.dir, None);

        let build = &docs.subcommands[0];
        assert_eq!(build.params[0].name, "OUT");
        assert_eq!(build.scripts, ["echo\n"]);
        assert_eq!(build.dir.as_deref(), Some(Path::new("docs")));
        assert_eq!(build.subcommands[0].dir.as_deref(), Some(Path::new("docs")));
        assert_eq!(docs.subcommands[2].dir, None);
    }

    #[test]
    fn the_embedding_heading_keeps_its_own_help() {
        let root = load_files(&[
            ("yio.md", "## docs\n\n[docs](docs/yio.md)\n\n> Mine\n"),
            ("docs/yio.md", "> Theirs\n\nTheir details.\n\n## build\n"),
        ])
        .unwrap();
        let docs = &root.subcommands[0];
        assert_eq!(docs.short_help.as_deref(), Some("Mine"));
        assert_eq!(docs.long_help.as_deref(), Some("Their details."));
    }

    #[test]
    fn embedding_under_the_h1_adds_top_level_commands() {
        let root = load_files(&[
            ("yio.md", "# Tasks\n\n[tools](tools/yio.md)\n\n## build\n"),
            ("tools/yio.md", "## lint\n## fmt\n"),
        ])
        .unwrap();
        assert_eq!(names(&root), ["lint", "fmt", "build"]);
    }

    #[test]
    fn embedded_links_are_relative_to_their_own_file() {
        let root = load_files(&[
            ("yio.md", "## docs\n\n[docs](./docs/yio.md)\n"),
            ("docs/yio.md", "## tools\n\n[tools](../tools/yio.md)\n\n## api\n\n[api](api/yio.md)\n"),
            ("tools/yio.md", "## lint\n"),
            ("docs/api/yio.md", "## gen\n"),
        ])
        .unwrap();
        let docs = &root.subcommands[0];
        let lint = &docs.subcommands[0].subcommands[0];
        assert_eq!(lint.name, "lint");
        assert_eq!(lint.dir.as_deref(), Some(Path::new("tools")));
        let generate = &docs.subcommands[1].subcommands[0];
        assert_eq!(generate.name, "gen");
        assert_eq!(generate.dir.as_deref(), Some(Path::new("docs/api")));
    }

    #[test]
    fn one_file_may_be_embedded_in_two_places() {
        let root = load_files(&[
            ("yio.md", "## a\n\n[x](shared/yio.md)\n\n## b\n\n[x](shared/yio.md)\n"),
            ("shared/yio.md", "## run\n"),
        ])
        .unwrap();
        assert_eq!(names(&root.subcommands[0]), ["run"]);
        assert_eq!(names(&root.subcommands[1]), ["run"]);
    }

    #[test]
    fn rejects_bad_embeds() {
        let cases: [(&[(&str, &str)], &str); 5] = [
            (
                &[("yio.md", "## a\n\n[self](yio.md)\n")],
                "yio.md: line 3: `yio.md` is already being loaded",
            ),
            (
                &[
                    ("yio.md", "## a\n\n[docs](docs/yio.md)\n"),
                    ("docs/yio.md", "## b\n\n[back](../yio.md)\n"),
                ],
                "docs/yio.md: line 3: `yio.md` is already being loaded",
            ),
            (
                &[
                    ("yio.md", "## a\n\n[docs](docs/yio.md)\n"),
                    ("docs/yio.md", "## b\n```json\n{}\n```\n"),
                ],
                "docs/yio.md: line 2: code block language `json`",
            ),
            (
                &[("yio.md", "## a\n\n[docs](docs/yio.md)\n")],
                "yio.md: line 3: could not read `docs/yio.md`",
            ),
            (
                &[
                    ("yio.md", "## a\n\n[docs](docs/yio.md)\n\n[more](more/yio.md)\n"),
                    ("docs/yio.md", "## build\n"),
                    ("more/yio.md", "## build\n"),
                ],
                "yio.md: line 5: command `build` from `more/yio.md` is already defined here",
            ),
        ];
        for (files, expected) in cases {
            let error = load_files(files).unwrap_err();
            assert!(error.starts_with(expected), "expected {expected:?}, got {error:?}");
        }
    }

    #[test]
    fn a_heading_cannot_redefine_an_embedded_command() {
        let error = load_files(&[
            ("yio.md", "## a\n\n[docs](docs/yio.md)\n\n### build\n"),
            ("docs/yio.md", "## build\n"),
        ])
        .unwrap_err();
        assert_eq!(error, "yio.md: line 5: command `build` is defined twice");
    }
}
