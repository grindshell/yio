mod bytes;
mod cache;
mod check;
mod cli;
mod command;
mod help;
mod input;
mod markdown;
mod plugin;
mod run;
mod script;
#[cfg(test)]
mod testing;

use std::io::ErrorKind;
use std::path::Path;
use std::process::ExitCode;

use cache::{Session, Unit, UnitKind};
use command::Command;
use input::{Freshness, Input, Vars};
use markdown::{FILE_NAME as FILE, ReadError};
use plugin::Plugins;

const ABOUT: &str = "Y I Oughta: a task runner for bash. Commands come from the yio.md in the current directory.";

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(message) => {
            eprintln!("yio: {message}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<ExitCode, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Taken before anything is read, so that every input recorded during
    // this run was read after it. See `input::check`.
    let started = input::now();
    // Plugins run before the commands are known, so what they are told
    // about this run comes from the command line as written.
    let flags = cli::scan(&args);
    let words = cli::words(&args);
    // `--cache` computes everything again instead of reusing what is stored.
    let session = Session::open(flags.cache);
    let plugins = Plugins::new(flags, &words, &session, started);

    let root = match tree(started, &plugins, &session) {
        Ok(root) => root,
        Err(error) => {
            // A yio.md that does not load should not block the built-ins
            // that need nothing from it.
            return match cli::parse(&Command::root(), &args) {
                Ok(invocation) if invocation.builtins.version => version(),
                Ok(invocation) if invocation.builtins.remove_cache => remove_cache(),
                _ => Err(error),
            };
        }
    };
    let found = root.is_some();
    if found {
        plugins.after()?;
    }
    // Without a yio.md there are no commands, but `--help` and `--version`
    // still describe the CLI itself.
    let root = root.unwrap_or_else(|| Command {
        short_help: Some(ABOUT.to_string()),
        ..Command::root()
    });
    let missing = || format!("no {FILE} in the current directory");

    let invocation = match cli::parse(&root, &args) {
        Ok(invocation) => invocation,
        Err(_) if !found => return Err(missing()),
        Err(error) => return Err(error),
    };
    let builtins = invocation.builtins;

    // A cache that exists is kept current. This is best effort: a directory
    // that cannot be written to should not stop the command from running.
    // A run without plugins saw only part of what the cache holds, so it
    // leaves the file alone.
    if plugins.enabled()
        && !builtins.cache
        && !builtins.remove_cache
        && let Some(contents) = session.refreshed()
    {
        let _ = cache::write(&contents);
    }

    let command = invocation.command;
    let describe = || {
        print!("{}", help::render(&invocation.path, command, builtins.verbose));
        Ok(ExitCode::SUCCESS)
    };

    if builtins.help {
        return describe();
    }
    if builtins.version {
        return version();
    }
    if !found {
        return Err(missing());
    }
    if builtins.cache {
        // Building the cache is the moment to look over every script.
        warn_all(&root);
        cache::write(&session.contents())
            .map_err(|error| format!("could not write {}: {error}", cache::FILE))?;
        println!("wrote {}", cache::FILE);
        return Ok(ExitCode::SUCCESS);
    }
    if builtins.remove_cache {
        return remove_cache();
    }

    if command.scripts.is_empty() {
        describe()
    } else {
        // Only with a cache, which is where the extra work is asked for.
        if session.active() {
            warn(command);
        }
        let list = invocation.rest.as_ref().map(|(name, values)| (*name, values.as_slice()));
        if builtins.dry_run {
            let title = std::iter::once("yio").chain(invocation.path.iter().copied());
            let title = title.collect::<Vec<_>>().join(" ");
            let file = file_of(command);
            print!("{}", run::plan(&title, &file, &command.scripts, &command.lines, &invocation.vars, list));
            return Ok(ExitCode::SUCCESS);
        }
        run::run(&command.scripts, &invocation.env(), list, command.dir.as_deref())
    }
}

/// Reports the uses of boolean variables in a command's scripts that
/// cannot work. These are warnings: the command still runs.
fn warn(command: &Command) {
    let file = file_of(command);
    for warning in check::booleans(command) {
        eprintln!("yio: warning: {file}: line {}: {}", warning.line, warning.message);
    }
}

/// The `yio.md` that defines a command, as a path from the current
/// directory.
fn file_of(command: &Command) -> String {
    let dir = command.dir.as_deref().unwrap_or(Path::new(""));
    dir.join(FILE).to_string_lossy().replace('\\', "/")
}

/// Does the same for a command and everything under it.
fn warn_all(command: &Command) {
    warn(command);
    command.subcommands.iter().for_each(warn_all);
}

fn version() -> Result<ExitCode, String> {
    println!("yio {}", cli::VERSION);
    Ok(ExitCode::SUCCESS)
}

fn remove_cache() -> Result<ExitCode, String> {
    let removed =
        cache::remove().map_err(|error| format!("could not remove {}: {error}", cache::FILE))?;
    if removed {
        println!("removed {}", cache::FILE);
    } else {
        println!("no {} to remove", cache::FILE);
    }
    Ok(ExitCode::SUCCESS)
}

/// The directory a `yio.md` is in, relative to the current directory.
fn dir_of(file: &Path) -> &Path {
    file.parent().unwrap_or(Path::new(""))
}

/// Gets the command tree for the current directory: from the cache when it
/// is current, and from the Markdown otherwise. `None` when there is no
/// `yio.md`. Either way, the `pre` plugins of each directory run before its
/// `yio.md` is relied on.
fn tree(started: u64, plugins: &Plugins, session: &Session) -> Result<Option<Command>, String> {
    if !session.active() {
        return load(&|path, mount| {
            plugins.before(dir_of(path), mount).map_err(ReadError::Failed)?;
            Ok(std::fs::read_to_string(path)?)
        });
    }

    if let Some(unit) = session.find(UnitKind::Tree, FILE)
        && let Ok(root) = cache::decode_tree(&unit.body)
        && pre_plugins(&unit.inputs, unit.as_of, started, &root, plugins)?
    {
        // A plugin may have rewritten the files the cached tree was built
        // from, so it is checked only now that they have all run.
        let freshness = input::check(&unit.inputs, unit.as_of, started, Vars::default());
        if freshness != Freshness::Dirty {
            match freshness {
                Freshness::Settled(inputs) => {
                    session.put(Unit { inputs, as_of: started, ..unit.clone() });
                }
                _ => session.keep(unit),
            }
            return Ok(Some(root));
        }
    }

    // Load from the Markdown, recording what was read.
    let recorder = input::Recorder::default();
    let root = load(&|path, mount| {
        plugins.before(dir_of(path), mount).map_err(ReadError::Failed)?;
        Ok(recorder.read(path)?)
    })?;
    if let Some(root) = &root {
        session.put(Unit {
            kind: UnitKind::Tree,
            source: FILE.to_string(),
            as_of: started,
            inputs: recorder.into_inputs(),
            body: cache::encode_tree(root),
        });
    }
    Ok(root)
}

/// Runs the `pre` plugins of every directory a cached tree covers, in the
/// order its files were read. Returns false as soon as one of those files
/// turns out to have changed, which leaves the rest to the loader.
///
/// What a directory's plugins are told depends on where its `yio.md` is
/// embedded, and that is only known from the cached tree. The tree says so
/// reliably for a file as long as every file read before it is unchanged,
/// since those are the ones that embed it. So each file is checked before
/// the plugins of the next one run.
fn pre_plugins(
    inputs: &[Input],
    as_of: u64,
    started: u64,
    root: &Command,
    plugins: &Plugins,
) -> Result<bool, String> {
    for input in inputs {
        let Input::File { path, .. } = input else {
            continue;
        };
        let dir = dir_of(Path::new(path));
        // A file that brought no commands has nothing below it to name, so
        // an empty list is what its plugins would be told either way.
        let mount = mount_of(root, dir).unwrap_or_default();
        plugins.before(dir, &mount)?;
        let one = std::slice::from_ref(input);
        if input::check(one, as_of, started, Vars::default()) == Freshness::Dirty {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The commands that the `yio.md` in `dir` is embedded under: the path to
/// the first command that came from it. `None` if no command did.
fn mount_of(root: &Command, dir: &Path) -> Option<Vec<String>> {
    if dir.as_os_str().is_empty() {
        return Some(Vec::new());
    }
    fn search<'a>(command: &'a Command, dir: &Path, path: &mut Vec<&'a str>) -> bool {
        for subcommand in &command.subcommands {
            if subcommand.dir.as_deref() == Some(dir) {
                return true;
            }
            path.push(&subcommand.name);
            if search(subcommand, dir, path) {
                return true;
            }
            path.pop();
        }
        false
    }
    let mut path = Vec::new();
    search(root, dir, &mut path).then(|| path.iter().map(|name| name.to_string()).collect())
}

/// Loads `yio.md` and the files it embeds. `None` when there is no `yio.md`.
fn load(read: markdown::Read) -> Result<Option<Command>, String> {
    let file = Path::new(FILE);
    let source = match read(file, &[]) {
        Ok(source) => source,
        Err(ReadError::Io(error)) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(ReadError::Io(error)) => return Err(format!("could not read {FILE}: {error}")),
        Err(ReadError::Failed(message)) => return Err(message),
    };
    markdown::load(file, &source, read).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `yio.md` at the top of this repository is its working example,
    /// so it has to load, and to pass the checks `yio --cache` would make.
    #[test]
    fn the_projects_own_yio_md_loads_cleanly() {
        let top = Path::new(env!("CARGO_MANIFEST_DIR"));
        let read = |path: &Path, _: &[String]| Ok(std::fs::read_to_string(top.join(path))?);
        let source = std::fs::read_to_string(top.join(FILE)).unwrap();
        let root = markdown::load(Path::new(FILE), &source, &read).unwrap();

        let names = |command: &Command| -> Vec<String> {
            command.subcommands.iter().map(|sub| sub.name.clone()).collect()
        };
        assert_eq!(names(&root), ["build", "test", "lint", "check", "run", "release", "docs"]);
        let docs = root.subcommands.last().unwrap();
        assert_eq!(names(docs), ["search", "links"]);
        // Every command there is to run has something to run and says
        // what it is for.
        fn each(command: &Command, visit: &mut dyn FnMut(&Command)) {
            visit(command);
            command.subcommands.iter().for_each(|sub| each(sub, visit));
        }
        each(&root, &mut |command| {
            assert!(command.short_help.is_some(), "`{}` has no short help", command.name);
            let runs = !command.scripts.is_empty() || !command.subcommands.is_empty();
            assert!(runs, "`{}` has nothing to run", command.name);
            assert_eq!(check::booleans(command), [], "`{}`", command.name);
        });
    }

    /// The skill in `skills/yio/` teaches the format by example, away from
    /// this repository. Every `yio.md` it shows has to be one that loads,
    /// and that uses its booleans the way the skill says to.
    #[test]
    fn the_examples_in_the_skill_load() {
        let skill = Path::new(env!("CARGO_MANIFEST_DIR")).join("skills/yio");
        // What an example embeds is not part of it, so any file will do.
        let read = |_: &Path, _: &[String]| Ok("## embedded\n\n```\ntrue\n```\n".to_string());
        for file in ["SKILL.md", "references/plugins.md"] {
            let mut examples = 0;
            let text = std::fs::read_to_string(skill.join(file)).unwrap();
            // An example of a whole file is fenced with four backticks,
            // since it has code blocks of its own.
            let mut inside: Option<String> = None;
            for line in text.lines() {
                match &mut inside {
                    None if line == "````markdown" => inside = Some(String::new()),
                    None => {}
                    Some(source) if line == "````" => {
                        let root = markdown::load(Path::new(FILE), source, &read)
                            .unwrap_or_else(|error| panic!("{file}: {error}\n{source}"));
                        fn each(command: &Command, visit: &mut dyn FnMut(&Command)) {
                            visit(command);
                            command.subcommands.iter().for_each(|sub| each(sub, visit));
                        }
                        each(&root, &mut |command| {
                            assert_eq!(check::booleans(command), [], "{file}: `{}`", command.name);
                        });
                        examples += 1;
                        inside = None;
                    }
                    Some(source) => {
                        source.push_str(line);
                        source.push('\n');
                    }
                }
            }
            assert!(inside.is_none(), "{file} ends inside an example");
            assert!(examples > 0, "{file} has no example to check");
        }
    }

    #[test]
    fn a_tree_says_where_each_directory_is_embedded() {
        let files = [
            ("yio.md", "[top](top/yio.md)\n\n## docs\n\n[docs](docs/yio.md)\n\n### part\n\n[one](one/yio.md#only)\n\n## empty\n\n[none](none/yio.md)\n"),
            ("top/yio.md", "## t\n"),
            ("docs/yio.md", "## api\n\n[api](api/yio.md)\n\n## guide\n"),
            ("docs/api/yio.md", "## gen\n"),
            ("one/yio.md", "## skipped\n\n## only\n"),
            ("none/yio.md", "> Only help\n"),
        ];
        let read = |path: &Path, _: &[String]| {
            let wanted = path.to_string_lossy().replace('\\', "/");
            let found = files.iter().find(|(name, _)| *name == wanted);
            Ok(found.expect("the file is listed").1.to_string())
        };
        let root = markdown::load(Path::new("yio.md"), files[0].1, &read).unwrap();
        let mount = |dir: &str| mount_of(&root, Path::new(dir)).map(|names| names.join(" "));

        assert_eq!(mount(""), Some(String::new()));
        assert_eq!(mount("top"), Some(String::new()));
        assert_eq!(mount("docs"), Some("docs".to_string()));
        assert_eq!(mount("docs/api"), Some("docs api".to_string()));
        assert_eq!(mount("one"), Some("docs part".to_string()));
        // A file that brought no commands, and a directory that is not there.
        assert_eq!(mount("none"), None);
        assert_eq!(mount("elsewhere"), None);
    }
}