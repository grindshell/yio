//! The `.yio` scripting language. See `docs/scripting-language.md`.

pub mod builtins;
pub mod eval;
pub mod reader;
pub mod value;

pub use reader::Error;
pub use value::Value;

/// Reads and evaluates the source of a script, giving its value.
pub fn run(source: &[u8]) -> Result<Value, Error> {
    eval::run(&reader::read(source)?)
}
