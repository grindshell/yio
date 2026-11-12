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
    /// A script file, relative to the directory it runs in.
    File(&'a str),
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
    for script in scripts {
        // The assignment goes on the script's first line, so that the line
        // numbers in bash's error messages still match the code block.
        let script = match &array {
            Some(array) => Cow::Owned(format!("{array}; {script}")),
            None => Cow::Borrowed(script.as_str()),
        };
        let code = run_one(Script::Text(&script), env, dir)?;
        if code != 0 {
            return Ok(ExitCode::from(code));
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// The bash statement that makes `name` an array holding `values`.
///
/// A list cannot travel as an environment variable, which holds one string,
/// so it is written into the script instead. Assigning it also replaces any
/// variable of that name inherited from the calling shell.
fn array(name: &str, values: &[String]) -> String {
    let words: Vec<String> = values.iter().map(|value| quote(value)).collect();
    format!("{name}=({})", words.join(" "))
}

/// `text` as a single-quoted POSIX shell word, which the shell takes
/// literally whatever it contains.
fn quote(text: &str) -> String {
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

/// Runs one script and gives its exit code, which is zero on success.
pub fn run_one(script: Script, env: &[(&str, String)], dir: Option<&Path>) -> Result<u8, String> {
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
    let status = process
        .status()
        .map_err(|error| format!("could not run `{}`: {error}", bash.display()))?;
    if status.success() {
        return Ok(0);
    }
    // No code means the script was killed by a signal.
    let code = status.code().and_then(|code| u8::try_from(code).ok());
    Ok(code.filter(|code| *code != 0).unwrap_or(1))
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
        Script::Text(text) => process.arg("-c").arg(text),
        Script::File(file) => process.arg(file),
    };
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
            Script::File(file) => process.raw_arg(quote(file)),
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

}

#[cfg(test)]
mod tests {
    use super::{array, quote};

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
