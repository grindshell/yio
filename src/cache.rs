//! The `.yio/cache.yio` file: what it holds and how it is encoded. Whether
//! its contents are still current is decided by `input.rs`. See
//! `docs/scripting-language.md`.
//!
//! The file is one `.yio` form, `(cache #<length>:<payload>)`. The payload
//! uses the encoding of `bytes.rs`:
//!
//! ```text
//! u8   format version
//! str  yio version
//! u32  unit count
//! unit:
//!   u8   kind
//!   str  source path
//!   u64  as-of time, ns since the Unix epoch
//!   u32  input count
//!   input: u8 kind, then the fields of that kind (see `put_input`)
//!   u32  body length
//!   body
//! ```
//!
//! A unit is one independently cached result together with the inputs it
//! was computed from and when. There are two kinds: the command tree, and
//! the value of one `.yio` plugin.

use std::cell::{Cell, RefCell};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::bytes::{Reader, put_bytes, put_len, put_option, put_str, put_u8, put_u32, put_u64};
use crate::cli::VERSION;
use crate::command::{Command, Flag, FlagType, Param};
use crate::input::{Input, PathKind};

pub const FILE: &str = ".yio/cache.yio";

/// Bumped whenever the payload layout changes.
const FORMAT: u8 = 2;
const PREFIX: &[u8] = b"(cache #";

/// How deeply commands may nest in a cache file. Real trees stay far below
/// this; it stops a damaged file from overflowing the stack.
const MAX_DEPTH: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnitKind {
    /// The command tree. The body is for `decode_tree`.
    Tree,
    /// The value of a `.yio` plugin. The body is for `script::encode`.
    Script,
}

/// One cached result.
#[derive(Debug, Clone, PartialEq)]
pub struct Unit {
    pub kind: UnitKind,
    /// The file the result came from: `yio.md`, or a plugin's path.
    pub source: String,
    /// When the inputs were recorded. See `input::check`.
    pub as_of: u64,
    pub inputs: Vec<Input>,
    pub body: Vec<u8>,
}

/// Builds the contents of a cache file.
pub fn serialize(units: &[Unit]) -> Vec<u8> {
    let mut payload = Vec::new();
    put_u8(&mut payload, FORMAT);
    put_str(&mut payload, VERSION);
    put_len(&mut payload, units.len());
    for unit in units {
        put_u8(&mut payload, match unit.kind {
            UnitKind::Tree => 1,
            UnitKind::Script => 2,
        });
        put_str(&mut payload, &unit.source);
        put_u64(&mut payload, unit.as_of);
        put_len(&mut payload, unit.inputs.len());
        for input in &unit.inputs {
            put_input(&mut payload, input);
        }
        put_bytes(&mut payload, &unit.body);
    }

    let mut file = format!("(cache #{}:", payload.len()).into_bytes();
    file.extend_from_slice(&payload);
    file.extend_from_slice(b")\n");
    file
}

/// Reads a cache file's units without decoding their bodies. Fails if the
/// file is damaged or was written by a different version of yio.
pub fn parse(file: &[u8]) -> Result<Vec<Unit>, String> {
    // This does not go through the general reader: the shape is fixed, so
    // the payload can be found directly.
    let rest = file.strip_prefix(PREFIX).ok_or("not a cache file")?;
    let digits = rest.iter().take_while(|byte| byte.is_ascii_digit()).count();
    let length: usize = std::str::from_utf8(&rest[..digits])
        .ok()
        .and_then(|text| text.parse().ok())
        .ok_or("cache has no payload length")?;
    let rest = rest[digits..].strip_prefix(b":").ok_or("cache has no payload")?;
    if rest.len() < length {
        return Err("cache is truncated".to_string());
    }
    let (payload, tail) = rest.split_at(length);
    if tail.trim_ascii() != b")" {
        return Err("cache is not closed".to_string());
    }

    let mut reader = Reader::new(payload);
    if reader.u8()? != FORMAT {
        return Err("cache has a different format version".to_string());
    }
    if reader.str()? != VERSION {
        return Err("cache was written by a different yio version".to_string());
    }

    let mut units = Vec::new();
    for _ in 0..reader.u32()? {
        let kind = match reader.u8()? {
            1 => UnitKind::Tree,
            2 => UnitKind::Script,
            _ => return Err("cache has an unknown kind of unit".to_string()),
        };
        let source = reader.str()?.to_string();
        let as_of = reader.u64()?;
        let mut inputs = Vec::new();
        for _ in 0..reader.u32()? {
            inputs.push(take_input(&mut reader)?);
        }
        let body = reader.bytes()?.to_vec();
        units.push(Unit { kind, source, as_of, inputs, body });
    }
    if !reader.is_empty() {
        return Err("cache has data after its last unit".to_string());
    }
    Ok(units)
}

/// Writes the cache file, creating `.yio/` if needed. The file is replaced
/// in one step so that a reader never sees half of it.
pub fn write(contents: &[u8]) -> io::Result<()> {
    let file = Path::new(FILE);
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)?;
    }
    let temporary = PathBuf::from(format!("{FILE}.{}.tmp", std::process::id()));
    let written = fs::write(&temporary, contents).and_then(|()| fs::rename(&temporary, file));
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written
}

/// Deletes the cache file. Returns whether there was one.
pub fn remove() -> io::Result<bool> {
    match fs::remove_file(FILE) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// The cache over the course of one run: what the file held at the start,
/// and what it should hold at the end.
///
/// Each result the run needs is either taken from the file with `find` and
/// kept with `keep`, or computed and added with `put`. Whatever is neither
/// kept nor put is dropped from the file, which is how the result of a
/// plugin that no longer exists goes away.
pub struct Session {
    /// Whether there was a cache file. Its existing is what turns caching
    /// on.
    existed: bool,
    /// Whether results should be recorded at all.
    active: bool,
    stored: Vec<Unit>,
    current: RefCell<Vec<Unit>>,
    changed: Cell<bool>,
}

impl Session {
    /// Opens the cache file of the current directory. With `fresh`, what
    /// the file holds is ignored and everything is computed again, which is
    /// what `--cache` asks for.
    pub fn open(fresh: bool) -> Session {
        Session::from_file(fs::read(FILE).ok().as_deref(), fresh)
    }

    pub(crate) fn from_file(file: Option<&[u8]>, fresh: bool) -> Session {
        let existed = file.is_some();
        // A file that cannot be used counts as out of date, so that it gets
        // replaced.
        let (stored, changed) = match file.map(parse) {
            Some(Ok(units)) if !fresh => (units, false),
            Some(_) => (Vec::new(), true),
            None => (Vec::new(), false),
        };
        Session {
            existed,
            active: existed || fresh,
            stored,
            current: RefCell::default(),
            changed: Cell::new(changed),
        }
    }

    /// Whether this run records results. When it does not, there is no
    /// point in gathering what a unit would need.
    pub fn active(&self) -> bool {
        self.active
    }

    /// A result the file held at the start of the run.
    pub fn find(&self, kind: UnitKind, source: &str) -> Option<&Unit> {
        self.stored.iter().find(|unit| unit.kind == kind && unit.source == source)
    }

    /// Carries a stored result over unchanged.
    pub fn keep(&self, unit: &Unit) {
        self.current.borrow_mut().push(unit.clone());
    }

    /// Records a result that is new or differs from what was stored.
    pub fn put(&self, unit: Unit) {
        self.current.borrow_mut().push(unit);
        self.changed.set(true);
    }

    /// The contents the cache file should have after this run.
    pub fn contents(&self) -> Vec<u8> {
        serialize(&self.current.borrow())
    }

    /// The new contents for the cache file, if there was one and this run
    /// left it out of date.
    pub fn refreshed(&self) -> Option<Vec<u8>> {
        let dropped = self.stored.len() != self.current.borrow().len();
        (self.existed && (self.changed.get() || dropped)).then(|| self.contents())
    }
}

const FILE_INPUT: u8 = 1;
const DIR_INPUT: u8 = 2;
const KIND_INPUT: u8 = 3;
const SIZE_INPUT: u8 = 4;
const ENV_INPUT: u8 = 5;

fn put_input(out: &mut Vec<u8>, input: &Input) {
    match input {
        Input::File { path, size, mtime, hash } => {
            put_u8(out, FILE_INPUT);
            put_str(out, path);
            put_u64(out, *size);
            put_u64(out, *mtime);
            put_u64(out, *hash);
        }
        Input::Dir { path, mtime, hash } => {
            put_u8(out, DIR_INPUT);
            put_str(out, path);
            put_u64(out, *mtime);
            put_u64(out, *hash);
        }
        Input::Kind { path, kind } => {
            put_u8(out, KIND_INPUT);
            put_str(out, path);
            put_u8(out, match kind {
                PathKind::Missing => 0,
                PathKind::File => 1,
                PathKind::Dir => 2,
                PathKind::Other => 3,
            });
        }
        Input::Size { path, size } => {
            put_u8(out, SIZE_INPUT);
            put_str(out, path);
            put_u64(out, *size);
        }
        Input::Env { name, hash } => {
            put_u8(out, ENV_INPUT);
            put_str(out, name);
            put_u8(out, u8::from(hash.is_some()));
            put_u64(out, hash.unwrap_or(0));
        }
    }
}

fn take_input(reader: &mut Reader) -> Result<Input, String> {
    let kind = reader.u8()?;
    let path = reader.str()?.to_string();
    Ok(match kind {
        FILE_INPUT => {
            Input::File { path, size: reader.u64()?, mtime: reader.u64()?, hash: reader.u64()? }
        }
        DIR_INPUT => Input::Dir { path, mtime: reader.u64()?, hash: reader.u64()? },
        KIND_INPUT => Input::Kind {
            path,
            kind: match reader.u8()? {
                0 => PathKind::Missing,
                1 => PathKind::File,
                2 => PathKind::Dir,
                3 => PathKind::Other,
                _ => return Err("cache has an unknown kind of path".to_string()),
            },
        },
        SIZE_INPUT => Input::Size { path, size: reader.u64()? },
        ENV_INPUT => {
            let set = reader.u8()? != 0;
            let hash = reader.u64()?;
            Input::Env { name: path, hash: set.then_some(hash) }
        }
        _ => return Err("cache has an unknown kind of input".to_string()),
    })
}

/// Encodes a command and everything under it, parents before children.
pub fn encode_tree(root: &Command) -> Vec<u8> {
    let mut out = Vec::new();
    put_command(&mut out, root);
    out
}

fn put_command(out: &mut Vec<u8>, command: &Command) {
    put_str(out, &command.name);
    put_option(out, command.short_help.as_deref());
    put_option(out, command.long_help.as_deref());
    let dir = command.dir.as_ref().map(|dir| dir.to_string_lossy().replace('\\', "/"));
    put_option(out, dir.as_deref());

    put_len(out, command.params.len());
    for param in &command.params {
        put_str(out, &param.name);
        put_u8(out, u8::from(param.required));
    }
    put_len(out, command.flags.len());
    for flag in &command.flags {
        put_str(out, &flag.name);
        // Zero stands for no short form; it is not a character a flag can use.
        put_u32(out, flag.short.map_or(0, u32::from));
        put_option(out, flag.long.as_deref());
        put_u8(out, match flag.ty {
            FlagType::String => 0,
            FlagType::Boolean => 1,
        });
        put_option(out, flag.default.as_deref());
        put_option(out, flag.description.as_deref());
    }
    put_len(out, command.scripts.len());
    for script in &command.scripts {
        put_str(out, script);
    }
    put_len(out, command.subcommands.len());
    for subcommand in &command.subcommands {
        put_command(out, subcommand);
    }
}

/// Decodes what `encode_tree` produced.
pub fn decode_tree(body: &[u8]) -> Result<Command, String> {
    let mut reader = Reader::new(body);
    let root = take_command(&mut reader, 0)?;
    if !reader.is_empty() {
        return Err("cache has data after the command tree".to_string());
    }
    Ok(root)
}

fn take_command(reader: &mut Reader, depth: usize) -> Result<Command, String> {
    if depth > MAX_DEPTH {
        return Err("cache nests commands too deeply".to_string());
    }
    let mut command = Command {
        name: reader.str()?.to_string(),
        short_help: reader.option()?,
        long_help: reader.option()?,
        dir: reader.option()?.map(PathBuf::from),
        ..Command::default()
    };
    for _ in 0..reader.u32()? {
        command.params.push(Param {
            name: reader.str()?.to_string(),
            required: reader.u8()? != 0,
        });
    }
    for _ in 0..reader.u32()? {
        command.flags.push(Flag {
            name: reader.str()?.to_string(),
            short: match reader.u32()? {
                0 => None,
                code => Some(char::from_u32(code).ok_or("cache has an invalid flag")?),
            },
            long: reader.option()?,
            ty: match reader.u8()? {
                0 => FlagType::String,
                1 => FlagType::Boolean,
                _ => return Err("cache has an unknown flag type".to_string()),
            },
            default: reader.option()?,
            description: reader.option()?,
        });
    }
    for _ in 0..reader.u32()? {
        command.scripts.push(reader.str()?.to_string());
    }
    for _ in 0..reader.u32()? {
        command.subcommands.push(take_command(reader, depth + 1)?);
    }
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown;
    use crate::script::reader::{self, FormKind};

    const SOURCE: &str = "\
> Project tasks

## build (TARGET) [PROFILE]

> Build a target

Longer description.

**FLAGS**
- JOBS
  - flags: -j --jobs
  - default: 4
  - description: Parallel jobs
- RELEASE
  - flags: --release
  - type: boolean

```
echo one
```

```
echo \"two\"
```

### clean

## docs
";

    fn tree() -> Command {
        let mut root = markdown::parse(SOURCE).unwrap();
        root.subcommands[1].dir = Some(PathBuf::from("docs/api"));
        root
    }

    fn input(path: &str) -> Input {
        Input::File { path: path.to_string(), size: 3, mtime: 42, hash: 7 }
    }

    fn tree_unit() -> Unit {
        Unit {
            kind: UnitKind::Tree,
            source: "yio.md".to_string(),
            as_of: 1234,
            inputs: vec![input("yio.md"), input("docs/yio.md")],
            body: encode_tree(&tree()),
        }
    }

    fn script_unit(source: &str) -> Unit {
        Unit {
            kind: UnitKind::Script,
            source: source.to_string(),
            as_of: 99,
            inputs: vec![input(source)],
            body: vec![1, 2, 3],
        }
    }

    #[test]
    fn a_tree_survives_a_round_trip() {
        let root = tree();
        assert_eq!(decode_tree(&encode_tree(&root)).unwrap(), root);
    }

    #[test]
    fn a_file_survives_a_round_trip() {
        let units = [tree_unit(), script_unit(".yio/pre/1_a.yio"), script_unit("docs/.yio/post/2_b.yio")];
        assert_eq!(parse(&serialize(&units)).unwrap(), units);
        assert_eq!(parse(&serialize(&[])).unwrap(), []);
    }

    #[test]
    fn every_kind_of_input_survives_a_round_trip() {
        let inputs = vec![
            input("yio.md"),
            Input::Dir { path: "lore".to_string(), mtime: 9, hash: u64::MAX },
            Input::Kind { path: "a".to_string(), kind: PathKind::Missing },
            Input::Kind { path: "b".to_string(), kind: PathKind::File },
            Input::Kind { path: "c".to_string(), kind: PathKind::Dir },
            Input::Kind { path: "d".to_string(), kind: PathKind::Other },
            Input::Size { path: "lore/a.md".to_string(), size: 12 },
            Input::Env { name: "HOME".to_string(), hash: Some(0) },
            Input::Env { name: "UNSET".to_string(), hash: None },
        ];
        let unit = Unit { inputs, ..script_unit("x") };
        assert_eq!(parse(&serialize(std::slice::from_ref(&unit))).unwrap(), [unit]);
    }

    #[test]
    fn the_file_is_one_yio_form() {
        let file = serialize(&[tree_unit()]);
        let forms = reader::read(&file).unwrap();
        assert_eq!(forms.len(), 1);
        let FormKind::List(items) = &forms[0].kind else {
            panic!("expected a list");
        };
        assert_eq!(items[0].kind, FormKind::Symbol("cache".to_string()));
        assert!(matches!(items[1].kind, FormKind::Bytes(_)));
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn damaged_files_are_rejected_without_panicking() {
        let file = serialize(&[tree_unit(), script_unit("a")]);
        for length in 0..file.len() - 1 {
            assert!(parse(&file[..length]).is_err(), "prefix of {length} bytes");
        }
        let body = encode_tree(&tree());
        for length in 0..body.len() {
            assert!(decode_tree(&body[..length]).is_err(), "prefix of {length} bytes");
        }
        // Counts that claim more than the file holds must not allocate or loop.
        let mut huge = body.clone();
        let end = huge.len();
        huge[end - 4..].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode_tree(&huge).is_err());
    }

    #[test]
    fn other_versions_are_rejected() {
        let file = serialize(&[tree_unit()]);
        let start = PREFIX.len() + file[PREFIX.len()..].iter().position(|b| *b == b':').unwrap() + 1;

        let mut format = file.clone();
        format[start] = FORMAT + 1;
        assert_eq!(parse(&format).err().unwrap(), "cache has a different format version");

        // The version string follows the format byte and its own length.
        let mut version = file.clone();
        version[start + 5] ^= 1;
        assert_eq!(
            parse(&version).err().unwrap(),
            "cache was written by a different yio version"
        );
    }

    #[test]
    fn a_session_without_a_file_records_nothing() {
        let session = Session::from_file(None, false);
        assert!(!session.active());
        assert_eq!(session.find(UnitKind::Tree, "yio.md"), None);
        session.put(tree_unit());
        assert_eq!(session.refreshed(), None);
    }

    #[test]
    fn a_session_that_reuses_everything_leaves_the_file_alone() {
        let file = serialize(&[tree_unit(), script_unit("a")]);
        let session = Session::from_file(Some(&file), false);
        assert!(session.active());
        session.keep(session.find(UnitKind::Tree, "yio.md").unwrap());
        session.keep(session.find(UnitKind::Script, "a").unwrap());
        assert_eq!(session.refreshed(), None);
        assert_eq!(session.contents(), file);
        // Kind and source together identify a unit.
        assert_eq!(session.find(UnitKind::Script, "yio.md"), None);
    }

    #[test]
    fn a_session_rewrites_the_file_when_a_result_changes_or_goes_away() {
        let file = serialize(&[tree_unit(), script_unit("a")]);

        // A recomputed result.
        let session = Session::from_file(Some(&file), false);
        session.keep(session.find(UnitKind::Tree, "yio.md").unwrap());
        let newer = Unit { body: vec![9], ..script_unit("a") };
        session.put(newer.clone());
        assert_eq!(parse(&session.refreshed().unwrap()).unwrap(), [tree_unit(), newer]);

        // A plugin that no longer exists: its result is not carried over.
        let session = Session::from_file(Some(&file), false);
        session.keep(session.find(UnitKind::Tree, "yio.md").unwrap());
        assert_eq!(parse(&session.refreshed().unwrap()).unwrap(), [tree_unit()]);
    }

    #[test]
    fn an_unusable_file_is_replaced() {
        let session = Session::from_file(Some(b"(cache #3:abc)"), false);
        assert!(session.active());
        assert_eq!(session.find(UnitKind::Tree, "yio.md"), None);
        session.put(tree_unit());
        assert_eq!(parse(&session.refreshed().unwrap()).unwrap(), [tree_unit()]);
    }

    #[test]
    fn a_fresh_session_ignores_the_file_and_can_start_one() {
        let file = serialize(&[tree_unit()]);
        let session = Session::from_file(Some(&file), true);
        assert_eq!(session.find(UnitKind::Tree, "yio.md"), None);
        assert!(session.refreshed().is_some());

        // `--cache` with no file yet: results are recorded, but nothing is
        // written unless it is asked for through `contents`.
        let session = Session::from_file(None, true);
        assert!(session.active());
        session.put(tree_unit());
        assert_eq!(session.refreshed(), None);
        assert_eq!(parse(&session.contents()).unwrap(), [tree_unit()]);
    }
}
