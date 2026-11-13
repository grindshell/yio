//! Runs bash.

use std::borrow::Cow;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command as Process, ExitCode};

/// Something for bash to run.
#[derive(Clone, Copy)]
pub enum Script<'a> {
    /// The text of a script.
    Text(&'a str),
    /// A script file, relative to the directory it runs in, and the
    /// arguments it is given as `$1`, `$2`, and so on.
    File(&'a str, &'a [String]),
}

/// Runs each script in its own bash process, in order, adding `env` to the
/// inherited environment. Nothing a script does to its shell carries over to
/// the next one. Stops at the first script that fails and returns its exit
/// code.
///
/// The scripts run in `dir` when it is given, and in the current directory
/// otherwise.
///
/// `list` is the name and values of a `[...NAME]` argument. Each script
/// sees it as a bash array of that name.
pub fn run(
    scripts: &[String],
    env: &[(&str, String)],
    list: Option<(&str, &[String])>,
    dir: Option<&Path>,
) -> Result<ExitCode, String> {
    let array = list.map(|(name, values)| array(name, values));
    // The last script that gives bash anything to do.
    let last = scripts.iter().rposition(|script| !is_idle(script));
    for (i, script) in scripts.iter().enumerate() {
        if is_idle(script) {
            continue;
        }
        // The assignment goes on the script's first line, so that the line
        // numbers in bash's error messages still match the code block.
        let script = match &array {
            Some(array) => Cow::Owned(format!("{array}; {script}")),
            None => Cow::Borrowed(script.as_str()),
        };
        let script = Script::Text(&script);
        let code = if Some(i) == last { run_last(script, env, dir)? } else { run_one(script, env, dir)? };
        if code != 0 {
            return Ok(ExitCode::from(code));
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// What `run` would do, written out instead of done, for `--dry-run`: the
/// variables the scripts would be given, then each script with where it is
/// from. `title` is the command, such as `yio build`, and `file` the
/// `yio.md` that defines it. `lines` has the line each script starts on.
///
/// What comes out is itself bash, so a script can be read with the values
/// it would see, or pasted into a shell to step through it.
pub fn plan(
    title: &str,
    file: &str,
    scripts: &[String],
    lines: &[usize],
    vars: &[(&str, String)],
    list: Option<(&str, &[String])>,
) -> String {
    let mut out = format!("# Dry run of `{title}`: nothing below has been run.\n");
    for (name, value) in vars {
        out.push_str(&format!("{name}={}\n", quote(value)));
    }
    if let Some((name, values)) = list {
        out.push_str(&array(name, values));
        out.push('\n');
    }
    for (script, line) in scripts.iter().zip(lines) {
        let idle = if is_idle(script) { ": only comments, so bash is not started" } else { "" };
        out.push_str(&format!("\n# {file}, line {line}{idle}\n"));
        out.push_str(script.trim_end_matches('\n'));
        out.push('\n');
    }
    out
}

/// Whether a script gives bash nothing to do: every line is blank or a
/// comment. Bash would start, do nothing, and exit with zero, so it is not
/// started.
///
/// This is the code block of a command that a plugin answers. The command
/// needs a block to be runnable, the plugin has already shown the result,
/// and starting bash would be most of what the command costs.
pub fn is_idle(script: &str) -> bool {
    // Bash takes only spaces and tabs as blanks before a `#`.
    script.lines().all(|line| {
        let line = line.trim_start_matches([' ', '\t']);
        line.is_empty() || line.starts_with('#')
    })
}

/// The bash statement that makes `name` an array holding `values`.
///
/// A list cannot travel as an environment variable, which holds one string,
/// so it is written into the script instead. Assigning it also replaces any
/// variable of that name inherited from the calling shell.
pub fn array(name: &str, values: &[String]) -> String {
    let words: Vec<String> = values.iter().map(|value| quote(value)).collect();
    format!("{name}=({})", words.join(" "))
}

/// `text` as a single-quoted POSIX shell word, which the shell takes
/// literally whatever it contains.
pub fn quote(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('\'');
    for c in text.chars() {
        match c {
            '\'' => quoted.push_str("'\\''"),
            c => quoted.push(c),
        }
    }
    quoted.push('\'');
    quoted
}

/// The bash process that would run `script`, ready to start, and the bash
/// it is.
fn command(script: Script, env: &[(&str, String)], dir: Option<&Path>) -> (Process, PathBuf) {
    let bash = bash();
    let mut process = Process::new(&bash);
    pass_script(&mut process, &bash, script);
    if let Some(dir) = dir {
        process.current_dir(dir);
    }
    process.envs(env.iter().map(|(name, value)| (*name, value.as_str())));
    #[cfg(windows)]
    wsl::forward(&mut process, env);

    // Anything yio has printed so far must reach the terminal before the
    // script's own output does.
    let _ = std::io::stdout().flush();
    (process, bash)
}

/// The error for a bash that did not start.
///
/// The usual reason is that there is none, which the system reports as a
/// file not being found without saying which. It says the same when the
/// directory to run in is missing, so that is ruled out first.
fn not_started(bash: &Path, dir: Option<&Path>, error: std::io::Error) -> String {
    let nowhere = dir.is_some_and(|dir| !dir.is_dir());
    if error.kind() != std::io::ErrorKind::NotFound || nowhere {
        return format!("could not run `{}`: {error}", bash.display());
    }
    let hint = if cfg!(windows) { "; Git for Windows and WSL each come with one" } else { "" };
    format!("there is no `bash` on PATH, and yio runs every script with it{hint}")
}

/// Runs one script and gives its exit code, which is zero on success.
pub fn run_one(script: Script, env: &[(&str, String)], dir: Option<&Path>) -> Result<u8, String> {
    let (mut process, bash) = command(script, env, dir);
    let status = process.status().map_err(|error| not_started(&bash, dir, error))?;
    if status.success() {
        return Ok(0);
    }
    // A script that a signal ended is reported the way a shell reports it,
    // as 128 plus the signal: 130 for the interrupt of Ctrl-C.
    #[cfg(unix)]
    if let Some(signal) = std::os::unix::process::ExitStatusExt::signal(&status) {
        return Ok(u8::try_from(signal.saturating_add(128)).unwrap_or(1));
    }
    let code = status.code().and_then(|code| u8::try_from(code).ok());
    Ok(code.filter(|code| *code != 0).unwrap_or(1))
}

/// Runs the script that nothing comes after, by letting it take over this
/// process. It only comes back when bash could not be started.
///
/// With `yio` out of the way, the script is the program the calling shell
/// is waiting for. Its exit code, or the signal that ended it, reaches that
/// shell untranslated. And Ctrl-C, which goes to everything the terminal is
/// running, no longer ends `yio` and returns the prompt while the script is
/// still cleaning up. There is one process fewer besides.
///
/// Not in the unit tests, where the process is the test run.
#[cfg(all(unix, not(test)))]
fn run_last(script: Script, env: &[(&str, String)], dir: Option<&Path>) -> Result<u8, String> {
    use std::os::unix::process::CommandExt;
    let (mut process, bash) = command(script, env, dir);
    Err(not_started(&bash, dir, process.exec()))
}

/// Where a process cannot be replaced, the last script runs like the rest.
#[cfg(not(all(unix, not(test))))]
fn run_last(script: Script, env: &[(&str, String)], dir: Option<&Path>) -> Result<u8, String> {
    run_one(script, env, dir)
}

/// Adds the arguments that make `bash` run `script`.
fn pass_script(process: &mut Process, bash: &Path, script: Script) {
    #[cfg(windows)]
    if wsl::is_launcher(bash) {
        wsl::pass_script(process, script);
        return;
    }
    let _ = bash;
    match script {
        Script::Text(text) => {
            process.arg("-c");
            #[cfg(not(windows))]
            process.arg(text);
            // Quoted for the same reason as a plugin's arguments below. A
            // script with no space in it would otherwise go unquoted, and
            // be cut off at its first line break.
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                process.raw_arg(wsl::always_quoted(text));
            }
        }
        Script::File(file, args) => {
            process.arg(file);
            #[cfg(not(windows))]
            process.args(args);
            #[cfg(windows)]
            for arg in args {
                use std::os::windows::process::CommandExt;
                process.raw_arg(wsl::always_quoted(arg));
            }
        }
    }
}

/// The bash to run: the first one on `PATH`.
///
/// On Windows that lookup is done here rather than left to the standard
/// library, which searches the system directories before `PATH` and so picks
/// the WSL launcher in System32 even when another bash comes first.
fn bash() -> PathBuf {
    #[cfg(windows)]
    {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let found = std::env::split_paths(&path)
            .map(|dir| dir.join("bash.exe"))
            .find(|candidate| candidate.is_file());
        if let Some(found) = found {
            return found;
        }
    }
    PathBuf::from("bash")
}

/// Workarounds for `C:\Windows\System32\bash.exe`, which is not a bash but a
/// launcher that starts one inside WSL.
#[cfg(windows)]
mod wsl {
    use std::os::windows::process::CommandExt;
    use std::path::Path;
    use std::process::Command as Process;

    use super::{Script, quote};

    /// The launcher is the only `bash.exe` that lives in the system
    /// directory.
    pub fn is_launcher(bash: &Path) -> bool {
        let Some(root) = std::env::var_os("SystemRoot") else {
            return false;
        };
        let system = Path::new(&root).join("System32");
        bash.parent()
            .is_some_and(|dir| dir.as_os_str().eq_ignore_ascii_case(system.as_os_str()))
    }

    /// The launcher does not hand its arguments to bash as they are. It
    /// passes the rest of its command line through a shell inside WSL, so a
    /// script given the usual way is expanded once there and again by the
    /// bash that runs it: `'$HOME'` arrives already expanded and `awk`'s
    /// `$1` is gone. Writing each argument as one single-quoted shell word
    /// makes that first pass leave it alone.
    pub fn pass_script(process: &mut Process, script: Script) {
        match script {
            Script::Text(text) => process.arg("-c").raw_arg(quote(text)),
            Script::File(file, args) => {
                process.raw_arg(quote(file));
                args.iter().fold(process, |process, arg| process.raw_arg(quote(arg)))
            }
        };
    }

    /// The launcher only forwards the variables named in `WSLENV`. The loader
    /// reserves that name, so this never replaces a value from a `yio.md`.
    pub fn forward(process: &mut Process, env: &[(&str, String)]) {
        let mut names = std::env::var("WSLENV").unwrap_or_default();
        for (name, _) in env {
            if !names.is_empty() {
                names.push(':');
            }
            names.push_str(name);
        }
        process.env("WSLENV", names);
    }

    /// `arg` in double quotes, as it goes onto a Windows command line.
    ///
    /// This is for every other bash on Windows, such as Git Bash. Those are
    /// built on MSYS or Cygwin, which read the command line again when the
    /// program that started them is a Windows one: an argument that is not
    /// quoted has `*` expanded against the files in the directory, `'`
    /// taken as the start of a quotation, `@name` replaced with the
    /// contents of a file, and a line break taken as the end of the
    /// argument. The standard library quotes an argument only when it has
    /// a space or a tab in it. So a script, and the arguments a plugin is
    /// told, which can be anything, are always quoted here.
    pub fn always_quoted(arg: &str) -> String {
        let mut quoted = String::from('"');
        let mut backslashes = 0;
        for c in arg.chars() {
            match c {
                '\\' => backslashes += 1,
                // A quote needs a backslash, and each backslash before it
                // one more, or they would be read as escaping the quote.
                '"' => {
                    quoted.extend(std::iter::repeat_n('\\', backslashes + 1));
                    backslashes = 0;
                }
                _ => backslashes = 0,
            }
            quoted.push(c);
        }
        // The same goes for backslashes before the closing quote.
        quoted.extend(std::iter::repeat_n('\\', backslashes));
        quoted.push('"');
        quoted
    }

    #[cfg(test)]
    mod tests {
        use super::always_quoted;

        #[test]
        fn quotes_an_argument_for_a_windows_command_line() {
            assert_eq!(always_quoted("plain"), r#""plain""#);
            assert_eq!(always_quoted(""), r#""""#);
            assert_eq!(always_quoted("it's *"), r#""it's *""#);
            assert_eq!(always_quoted(r#"a"b"#), r#""a\"b""#);
            assert_eq!(always_quoted(r"dir\file"), r#""dir\file""#);
            assert_eq!(always_quoted(r#"a\"b"#), r#""a\\\"b""#);
            assert_eq!(always_quoted(r"ends\"), r#""ends\\""#);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::process::ExitCode;

    use super::{array, is_idle, not_started, plan, quote, run};

    #[test]
    fn a_missing_bash_is_said_in_so_many_words() {
        use std::io::{Error, ErrorKind};
        use std::path::Path;

        let bash = Path::new("bash");
        let missing = not_started(bash, None, Error::from(ErrorKind::NotFound));
        assert!(missing.starts_with("there is no `bash` on PATH, and yio runs every script with it"), "{missing}");
        // Where to get one is only a question on Windows.
        assert_eq!(missing.contains("Git for Windows"), cfg!(windows));

        // Anything else is reported as the system put it.
        let denied = not_started(bash, None, Error::from(ErrorKind::PermissionDenied));
        assert!(denied.starts_with("could not run `bash`: "), "{denied}");
        // So is a directory that is not there, which looks the same to the
        // system as a bash that is not.
        let nowhere = not_started(bash, Some(Path::new("no/such/directory")), Error::from(ErrorKind::NotFound));
        assert!(nowhere.starts_with("could not run `bash`: "), "{nowhere}");
    }

    #[test]
    fn a_script_of_only_comments_gives_bash_nothing_to_do() {
        for idle in ["", "\n\n", "# Answered by a plugin.", "  # one\n\t# two\n\n#!/bin/sh"] {
            assert!(is_idle(idle), "{idle:?}");
        }
        for busy in ["true", "# note\necho hi", "echo hi # note", ": # no-op", "\u{a0}# not a blank to bash"] {
            assert!(!is_idle(busy), "{busy:?}");
        }
        // Nothing is started for one, so this passes where there is no bash.
        let idle = ["# Answered by a plugin.\n".to_string()];
        assert_eq!(run(&idle, &[], None, None), Ok(ExitCode::SUCCESS));
    }

    #[test]
    fn a_plan_shows_the_values_and_each_script() {
        let scripts = ["set -e\necho \"$TARGET\"\n".to_string(), "# Answered by a plugin.\n".to_string()];
        let vars = [("TARGET", "it's web".to_string()), ("RELEASE", "false".to_string())];
        let names = ["a".to_string(), "b c".to_string()];
        assert_eq!(
            plan("yio build", "docs/yio.md", &scripts, &[12, 20], &vars, Some(("NAMES", &names))),
            "\
# Dry run of `yio build`: nothing below has been run.
TARGET='it'\\''s web'
RELEASE='false'
NAMES=('a' 'b c')

# docs/yio.md, line 12
set -e
echo \"$TARGET\"

# docs/yio.md, line 20: only comments, so bash is not started
# Answered by a plugin.
"
        );
        // A command that takes nothing has no values to show.
        assert_eq!(
            plan("yio lint", "yio.md", &["true\n".to_string()], &[5], &[], None),
            "# Dry run of `yio lint`: nothing below has been run.\n\n# yio.md, line 5\ntrue\n"
        );
    }

    #[test]
    fn quotes_text_as_one_shell_word() {
        assert_eq!(quote("echo \"$HOME\"\n"), "'echo \"$HOME\"\n'");
        assert_eq!(quote("echo 'a b'"), "'echo '\\''a b'\\'''");
    }

    #[test]
    fn writes_a_list_as_a_bash_array() {
        let list = |values: &[&str]| {
            let values: Vec<String> = values.iter().map(|value| value.to_string()).collect();
            array("TERMS", &values)
        };
        assert_eq!(list(&[]), "TERMS=()");
        assert_eq!(list(&["built-in", "functions"]), "TERMS=('built-in' 'functions')");
        // Nothing in a value is left for bash to expand or split.
        assert_eq!(
            list(&["two words", "$HOME", "it's", "", "*"]),
            "TERMS=('two words' '$HOME' 'it'\\''s' '' '*')"
        );
    }
}
