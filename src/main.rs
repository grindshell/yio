mod bytes;
mod cache;
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
use input::{Freshness, Input};
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
    // `--cache` computes everything again instead of reusing what is stored.
    let session = Session::open(flags.cache);
    let plugins = Plugins::new(flags, &session, started);

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
        run::run(&command.scripts, &invocation.env(), command.dir.as_deref())
    }
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
        return load(&|path| {
            plugins.before(dir_of(path)).map_err(ReadError::Failed)?;
            Ok(std::fs::read_to_string(path)?)
        });
    }

    if let Some(unit) = session.find(UnitKind::Tree, FILE) {
        // A plugin may rewrite the files the cached tree was built from, so
        // the plugins of every directory it covers run before it is checked.
        for input in &unit.inputs {
            if let Input::File { path, .. } = input {
                plugins.before(dir_of(Path::new(path)))?;
            }
        }
        let freshness = input::check(&unit.inputs, unit.as_of, started, &[]);
        if freshness != Freshness::Dirty
            && let Ok(root) = cache::decode_tree(&unit.body)
        {
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
    let root = load(&|path| {
        plugins.before(dir_of(path)).map_err(ReadError::Failed)?;
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

/// Loads `yio.md` and the files it embeds. `None` when there is no `yio.md`.
fn load(read: markdown::Read) -> Result<Option<Command>, String> {
    let file = Path::new(FILE);
    let source = match read(file) {
        Ok(source) => source,
        Err(ReadError::Io(error)) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(ReadError::Io(error)) => return Err(format!("could not read {FILE}: {error}")),
        Err(ReadError::Failed(message)) => return Err(message),
    };
    markdown::load(file, &source, read).map(Some)
}
