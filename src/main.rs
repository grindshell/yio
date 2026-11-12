mod cli;
mod command;
mod help;
mod markdown;
mod run;

use std::io::ErrorKind;
use std::path::Path;
use std::process::ExitCode;

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

    let source = match std::fs::read_to_string(FILE) {
        Ok(source) => Some(source),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => return Err(format!("could not read {FILE}: {error}")),
    };
    // Without a yio.md there are no commands, but `--help` and `--version`
    // still describe the CLI itself.
    let root = match &source {
        Some(source) => markdown::load(Path::new(FILE), source, &|path| {
            std::fs::read_to_string(path)
        })?,
        None => Command {
            short_help: Some(ABOUT.to_string()),
            ..Command::root()
        },
    };
    let missing = || format!("no {FILE} in the current directory");

    let invocation = match cli::parse(&root, &args) {
        Ok(invocation) => invocation,
        Err(_) if source.is_none() => return Err(missing()),
        Err(error) => return Err(error),
    };

    let command = invocation.command;
    let describe = || {
        let verbose = invocation.builtins.verbose;
        print!("{}", help::render(&invocation.path, command, verbose));
        Ok(ExitCode::SUCCESS)
    };

    if invocation.builtins.help {
        return describe();
    }
    if invocation.builtins.version {
        println!("yio {}", cli::VERSION);
        return Ok(ExitCode::SUCCESS);
    }
    if source.is_none() {
        return Err(missing());
    }
    // TODO: .yio/cache.yio, pre/ and post/ plugins (see AGENTS.md).
    if invocation.builtins.cache || invocation.builtins.remove_cache {
        return Err("`--cache` and `--remove-cache` are not implemented yet".to_string());
    }

    if command.scripts.is_empty() {
        describe()
    } else {
        run::run(&command.scripts, &invocation.env(), command.dir.as_deref())
    }
}
