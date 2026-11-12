//! Resolves command-line arguments against a [`Command`] tree.
//!
//! The tree comes from a `yio.md` file, so nothing about the commands, their
//! arguments, or their flags is known at compile time. See
//! `docs/cli-parsing.md` for the rules implemented here.

use crate::command::{Builtin, Command, Flag, FlagType};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Which built-in flags were passed.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Builtins {
    pub help: bool,
    pub verbose: bool,
    pub version: bool,
    pub cache: bool,
    pub remove_cache: bool,
}

/// The outcome of parsing: which command was named and with what values.
#[derive(Debug)]
pub struct Invocation<'a> {
    /// Names of the commands walked from the root, not including the root.
    pub path: Vec<&'a str>,
    pub command: &'a Command,
    pub builtins: Builtins,
    /// A `(name, value)` pair for every argument and flag of `command`, in
    /// declaration order. Anything not given on the command line has its
    /// default, or `"false"` for booleans, or an empty string.
    pub vars: Vec<(&'a str, String)>,
}

impl Invocation<'_> {
    /// The environment variables to inject into the command's script. A
    /// command's own variables replace built-ins of the same name.
    pub fn env(&self) -> Vec<(&str, String)> {
        let builtins = self.builtins;
        let mut env: Vec<(&str, String)> = Builtin::ALL
            .into_iter()
            .map(|builtin| {
                let value = match builtin {
                    Builtin::Help => builtins.help.to_string(),
                    Builtin::Verbose => builtins.verbose.to_string(),
                    Builtin::Version => VERSION.to_string(),
                    Builtin::Cache => builtins.cache.to_string(),
                    Builtin::RemoveCache => builtins.remove_cache.to_string(),
                };
                (builtin.env_name(), value)
            })
            .filter(|(name, _)| !self.vars.iter().any(|(var, _)| var == name))
            .collect();
        env.extend(self.vars.iter().map(|(name, value)| (*name, value.clone())));
        env
    }
}

/// Parses `args` (without the program name) against the tree under `root`.
pub fn parse<'a>(root: &'a Command, args: &[String]) -> Result<Invocation<'a>, String> {
    let mut state = State {
        command: root,
        path: Vec::new(),
        positionals: Vec::new(),
        given: Vec::new(),
        builtins: Builtins::default(),
    };
    let mut args = args.iter().map(String::as_str);
    // Set by `--`: everything after it is a positional argument.
    let mut literal = false;

    while let Some(arg) = args.next() {
        if literal {
            state.positionals.push(arg);
        } else if arg == "--" {
            literal = true;
        } else if let Some(body) = arg.strip_prefix("--") {
            let (name, inline) = match body.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (body, None),
            };
            let label = format!("--{name}");
            let target = state.long(name).ok_or_else(|| state.unknown_flag(&label))?;
            state.apply(target, &label, inline, &mut args)?;
        } else if arg.len() > 1 && arg.starts_with('-') {
            // One or more short flags: `-a`, `-abc`, `-j4`, `-j=4`, `-j 4`.
            let mut rest = &arg[1..];
            while let Some(c) = rest.chars().next() {
                rest = &rest[c.len_utf8()..];
                let label = format!("-{c}");
                let target = state.short(c).ok_or_else(|| state.unknown_flag(&label))?;
                if let Some(value) = rest.strip_prefix('=') {
                    state.apply(target, &label, Some(value), &mut args)?;
                    break;
                }
                if target.takes_value() {
                    let inline = (!rest.is_empty()).then_some(rest);
                    state.apply(target, &label, inline, &mut args)?;
                    break;
                }
                state.apply(target, &label, None, &mut args)?;
            }
        } else if state.positionals.is_empty()
            && let Some(sub) = state.command.subcommands.iter().find(|c| c.name == arg)
        {
            state.descend(sub)?;
        } else {
            state.positionals.push(arg);
        }
    }
    state.finish()
}

/// What a flag on the command line refers to.
#[derive(Clone, Copy)]
enum Target<'a> {
    User(&'a Flag),
    Builtin(Builtin),
}

impl Target<'_> {
    fn takes_value(self) -> bool {
        matches!(self, Target::User(flag) if flag.ty == FlagType::String)
    }
}

struct State<'a, 'b> {
    /// The command resolved so far.
    command: &'a Command,
    path: Vec<&'a str>,
    positionals: Vec<&'b str>,
    /// Values given for flags of `command`.
    given: Vec<(&'a Flag, String)>,
    builtins: Builtins,
}

impl<'a, 'b> State<'a, 'b> {
    fn long(&self, name: &str) -> Option<Target<'a>> {
        let user = self.command.flags.iter();
        let user = user.filter(|f| f.long.as_deref() == Some(name)).map(Target::User);
        let builtin = Builtin::ALL.into_iter();
        let builtin = builtin.filter(|b| b.long() == name).map(Target::Builtin);
        user.chain(builtin).next()
    }

    fn short(&self, c: char) -> Option<Target<'a>> {
        let user = self.command.flags.iter();
        let user = user.filter(|f| f.short == Some(c)).map(Target::User);
        let builtin = Builtin::ALL.into_iter();
        let builtin = builtin.filter(|b| b.short() == Some(c)).map(Target::Builtin);
        user.chain(builtin).next()
    }

    fn apply(
        &mut self,
        target: Target<'a>,
        label: &str,
        inline: Option<&str>,
        args: &mut impl Iterator<Item = &'b str>,
    ) -> Result<(), String> {
        match target {
            Target::Builtin(builtin) => {
                let slot = match builtin {
                    Builtin::Help => &mut self.builtins.help,
                    Builtin::Verbose => &mut self.builtins.verbose,
                    Builtin::Version => &mut self.builtins.version,
                    Builtin::Cache => &mut self.builtins.cache,
                    Builtin::RemoveCache => &mut self.builtins.remove_cache,
                };
                *slot = boolean(label, inline)?;
            }
            Target::User(flag) => {
                let value = match (flag.ty, inline) {
                    (FlagType::Boolean, _) => boolean(label, inline)?.to_string(),
                    (FlagType::String, Some(value)) => value.to_string(),
                    (FlagType::String, None) => args
                        .next()
                        .ok_or_else(|| format!("flag `{label}` needs a value"))?
                        .to_string(),
                };
                // The last occurrence of a repeated flag wins.
                match self.given.iter_mut().find(|(f, _)| std::ptr::eq(*f, flag)) {
                    Some(slot) => slot.1 = value,
                    None => self.given.push((flag, value)),
                }
            }
        }
        Ok(())
    }

    fn descend(&mut self, sub: &'a Command) -> Result<(), String> {
        // Flags are not inherited, so one given before the subcommand name
        // would otherwise be dropped silently.
        if let Some((flag, _)) = self.given.first() {
            let label = match (&flag.long, flag.short) {
                (Some(long), _) => format!("--{long}"),
                (None, Some(short)) => format!("-{short}"),
                (None, None) => flag.name.clone(),
            };
            return Err(format!(
                "flag `{label}` belongs to `{}`, not to its subcommand `{}`",
                self.display_path(),
                sub.name
            ));
        }
        self.path.push(&sub.name);
        self.command = sub;
        Ok(())
    }

    fn finish(self) -> Result<Invocation<'a>, String> {
        if self.builtins.cache && self.builtins.remove_cache {
            return Err("`--cache` and `--remove-cache` cannot be used together".to_string());
        }
        let command = self.command;
        let path = self.display_path();

        if let Some(extra) = self.positionals.get(command.params.len()) {
            return Err(if command.params.is_empty() && !command.subcommands.is_empty() {
                format!("unknown command `{extra}` for `{path}`")
            } else {
                format!("unexpected argument `{extra}` for `{path}`")
            });
        }

        let mut vars = Vec::new();
        for (i, param) in command.params.iter().enumerate() {
            let value = match self.positionals.get(i) {
                Some(value) => value.to_string(),
                // Asking for help on a command should not require its arguments.
                None if param.required && !self.builtins.help => {
                    return Err(format!(
                        "missing required argument `({})` for `{path}`",
                        param.name
                    ));
                }
                None => String::new(),
            };
            vars.push((param.name.as_str(), value));
        }
        for flag in &command.flags {
            let given = self.given.iter().find(|(f, _)| std::ptr::eq(*f, flag));
            let value = match (given, &flag.default, flag.ty) {
                (Some((_, value)), _, _) => value.clone(),
                (None, Some(default), _) => default.clone(),
                (None, None, FlagType::Boolean) => "false".to_string(),
                (None, None, FlagType::String) => String::new(),
            };
            vars.push((flag.name.as_str(), value));
        }

        Ok(Invocation {
            path: self.path,
            command,
            builtins: self.builtins,
            vars,
        })
    }

    fn unknown_flag(&self, label: &str) -> String {
        format!("unknown flag `{label}` for `{}`", self.display_path())
    }

    fn display_path(&self) -> String {
        let mut path = String::from("yio");
        for name in &self.path {
            path.push(' ');
            path.push_str(name);
        }
        path
    }
}

/// The value of a boolean flag: bare means true, `=true` and `=false` are
/// explicit, anything else is a type error.
fn boolean(label: &str, inline: Option<&str>) -> Result<bool, String> {
    match inline {
        None | Some("true") => Ok(true),
        Some("false") => Ok(false),
        Some(other) => Err(format!(
            "flag `{label}` is a boolean; expected `true` or `false`, found `{other}`"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown;

    const SOURCE: &str = "\
## build (TARGET) [PROFILE]

**FLAGS**
- JOBS
  - flags: -j --jobs
  - default: 4
- RELEASE
  - flags: -r --release
  - type: boolean

```
echo
```

### clean

```
echo
```

## group

### leaf

**FLAGS**
- YIO_VERBOSE
  - flags: -v --loud
  - type: boolean

```
echo
```
";

    fn tree() -> Command {
        markdown::parse(SOURCE).unwrap()
    }

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(String::from).collect()
    }

    /// Parses `line` and returns the path and variables as plain strings.
    fn resolve(root: &Command, line: &str) -> (String, Vec<String>) {
        let invocation = parse(root, &args(line)).unwrap();
        let vars = invocation.vars.iter().map(|(k, v)| format!("{k}={v}"));
        (invocation.path.join(" "), vars.collect())
    }

    #[test]
    fn no_arguments_resolves_to_the_root() {
        let root = tree();
        let invocation = parse(&root, &[]).unwrap();
        assert!(invocation.path.is_empty());
        assert!(std::ptr::eq(invocation.command, &root));
        assert_eq!(invocation.builtins, Builtins::default());
    }

    #[test]
    fn binds_arguments_and_flags() {
        let root = tree();
        assert_eq!(
            resolve(&root, "build web prod -j 8 --release"),
            (
                "build".to_string(),
                vec!["TARGET=web".into(), "PROFILE=prod".into(), "JOBS=8".into(), "RELEASE=true".into()]
            )
        );
    }

    #[test]
    fn fills_in_defaults() {
        let root = tree();
        let (_, vars) = resolve(&root, "build web");
        assert_eq!(vars, ["TARGET=web", "PROFILE=", "JOBS=4", "RELEASE=false"]);
    }

    #[test]
    fn accepts_every_flag_spelling() {
        let root = tree();
        for line in [
            "build web --jobs 8 --release",
            "build web --jobs=8 --release=true",
            "build web -j 8 -r",
            "build web -j8 -r",
            "build web -j=8 -r=true",
            "build web -rj 8",
            "build web -rj8",
            "build -j 2 -r=false -r web --jobs 8",
        ] {
            let (_, vars) = resolve(&root, line);
            assert_eq!(vars[2..], ["JOBS=8", "RELEASE=true"], "{line}");
        }
    }

    #[test]
    fn subcommands_win_over_arguments_until_an_argument_is_seen() {
        let root = tree();
        assert_eq!(resolve(&root, "build clean").0, "build clean");
        assert_eq!(resolve(&root, "group leaf").0, "group leaf");

        let (path, vars) = resolve(&root, "build web clean");
        assert_eq!(path, "build");
        assert_eq!(vars[..2], ["TARGET=web", "PROFILE=clean"]);

        let (path, vars) = resolve(&root, "build -- clean");
        assert_eq!(path, "build");
        assert_eq!(vars[0], "TARGET=clean");
    }

    #[test]
    fn builtins_work_at_any_depth() {
        let root = tree();
        let invocation = parse(&root, &args("-V build clean --cache")).unwrap();
        assert_eq!(invocation.path, ["build", "clean"]);
        assert_eq!(
            invocation.builtins,
            Builtins { verbose: true, cache: true, ..Builtins::default() }
        );
        assert!(parse(&root, &args("-v")).unwrap().builtins.version);
    }

    #[test]
    fn help_does_not_require_arguments() {
        let root = tree();
        for line in ["build --help", "build -h", "-h build", "build -hV"] {
            let invocation = parse(&root, &args(line)).unwrap();
            assert!(invocation.builtins.help, "{line}");
            assert_eq!(invocation.path, ["build"], "{line}");
        }
        assert!(parse(&root, &args("--help")).unwrap().path.is_empty());
        assert!(parse(&root, &args("nope --help")).is_err());
    }

    #[test]
    fn a_command_can_override_a_builtin() {
        let root = tree();
        let invocation = parse(&root, &args("group leaf -v")).unwrap();
        assert!(!invocation.builtins.version);
        let env = invocation.env();
        let verbose: Vec<_> = env.iter().filter(|(k, _)| *k == "YIO_VERBOSE").collect();
        assert_eq!(verbose, [&("YIO_VERBOSE", "true".to_string())]);
    }

    #[test]
    fn env_carries_the_builtins() {
        let root = tree();
        let invocation = parse(&root, &args("build clean -V")).unwrap();
        assert_eq!(
            invocation.env(),
            [
                ("YIO_HELP", "false".to_string()),
                ("YIO_VERBOSE", "true".to_string()),
                ("YIO_VERSION", VERSION.to_string()),
                ("YIO_CACHE", "false".to_string()),
                ("YIO_REMOVE_CACHE", "false".to_string()),
            ]
        );
    }

    #[test]
    fn rejects_bad_command_lines() {
        let root = tree();
        let cases = [
            ("nope", "unknown command `nope` for `yio`"),
            ("group nope", "unknown command `nope` for `yio group`"),
            ("build", "missing required argument `(TARGET)` for `yio build`"),
            ("build a b c", "unexpected argument `c` for `yio build`"),
            ("build web --nope", "unknown flag `--nope` for `yio build`"),
            ("build web -x", "unknown flag `-x` for `yio build`"),
            ("build clean -j 8", "unknown flag `-j` for `yio build clean`"),
            ("build web --jobs", "flag `--jobs` needs a value"),
            ("build web --release=yes", "expected `true` or `false`, found `yes`"),
            ("build -r clean", "flag `--release` belongs to `yio build`, not to its subcommand `clean`"),
            ("--cache --remove-cache", "cannot be used together"),
        ];
        for (line, expected) in cases {
            let error = parse(&root, &args(line)).unwrap_err();
            assert!(error.contains(expected), "`{line}` gave {error:?}");
        }
    }
}
