//! Runs a command's scripts.

use std::path::{Path, PathBuf};
use std::process::{Command as Process, ExitCode};

/// Runs each script in its own bash process, in order, adding `env` to the
/// inherited environment. Nothing a script does to its shell carries over to
/// the next one. Stops at the first script that fails and returns its exit
/// code.
///
/// The scripts run in `dir` when it is given, and in the current directory
/// otherwise.
pub fn run(
    scripts: &[String],
    env: &[(&str, String)],
    dir: Option<&Path>,
) -> Result<ExitCode, String> {
    let bash = bash();
    for script in scripts {
        let mut process = Process::new(&bash);
        pass_script(&mut process, &bash, script);
        if let Some(dir) = dir {
            process.current_dir(dir);
        }
        process.envs(env.iter().map(|(name, value)| (*name, value.as_str())));
        #[cfg(windows)]
        wsl::forward(&mut process, env);

        let status = process
            .status()
            .map_err(|error| format!("could not run `{}`: {error}", bash.display()))?;
        if !status.success() {
            // No code means the script was killed by a signal.
            let code = status.code().and_then(|code| u8::try_from(code).ok());
            return Ok(ExitCode::from(code.unwrap_or(1)));
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Adds the arguments that make `bash` run `script`.
fn pass_script(process: &mut Process, bash: &Path, script: &str) {
    process.arg("-c");
    #[cfg(windows)]
    if wsl::is_launcher(bash) {
        wsl::pass_script(process, script);
        return;
    }
    let _ = bash;
    process.arg(script);
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
    /// `$1` is gone. Writing the script as one single-quoted shell word
    /// makes that first pass leave it alone.
    pub fn pass_script(process: &mut Process, script: &str) {
        process.raw_arg(quote(script));
    }

    /// `text` as a single-quoted POSIX shell word.
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

    #[cfg(test)]
    mod tests {
        use super::quote;

        #[test]
        fn quotes_a_script_as_one_shell_word() {
            assert_eq!(quote("echo \"$HOME\"\n"), "'echo \"$HOME\"\n'");
            assert_eq!(quote("echo 'a b'"), "'echo '\\''a b'\\'''");
        }
    }
}
