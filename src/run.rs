//! Runs a command's scripts.

use std::path::PathBuf;
use std::process::{Command as Process, ExitCode};

/// Runs each script in its own bash process, in order, adding `env` to the
/// inherited environment. Nothing a script does to its shell carries over to
/// the next one. Stops at the first script that fails and returns its exit
/// code.
pub fn run(scripts: &[String], env: &[(&str, String)]) -> Result<ExitCode, String> {
    let bash = bash();
    for script in scripts {
        let mut process = Process::new(&bash);
        process.arg("-c").arg(script);
        process.envs(env.iter().map(|(name, value)| (*name, value.as_str())));

        // When the bash found on a Windows PATH is the WSL launcher, only the
        // variables named in WSLENV are forwarded. Other bashes ignore it.
        // The loader reserves the name, so this never replaces a user value.
        #[cfg(windows)]
        {
            let mut wslenv = std::env::var("WSLENV").unwrap_or_default();
            for (name, _) in env {
                if !wslenv.is_empty() {
                    wslenv.push(':');
                }
                wslenv.push_str(name);
            }
            process.env("WSLENV", wslenv);
        }

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
