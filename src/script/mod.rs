//! The `.yio` scripting language. `docs/scripting-api.md` is the reference
//! and `docs/scripting-language.md` the design.

pub mod builtins;
pub mod encode;
pub mod eval;
pub mod reader;
pub mod render;
pub mod value;

use std::path::Path;
use std::thread;

pub use eval::Outcome;
pub use reader::{Error, Pos};

use crate::input::Vars;

/// The stack a script runs on. Evaluation recurses on the native stack, and
/// the main thread's (1 MiB on Windows) is too small for the depth limit in
/// `eval` to be both useful and safe. Only the part that is used gets
/// committed.
const STACK_SIZE: usize = 64 * 1024 * 1024;

/// Reads and evaluates the source of a script, then hands what it produced
/// to `finish` and returns the result of that.
///
/// Relative paths in the script are resolved against `base`, which is itself
/// relative to the current directory. `vars` are variables the script sees
/// through `env` ahead of the real environment.
///
/// The script runs on its own thread, and its values cannot leave that
/// thread, which is why the caller receives them through `finish`: render,
/// encode, or otherwise reduce the outcome there to something plain.
pub fn run<T: Send>(
    source: &[u8],
    base: &Path,
    vars: Vars,
    finish: impl FnOnce(Outcome) -> T + Send,
) -> Result<T, Error> {
    let work = || Ok(finish(eval::run(&reader::read(source)?, base, vars)?));
    thread::scope(|scope| {
        let thread = thread::Builder::new().stack_size(STACK_SIZE);
        match thread.spawn_scoped(scope, work) {
            Ok(handle) => handle.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic)),
            Err(cause) => Err(Error {
                pos: Pos { line: 1, column: 1 },
                message: format!("could not start a thread to run the script: {cause}"),
            }),
        }
    })
}
