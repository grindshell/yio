//! The `.yio/cache.yio` file: what it holds and how it is encoded. Whether
//! its contents are still current is decided by `input.rs`. See
//! `docs/scripting-language.md`.
//!
//! The file is one `.yio` form, `(cache #<length>:<payload>)`. The payload is
//! a custom binary encoding: integers are fixed-width little-endian and a
//! string is a `u32` length followed by UTF-8.
//!
//! ```text
//! u8   format version
//! str  yio version
//! u64  as-of time, ns since the Unix epoch
//! u32  unit count
//! unit:
//!   u8   kind
//!   str  source path
//!   u32  input count
//!   input: u8 kind, then the fields of that kind (see `put_input`)
//!   u32  body length
//!   body
//! ```
//!
//! A unit is one independently cached result together with the inputs it was
//! computed from. The only kind so far is the command tree.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::cli::VERSION;
use crate::command::{Command, Flag, FlagType, Param};
use crate::input::{Input, PathKind};
use crate::markdown::FILE_NAME;

pub const FILE: &str = ".yio/cache.yio";

/// Bumped whenever the payload layout changes.
const FORMAT: u8 = 1;
const PREFIX: &[u8] = b"(cache #";
const TREE_UNIT: u8 = 1;

/// How deeply commands may nest in a cache file. Real trees stay far below
/// this; it stops a damaged file from overflowing the stack.
const MAX_DEPTH: usize = 256;

/// The command tree unit of a cache file.
pub struct Stored<'a> {
    pub as_of: u64,
    pub inputs: Vec<Input>,
    /// The encoded tree, for `decode_tree`.
    pub body: &'a [u8],
}

/// Builds the contents of a cache file holding one command tree.
pub fn serialize(as_of: u64, inputs: &[Input], tree: &[u8]) -> Vec<u8> {
    let mut payload = Vec::new();
    put_u8(&mut payload, FORMAT);
    put_str(&mut payload, VERSION);
    put_u64(&mut payload, as_of);

    put_len(&mut payload, 1);
    put_u8(&mut payload, TREE_UNIT);
    put_str(&mut payload, FILE_NAME);
    put_len(&mut payload, inputs.len());
    for input in inputs {
        put_input(&mut payload, input);
    }
    put_len(&mut payload, tree.len());
    payload.extend_from_slice(tree);

    let mut file = format!("(cache #{}:", payload.len()).into_bytes();
    file.extend_from_slice(&payload);
    file.extend_from_slice(b")\n");
    file
}

/// Reads the header and unit table of a cache file without decoding any
/// unit's body. Fails if the file is damaged or was written by a different
/// version of yio.
pub fn parse(file: &[u8]) -> Result<Stored<'_>, String> {
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

    let mut bytes = Bytes { data: payload };
    if bytes.u8()? != FORMAT {
        return Err("cache has a different format version".to_string());
    }
    if bytes.str()? != VERSION {
        return Err("cache was written by a different yio version".to_string());
    }
    let as_of = bytes.u64()?;

    let mut tree = None;
    for _ in 0..bytes.u32()? {
        let kind = bytes.u8()?;
        let _source = bytes.str()?;
        let mut inputs = Vec::new();
        for _ in 0..bytes.u32()? {
            inputs.push(take_input(&mut bytes)?);
        }
        let length = bytes.u32()? as usize;
        let body = bytes.take(length)?;
        // The length is what lets a unit be passed over without decoding it.
        if kind == TREE_UNIT {
            tree = Some(Stored { as_of, inputs, body });
        }
    }
    tree.ok_or_else(|| "cache has no command tree".to_string())
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

fn take_input(bytes: &mut Bytes) -> Result<Input, String> {
    let kind = bytes.u8()?;
    let path = bytes.str()?.to_string();
    Ok(match kind {
        FILE_INPUT => {
            Input::File { path, size: bytes.u64()?, mtime: bytes.u64()?, hash: bytes.u64()? }
        }
        DIR_INPUT => Input::Dir { path, mtime: bytes.u64()?, hash: bytes.u64()? },
        KIND_INPUT => Input::Kind {
            path,
            kind: match bytes.u8()? {
                0 => PathKind::Missing,
                1 => PathKind::File,
                2 => PathKind::Dir,
                3 => PathKind::Other,
                _ => return Err("cache has an unknown kind of path".to_string()),
            },
        },
        SIZE_INPUT => Input::Size { path, size: bytes.u64()? },
        ENV_INPUT => {
            let set = bytes.u8()? != 0;
            let hash = bytes.u64()?;
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
    let mut bytes = Bytes { data: body };
    let root = take_command(&mut bytes, 0)?;
    if !bytes.data.is_empty() {
        return Err("cache has data after the command tree".to_string());
    }
    Ok(root)
}

fn take_command(bytes: &mut Bytes, depth: usize) -> Result<Command, String> {
    if depth > MAX_DEPTH {
        return Err("cache nests commands too deeply".to_string());
    }
    let mut command = Command {
        name: bytes.str()?.to_string(),
        short_help: bytes.option()?,
        long_help: bytes.option()?,
        dir: bytes.option()?.map(PathBuf::from),
        ..Command::default()
    };
    for _ in 0..bytes.u32()? {
        command.params.push(Param {
            name: bytes.str()?.to_string(),
            required: bytes.u8()? != 0,
        });
    }
    for _ in 0..bytes.u32()? {
        command.flags.push(Flag {
            name: bytes.str()?.to_string(),
            short: match bytes.u32()? {
                0 => None,
                code => Some(char::from_u32(code).ok_or("cache has an invalid flag")?),
            },
            long: bytes.option()?,
            ty: match bytes.u8()? {
                0 => FlagType::String,
                1 => FlagType::Boolean,
                _ => return Err("cache has an unknown flag type".to_string()),
            },
            default: bytes.option()?,
            description: bytes.option()?,
        });
    }
    for _ in 0..bytes.u32()? {
        command.scripts.push(bytes.str()?.to_string());
    }
    for _ in 0..bytes.u32()? {
        command.subcommands.push(take_command(bytes, depth + 1)?);
    }
    Ok(command)
}

fn put_u8(out: &mut Vec<u8>, value: u8) {
    out.push(value);
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_len(out: &mut Vec<u8>, length: usize) {
    put_u32(out, u32::try_from(length).expect("nothing in a yio.md reaches 4 GiB"));
}

fn put_str(out: &mut Vec<u8>, text: &str) {
    put_len(out, text.len());
    out.extend_from_slice(text.as_bytes());
}

fn put_option(out: &mut Vec<u8>, text: Option<&str>) {
    put_u8(out, u8::from(text.is_some()));
    if let Some(text) = text {
        put_str(out, text);
    }
}

/// The unread remainder of a payload. Every read is bounds-checked, so a
/// damaged file produces an error rather than a panic.
struct Bytes<'a> {
    data: &'a [u8],
}

impl<'a> Bytes<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8], String> {
        let (taken, rest) = self.data.split_at_checked(length).ok_or("cache is truncated")?;
        self.data = rest;
        Ok(taken)
    }

    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, String> {
        let bytes = self.take(4)?.try_into().expect("took four bytes");
        Ok(u32::from_le_bytes(bytes))
    }

    fn u64(&mut self) -> Result<u64, String> {
        let bytes = self.take(8)?.try_into().expect("took eight bytes");
        Ok(u64::from_le_bytes(bytes))
    }

    fn str(&mut self) -> Result<&'a str, String> {
        let length = self.u32()? as usize;
        let bytes = self.take(length)?;
        std::str::from_utf8(bytes).map_err(|_| "cache has text that is not UTF-8".to_string())
    }

    fn option(&mut self) -> Result<Option<String>, String> {
        match self.u8()? {
            0 => Ok(None),
            _ => Ok(Some(self.str()?.to_string())),
        }
    }
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

    #[test]
    fn a_tree_survives_a_round_trip() {
        let root = tree();
        assert_eq!(decode_tree(&encode_tree(&root)).unwrap(), root);
    }

    #[test]
    fn a_file_survives_a_round_trip() {
        let inputs = [input("yio.md"), input("docs/yio.md")];
        let body = encode_tree(&tree());
        let file = serialize(1234, &inputs, &body);
        let stored = parse(&file).unwrap();
        assert_eq!(stored.as_of, 1234);
        assert_eq!(stored.inputs, inputs);
        assert_eq!(stored.body, body);
    }

    #[test]
    fn every_kind_of_input_survives_a_round_trip() {
        let inputs = [
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
        let file = serialize(0, &inputs, &encode_tree(&tree()));
        assert_eq!(parse(&file).unwrap().inputs, inputs);
    }

    #[test]
    fn the_file_is_one_yio_form() {
        let file = serialize(0, &[input("yio.md")], &encode_tree(&tree()));
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
        let file = serialize(0, &[input("yio.md")], &encode_tree(&tree()));
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
        let file = serialize(0, &[], &encode_tree(&tree()));
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
}
