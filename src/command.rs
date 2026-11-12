//! The command tree a `yio.md` file is interpreted into.

/// One command. The root of the tree is the file itself; its subcommands are
/// the H2 headings, theirs the H3 headings, and so on.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Command {
    pub name: String,
    /// Blockquote text.
    pub short_help: Option<String>,
    /// Paragraph text, shown with `--verbose`.
    pub long_help: Option<String>,
    pub params: Vec<Param>,
    pub flags: Vec<Flag>,
    /// Bodies of the code blocks, in order. Each runs in its own bash process.
    /// A command without any only groups subcommands.
    pub scripts: Vec<String>,
    pub subcommands: Vec<Command>,
}

impl Command {
    pub fn root() -> Self {
        Command {
            name: "yio".to_string(),
            ..Command::default()
        }
    }
}

/// A positional argument: `(NAME)` is required, `[NAME]` is optional.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub required: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum FlagType {
    #[default]
    String,
    Boolean,
}

/// A named flag from a `**FLAGS**` list.
#[derive(Debug, Clone, PartialEq)]
pub struct Flag {
    /// Environment variable the value is injected under.
    pub name: String,
    pub short: Option<char>,
    pub long: Option<String>,
    pub ty: FlagType,
    pub default: Option<String>,
    pub description: Option<String>,
}

/// Flags every command accepts. A command's own flags are looked up first, so
/// a `yio.md` can override any of these.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Builtin {
    Help,
    Verbose,
    Version,
    Cache,
    RemoveCache,
}

impl Builtin {
    pub const ALL: [Builtin; 5] = [
        Builtin::Help,
        Builtin::Verbose,
        Builtin::Version,
        Builtin::Cache,
        Builtin::RemoveCache,
    ];

    pub fn env_name(self) -> &'static str {
        match self {
            Builtin::Help => "YIO_HELP",
            Builtin::Verbose => "YIO_VERBOSE",
            Builtin::Version => "YIO_VERSION",
            Builtin::Cache => "YIO_CACHE",
            Builtin::RemoveCache => "YIO_REMOVE_CACHE",
        }
    }

    pub fn short(self) -> Option<char> {
        match self {
            Builtin::Help => Some('h'),
            Builtin::Verbose => Some('V'),
            Builtin::Version => Some('v'),
            Builtin::Cache | Builtin::RemoveCache => None,
        }
    }

    pub fn long(self) -> &'static str {
        match self {
            Builtin::Help => "help",
            Builtin::Verbose => "verbose",
            Builtin::Version => "version",
            Builtin::Cache => "cache",
            Builtin::RemoveCache => "remove-cache",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Builtin::Help => "Show help for a command",
            Builtin::Verbose => "Show verbose help text",
            Builtin::Version => "Print the yio version",
            Builtin::Cache => "Write the interpreted yio.md to .yio/cache.yio",
            Builtin::RemoveCache => "Remove .yio/cache.yio",
        }
    }
}

/// Environment variable names: a letter or underscore, then letters, digits,
/// or underscores.
pub fn is_env_name(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub fn is_boolean(s: &str) -> bool {
    s == "true" || s == "false"
}
