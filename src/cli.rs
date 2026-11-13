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
    pub dry_run: bool,
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
    /// The name and values of the command's `[...NAME]` argument, when it
    /// has one. The values are every argument left over after the others,
    /// which may be none.
    pub rest: Option<(&'a str, Vec<String>)>,
}

impl Invocation<'_> {
    /// The environment variables to inject into the command's script. A
    /// command's own variables replace built-ins of the same name.
    pub fn env(&self) -> Vec<(&str, String)> {
        let mut env: Vec<(&str, String)> = self.builtins.env();
        env.retain(|(name, _)| !self.vars.iter().any(|(var, _)| var == name));
        env.extend(self.vars.iter().map(|(name, value)| (*name, value.clone())));
        env
    }
}

impl Builtins {
    /// The `YIO_*` variables these flags stand for.
    pub fn env(self) -> Vec<(&'static str, String)> {
        let value = |builtin| match builtin {
            Builtin::Help => self.help.to_string(),
            Builtin::Verbose => self.verbose.to_string(),
            Builtin::Version => VERSION.to_string(),
            Builtin::Cache => self.cache.to_string(),
            Builtin::RemoveCache => self.remove_cache.to_string(),
            Builtin::DryRun => self.dry_run.to_string(),
        };
        Builtin::ALL.into_iter().map(|builtin| (builtin.env_name(), value(builtin))).collect()
    }

    fn set(&mut self, builtin: Builtin, value: bool) {
        *match builtin {
            Builtin::Help => &mut self.help,
            Builtin::Verbose => &mut self.verbose,
            Builtin::Version => &mut self.version,
            Builtin::Cache => &mut self.cache,
            Builtin::RemoveCache => &mut self.remove_cache,
            Builtin::DryRun => &mut self.dry_run,
        } = value;
    }
}

/// Finds the built-in flags on a command line by their spelling alone,
/// without knowing the commands.
///
/// Plugins run before `yio.md` is loaded, so what they are told about the
/// run has to come from here. The proper parse happens later and can differ
/// in one case: a command that defines a flag with a built-in's spelling.
/// Then this still reports the built-in.
pub fn scan(args: &[String]) -> Builtins {
    let mut found = Builtins::default();
    for arg in args.iter().take_while(|arg| *arg != "--") {
        for (builtin, value) in spelled(arg) {
            found.set(builtin, value);
        }
    }
    found
}

/// The built-in flags that `arg` sets, going by its spelling alone. Empty
/// when it is anything else.
fn spelled(arg: &str) -> Vec<(Builtin, bool)> {
    if let Some(body) = arg.strip_prefix("--") {
        let (name, value) = body.split_once('=').unwrap_or((body, "true"));
        let builtin = Builtin::ALL.into_iter().find(|b| b.long() == name);
        return builtin.map(|builtin| (builtin, value != "false")).into_iter().collect();
    }
    let Some(cluster) = arg.strip_prefix('-') else {
        return Vec::new();
    };
    let builtin = |c| Builtin::ALL.into_iter().find(|b| b.short() == Some(c));
    if cluster.chars().all(|c| builtin(c).is_some()) {
        cluster.chars().filter_map(builtin).map(|builtin| (builtin, true)).collect()
    } else {
        Vec::new()
    }
}

/// The command line as plugins are told it: every argument except the
/// built-in flags, which they are told as `YIO_*` variables instead.
///
/// Like `scan`, this goes by spelling, and nothing after `--` is a flag.
pub fn words(args: &[String]) -> Vec<String> {
    let mut literal = false;
    let keep = |arg: &&String| {
        literal |= *arg == "--";
        literal || spelled(arg).is_empty()
    };
    args.iter().filter(keep).cloned().collect()
}

/// The part of `words` that is for a `yio.md` embedded under the commands
/// in `mount`: what follows those command names. Empty when the command
/// line leads somewhere else. For the `yio.md` that `yio` was started on,
/// `mount` is empty and this is every word.
pub fn below<'a>(words: &'a [String], mount: &[String]) -> &'a [String] {
    words.strip_prefix(mount).unwrap_or(&[])
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
        let user = self.command.long_flag(name).map(Target::User);
        let mut builtins = Builtin::ALL.into_iter();
        user.or_else(|| builtins.find(|b| b.long() == name).map(Target::Builtin))
    }

    fn short(&self, c: char) -> Option<Target<'a>> {
        let user = self.command.short_flag(c).map(Target::User);
        let mut builtins = Builtin::ALL.into_iter();
        user.or_else(|| builtins.find(|b| b.short() == Some(c)).map(Target::Builtin))
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
                self.builtins.set(builtin, boolean(label, inline)?);
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
            let label = flag.label();
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
        // These four do something else in place of running the command, so
        // they should not need what running it requires.
        let builtins = self.builtins;
        let runs = !(builtins.help || builtins.version || builtins.cache || builtins.remove_cache);

        // Whatever the other arguments leave over belongs to `[...NAME]`.
        let rest = command.rest.as_deref().map(|name| {
            let values = self.positionals.iter().skip(command.params.len());
            (name, values.map(|value| value.to_string()).collect())
        });
        if rest.is_none()
            && let Some(extra) = self.positionals.get(command.params.len())
        {
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
                None if param.required && runs => {
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
            // As with a required argument, not when the command will not run.
            if given.is_none() && flag.required && runs {
                return Err(format!("missing required flag `{}` for `{path}`", flag.label()));
            }
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
            rest,
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

## search [...TERMS]

**FLAGS**
- EXACT
  - flags: -e --exact
  - type: boolean

```
echo
```

## tag (FILE) [...TAGS]

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

    /// Parses `line` and returns the values of the `[...NAME]` argument.
    fn rest(root: &Command, line: &str) -> (String, Vec<String>) {
        let invocation = parse(root, &args(line)).unwrap();
        let (name, values) = invocation.rest.expect("the command takes `[...NAME]`");
        (name.to_string(), values)
    }

    #[test]
    fn the_rest_argument_takes_what_the_others_leave() {
        let root = tree();
        let terms = |line: &str| rest(&root, line).1;
        assert_eq!(rest(&root, "search built-in functions"), ("TERMS".to_string(), vec!["built-in".to_string(), "functions".to_string()]));
        assert_eq!(terms("search one"), ["one"]);
        // None at all is fine, and still a list.
        assert_eq!(terms("search"), [""; 0]);

        // The arguments before it are filled first and are not part of it.
        let invocation = parse(&root, &args("tag notes.md draft lore")).unwrap();
        assert_eq!(invocation.vars, [("FILE", "notes.md".to_string())]);
        assert_eq!(invocation.rest, Some(("TAGS", vec!["draft".to_string(), "lore".to_string()])));
        assert_eq!(rest(&root, "tag notes.md").1, [""; 0]);
        let error = parse(&root, &args("tag")).unwrap_err();
        assert_eq!(error, "missing required argument `(FILE)` for `yio tag`");

        // Flags are still flags wherever they appear, until `--`.
        assert_eq!(terms("search a -e b"), ["a", "b"]);
        assert_eq!(resolve(&root, "search a -e b").1, ["EXACT=true"]);
        assert_eq!(terms("search a -- -e --help b"), ["a", "-e", "--help", "b"]);
        assert_eq!(resolve(&root, "search a -- -e").1, ["EXACT=false"]);
        let error = parse(&root, &args("search a -x")).unwrap_err();
        assert_eq!(error, "unknown flag `-x` for `yio search`");

        // A command without one has no list.
        assert_eq!(parse(&root, &args("build web")).unwrap().rest, None);
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
    fn version_does_not_require_arguments() {
        let root = tree();
        for line in ["build --version", "build -v", "-v build"] {
            assert!(parse(&root, &args(line)).unwrap().builtins.version, "{line}");
        }
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
                ("YIO_DRY_RUN", "false".to_string()),
            ]
        );
    }

    #[test]
    fn a_dry_run_is_asked_for_like_any_built_in() {
        let root = tree();
        let invocation = parse(&root, &args("build web --dry-run -j 8")).unwrap();
        assert!(invocation.builtins.dry_run);
        // The command is resolved as it would be for a real run.
        assert_eq!(invocation.path, ["build"]);
        assert!(invocation.vars.contains(&("JOBS", "8".to_string())));
        assert!(parse(&root, &args("build --dry-run")).unwrap_err().contains("missing required argument"));
        assert_eq!(scan(&args("build web --dry-run")), Builtins { dry_run: true, ..Builtins::default() });
        assert_eq!(words(&args("build web --dry-run")), ["build", "web"]);
    }

    #[test]
    fn a_required_flag_has_to_be_given() {
        let source = "\
## deploy (TARGET)

**FLAGS**
- YES
  - flags: -y --yes
  - type: boolean
  - required: true
- TAG
  - flags: -t
  - required: true
- NOTE
  - flags: --note

```
echo
```
";
        let root = markdown::parse(source).unwrap();
        let run = |line: &str| parse(&root, &args(line)).map(|invocation| invocation.vars);
        assert_eq!(
            run("deploy prod --yes -t v1").unwrap(),
            [
                ("TARGET", "prod".to_string()),
                ("YES", "true".to_string()),
                ("TAG", "v1".to_string()),
                ("NOTE", String::new()),
            ]
        );
        // Each is named by its long form, or its short one when that is all it has.
        assert_eq!(run("deploy prod -t v1").unwrap_err(), "missing required flag `--yes` for `yio deploy`");
        assert_eq!(run("deploy prod -y").unwrap_err(), "missing required flag `-t` for `yio deploy`");
        // Given is given, whatever the value.
        assert!(run("deploy prod --yes=false -t v1").unwrap().contains(&("YES", "false".to_string())));
        assert!(run("deploy prod -y -t=").unwrap().contains(&("TAG", String::new())));
        // Describing the command does not need them, as with its arguments.
        assert!(run("deploy --help").is_ok());
        assert!(run("deploy --version").is_ok());
        // Nor does writing or removing the cache, wherever it is asked for.
        assert!(run("deploy --cache").is_ok());
        assert!(run("deploy --remove-cache").is_ok());
        // A dry run resolves the command as a real run would.
        assert_eq!(run("deploy --dry-run").unwrap_err(), "missing required argument `(TARGET)` for `yio deploy`");
    }

    #[test]
    fn scanning_finds_builtins_without_knowing_the_commands() {
        let found = |line: &str| scan(&args(line));
        assert_eq!(found(""), Builtins::default());
        assert_eq!(found("build web -j 8"), Builtins::default());
        assert_eq!(found("--cache"), Builtins { cache: true, ..Builtins::default() });
        assert_eq!(found("docs build -V --help"), Builtins { help: true, verbose: true, ..Builtins::default() });
        assert_eq!(found("-hV"), Builtins { help: true, verbose: true, ..Builtins::default() });
        assert_eq!(found("--verbose=false -v"), Builtins { version: true, ..Builtins::default() });
        assert_eq!(found("--remove-cache"), Builtins { remove_cache: true, ..Builtins::default() });
        // A cluster with anything else in it is some command's own flags.
        assert_eq!(found("-jV -x"), Builtins::default());
        // Nothing after `--` is a flag.
        assert_eq!(found("build -- --cache -V"), Builtins::default());
    }

    #[test]
    fn plugins_are_told_the_words_but_not_the_builtin_flags() {
        let told = |line: &str| words(&args(line)).join(" ");
        assert_eq!(told(""), "");
        assert_eq!(told("search built-in functions"), "search built-in functions");
        assert_eq!(told("-V search --help dir --cache=false"), "search dir");
        assert_eq!(told("-hV build web -j 8 --release"), "build web -j 8 --release");
        // A cluster with anything else in it is some command's own flags.
        assert_eq!(told("build -jV 8"), "build -jV 8");
        // Nothing after `--` is a flag, and the `--` is kept.
        assert_eq!(told("search -V -- --help -v"), "search -- --help -v");
        assert_eq!(told("search -"), "search -");
    }

    #[test]
    fn an_embedded_file_gets_the_words_after_where_it_sits() {
        let words = args("docs search built-in functions");
        let mount = |path: &str| args(path);
        assert_eq!(below(&words, &mount("")), words);
        assert_eq!(below(&words, &mount("docs")), &words[1..]);
        assert_eq!(below(&words, &mount("docs search")), &words[2..]);
        // The command line leads somewhere else.
        assert_eq!(below(&words, &mount("tools")), [""; 0]);
        assert_eq!(below(&words, &mount("docs api")), [""; 0]);
        assert_eq!(below(&args("docs"), &mount("docs")), [""; 0]);
        assert_eq!(below(&args("-- docs search"), &mount("docs")), [""; 0]);
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
