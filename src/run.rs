//! Runs a command's scripts.

use std::process::{Command as Process, ExitCode};

/// Runs each script in its own bash process, in order, adding `env` to the
/// inherited environment. Nothing a script does to its shell carries over to
/// the next one. Stops at the first script that fails and returns its exit
/// code.
pub fn run(scripts: &[String], env: &[(&str, String)]) -> Result<ExitCode, String> {
    for script in scripts {
        let mut process = Process::new("bash");
        process.arg("-c").arg(script);
        process.envs(env.iter().map(|(name, value)| (*name, value.as_str())));

        // On Windows the `bash` on PATH is often the WSL launcher, which only
        // forwards the variables named in WSLENV. Other bashes ignore it.
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
            .map_err(|error| format!("could not run bash: {error}"))?;
        if !status.success() {
            // No code means the script was killed by a signal.
            let code = status.code().and_then(|code| u8::try_from(code).ok());
            return Ok(ExitCode::from(code.unwrap_or(1)));
        }
    }
    Ok(ExitCode::SUCCESS)
}
