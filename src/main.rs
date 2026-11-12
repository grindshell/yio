mod cache;
mod cli;
mod command;
mod help;
mod markdown;
mod run;
// Outside tests, nothing calls the reader until scripts are evaluated
// (phase 3 of docs/scripting-language.md). Remove this allowance then.
#[allow(dead_code)]
mod script;

use std::io::ErrorKind;
use std::path::Path;
use std::process::ExitCode;

use cache::Freshness;
use command::Command;
use markdown::FILE_NAME as FILE;

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
    // this run was read after it. See `cache::check`.
    let started = cache::now();

    let Tree { root, refresh } = match tree(started) {
        Ok(tree) => tree,
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
    if let Some(contents) = refresh
        && !builtins.cache
        && !builtins.remove_cache
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
        // Always from the Markdown, whatever the existing cache says.
        let (_, contents) = load_recorded(started)?;
        let contents = contents.ok_or_else(missing)?;
        cache::write(&contents)
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

/// The command tree for the current directory.
struct Tree {
    /// `None` when there is no `yio.md`.
    root: Option<Command>,
    /// New contents for the cache file, when it exists and is out of date.
    refresh: Option<Vec<u8>>,
}

/// Gets the command tree from the cache when it is current, and from the
/// Markdown otherwise.
fn tree(started: u64) -> Result<Tree, String> {
    // The file existing is what turns caching on. Unreadable or damaged
    // counts as out of date.
    let Ok(file) = std::fs::read(cache::FILE) else {
        let root = load(&|path| std::fs::read_to_string(path))?;
        return Ok(Tree { root, refresh: None });
    };

    if let Ok(stored) = cache::parse(&file) {
        let freshness = cache::check(&stored.inputs, stored.as_of, started);
        if freshness != Freshness::Dirty
            && let Ok(root) = cache::decode_tree(stored.body)
        {
            let refresh = match freshness {
                Freshness::Settled(inputs) => {
                    Some(cache::serialize(started, &inputs, stored.body))
                }
                _ => None,
            };
            return Ok(Tree { root: Some(root), refresh });
        }
    }

    let (root, refresh) = load_recorded(started)?;
    Ok(Tree { root, refresh })
}

/// Loads `yio.md` and the files it embeds. `None` when there is no `yio.md`.
fn load(read: markdown::Read) -> Result<Option<Command>, String> {
    let file = Path::new(FILE);
    let source = match read(file) {
        Ok(source) => source,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("could not read {FILE}: {error}")),
    };
    markdown::load(file, &source, read).map(Some)
}

/// Loads the tree from the Markdown while recording what was read, and
/// builds the cache contents that describe it.
fn load_recorded(as_of: u64) -> Result<(Option<Command>, Option<Vec<u8>>), String> {
    let recorder = cache::Recorder::default();
    let root = load(&|path| recorder.read(path))?;
    let contents = root.as_ref().map(|root| {
        let inputs = recorder.into_inputs();
        cache::serialize(as_of, &inputs, &cache::encode_tree(root))
    });
    Ok((root, contents))
}
