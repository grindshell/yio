//! Writes `yio`'s own text to standard output: listings, what a plugin
//! drew, a dry run.

use std::io::{ErrorKind, Write};

/// Writes `text` to standard output.
///
/// Whoever was reading may have stopped, as `head` does once it has the
/// lines it wants. That is not a failure: the text is dropped and the run
/// carries on, so a command still runs and its exit code is still `yio`'s.
/// `print!` would end the run with a panic instead.
pub fn print(text: &str) -> Result<(), String> {
    let mut out = std::io::stdout().lock();
    match out.write_all(text.as_bytes()) {
        Err(error) if error.kind() != ErrorKind::BrokenPipe => {
            Err(format!("could not write to standard output: {error}"))
        }
        _ => Ok(()),
    }
}
