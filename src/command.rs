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

    /// The command's own flag spelled `-c`. It shadows a built-in with the
    /// same spelling.
    pub fn short_flag(&self, c: char) -> Option<&Flag> {
        self.flags.iter().find(|flag| flag.short == Some(c))
    }

    /// The command's own flag spelled `--name`. It shadows a built-in with
    /// the same spelling.
    pub fn long_flag(&self, name: &str) -> Option<&Flag> {
        self.flags.iter().find(|flag| flag.long.as_deref() == Some(name))
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

/// Checks that `name` can be injected into a script's environment under
/// exactly that name on every platform.
///
/// Windows treats environment variable names as case-insensitive, so two
/// names that differ only by case would silently become one variable there.
/// Names are therefore compared ignoring case everywhere, which keeps a
/// `yio.md` behaving the same on all platforms.
pub fn check_var_name(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    let valid = matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !valid {
        return Err(format!(
            "`{name}` is not a valid name; it must work as an environment variable"
        ));
    }
    // See `run.rs`: yio sets WSLENV itself, which would discard the value.
    if name.eq_ignore_ascii_case("WSLENV") {
        return Err(format!(
            "`{name}` is reserved; yio sets WSLENV to forward variables to WSL"
        ));
    }
    // Using a built-in's exact name overrides it, which is allowed.
    for builtin in Builtin::ALL {
        let reserved = builtin.env_name();
        if name != reserved && name.eq_ignore_ascii_case(reserved) {
            return Err(same_ignoring_case(name, reserved));
        }
    }
    Ok(())
}

/// The error for a name that clashes with `existing` once case is ignored.
pub fn same_ignoring_case(name: &str, existing: &str) -> String {
    format!(
        "`{name}` differs from `{existing}` only by case; variable names must be unique ignoring case"
    )
}

pub fn is_boolean(s: &str) -> bool {
    s == "true" || s == "false"
}
