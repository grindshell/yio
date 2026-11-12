//! The `.yio/cache.yio` file: what it holds, how it is encoded, and how to
//! tell whether it is still current. See `docs/scripting-language.md`.
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
//!   input: u8 kind, str path, u64 size, u64 mtime ns, u64 content hash
//!   u32  body length
//!   body
//! ```
//!
//! A unit is one independently cached result together with the inputs it was
//! computed from. The only kind so far is the command tree.

use std::cell::RefCell;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cli::VERSION;
use crate::command::{Command, Flag, FlagType, Param};
use crate::markdown::FILE_NAME;

pub const FILE: &str = ".yio/cache.yio";

/// Bumped whenever the payload layout changes.
const FORMAT: u8 = 1;
const PREFIX: &[u8] = b"(cache #";
const TREE_UNIT: u8 = 1;
const FILE_INPUT: u8 = 1;

/// How far apart two writes must be for their modification times to be sure
/// to differ, on the coarsest file system worth supporting (FAT, at two
/// seconds).
const RESOLUTION_NS: u64 = 2_000_000_000;

/// How deeply commands may nest in a cache file. Real trees stay far below
/// this; it stops a damaged file from overflowing the stack.
const MAX_DEPTH: usize = 256;

/// Nanoseconds since the Unix epoch.
pub fn now() -> u64 {
    since_epoch(SystemTime::now())
}

fn since_epoch(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_nanos() as u64)
}

fn modified(metadata: &fs::Metadata) -> u64 {
    metadata.modified().map_or(0, since_epoch)
}

/// FNV-1a. It only has to notice that a file changed, not resist tampering.
fn hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// A file a cached result was computed from, and what it looked like then.
#[derive(Debug, Clone, PartialEq)]
pub struct Input {
    /// Relative to the current directory, with forward slashes.
    pub path: String,
    pub size: u64,
    pub mtime: u64,
    pub hash: u64,
}

/// Reads files on behalf of the loader and remembers each one as an input.
#[derive(Default)]
pub struct Recorder {
    inputs: RefCell<Vec<Input>>,
}

impl Recorder {
    pub fn read(&self, path: &Path) -> io::Result<String> {
        // The metadata is taken first. If the file changes between the two
        // calls, the recorded time is older than the contents, so the next
        // check falls back to comparing hashes instead of trusting it.
        let metadata = fs::metadata(path)?;
        let text = fs::read_to_string(path)?;
        let input = Input {
            path: path.to_string_lossy().replace('\\', "/"),
            size: text.len() as u64,
            mtime: modified(&metadata),
            hash: hash(text.as_bytes()),
        };
        let mut inputs = self.inputs.borrow_mut();
        if !inputs.iter().any(|known| known.path == input.path) {
            inputs.push(input);
        }
        Ok(text)
    }

    pub fn into_inputs(self) -> Vec<Input> {
        self.inputs.into_inner()
    }
}

/// Whether a cached result can still be used.
#[derive(Debug, PartialEq)]
pub enum Freshness {
    /// Every input is unchanged.
    Clean,
    /// Every input is unchanged, but some had to be read to be sure. Storing
    /// these updated inputs with a new as-of time makes the next check cheap.
    Settled(Vec<Input>),
    /// At least one input changed.
    Dirty,
}

/// Compares `inputs` with the files as they are now. `as_of` is when the
/// inputs were recorded and `now` is the time of this check.
pub fn check(inputs: &[Input], as_of: u64, now: u64) -> Freshness {
    let mut current = Vec::with_capacity(inputs.len());
    let mut settle = false;
    for input in inputs {
        let Ok(metadata) = fs::metadata(&input.path) else {
            return Freshness::Dirty;
        };
        if !metadata.is_file() || metadata.len() != input.size {
            return Freshness::Dirty;
        }
        let mtime = modified(&metadata);
        // A file modified this close to the recording could have been
        // changed again afterwards without its time moving on.
        let racy = mtime.saturating_add(RESOLUTION_NS) >= as_of;
        if mtime != input.mtime || racy {
            // The time proves nothing either way, so compare contents. This
            // also keeps `touch` and same-content rewrites from counting as
            // changes.
            match fs::read(&input.path) {
                Ok(bytes) if hash(&bytes) == input.hash => {}
                _ => return Freshness::Dirty,
            }
            // Re-recording only helps once the file is old enough to trust.
            settle |= mtime.saturating_add(RESOLUTION_NS) < now;
        }
        current.push(Input { mtime, ..input.clone() });
    }
    if settle {
        Freshness::Settled(current)
    } else {
        Freshness::Clean
    }
}

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
        put_u8(&mut payload, FILE_INPUT);
        put_str(&mut payload, &input.path);
        put_u64(&mut payload, input.size);
        put_u64(&mut payload, input.mtime);
        put_u64(&mut payload, input.hash);
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
            if bytes.u8()? != FILE_INPUT {
                return Err("cache has an unknown kind of input".to_string());
            }
            inputs.push(Input {
                path: bytes.str()?.to_string(),
                size: bytes.u64()?,
                mtime: bytes.u64()?,
                hash: bytes.u64()?,
            });
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
    use std::time::Duration;

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
        Input { path: path.to_string(), size: 3, mtime: 42, hash: 7 }
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

    /// A scratch directory that is removed when the test ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("yio-{}-{name}", std::process::id()));
            fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }

        /// Writes a file and sets its modification time. Returns the path
        /// and the input a recorder would have made for it.
        fn file(&self, name: &str, text: &str, mtime: u64) -> (String, Input) {
            let path = self.0.join(name);
            fs::write(&path, text).unwrap();
            set_mtime(&path, mtime);
            let path = path.to_string_lossy().into_owned();
            let recorded = Input {
                path: path.clone(),
                size: text.len() as u64,
                mtime,
                hash: hash(text.as_bytes()),
            };
            (path, recorded)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn set_mtime(path: impl AsRef<Path>, mtime: u64) {
        let file = fs::File::options().write(true).open(path).unwrap();
        file.set_modified(UNIX_EPOCH + Duration::from_nanos(mtime)).unwrap();
    }

    const SECOND: u64 = 1_000_000_000;
    /// An arbitrary modification time, in 2023.
    const THEN: u64 = 1_700_000_000 * SECOND;

    #[test]
    fn an_untouched_file_is_clean_without_being_read() {
        let scratch = Scratch::new("untouched");
        let (_, mut recorded) = scratch.file("yio.md", "one", THEN);
        // A wrong hash proves the file was not read: only the stat is used.
        recorded.hash ^= 1;
        let as_of = THEN + 10 * SECOND;
        assert_eq!(check(&[recorded], as_of, as_of + SECOND), Freshness::Clean);
    }

    #[test]
    fn a_changed_file_is_dirty() {
        let scratch = Scratch::new("changed");
        let (path, recorded) = scratch.file("yio.md", "one", THEN);
        let as_of = THEN + 10 * SECOND;
        let now = as_of + 100 * SECOND;

        fs::write(&path, "longer").unwrap();
        assert_eq!(check(std::slice::from_ref(&recorded), as_of, now), Freshness::Dirty);

        // Same size, later time: only the contents show the change.
        fs::write(&path, "two").unwrap();
        set_mtime(&path, THEN + 50 * SECOND);
        assert_eq!(check(std::slice::from_ref(&recorded), as_of, now), Freshness::Dirty);

        fs::remove_file(&path).unwrap();
        assert_eq!(check(&[recorded], as_of, now), Freshness::Dirty);
    }

    #[test]
    fn a_touched_file_is_still_clean() {
        let scratch = Scratch::new("touched");
        let (path, recorded) = scratch.file("yio.md", "one", THEN);
        let as_of = THEN + 10 * SECOND;
        let touched = THEN + 50 * SECOND;
        set_mtime(&path, touched);

        // Old enough to trust: worth recording the new time.
        let settled = Input { mtime: touched, ..recorded.clone() };
        assert_eq!(
            check(std::slice::from_ref(&recorded), as_of, touched + 10 * SECOND),
            Freshness::Settled(vec![settled])
        );
        // Too recent: clean, but recording it would not help yet.
        assert_eq!(check(&[recorded], as_of, touched + SECOND), Freshness::Clean);
    }

    #[test]
    fn a_file_modified_just_before_recording_is_compared_by_content() {
        let scratch = Scratch::new("racy");
        let (path, recorded) = scratch.file("yio.md", "one", THEN);
        // Recorded one second after the file was written: a second write in
        // the same timestamp tick would look identical to `stat`.
        let as_of = THEN + SECOND;
        let later = THEN + 100 * SECOND;
        assert_eq!(
            check(std::slice::from_ref(&recorded), as_of, later),
            Freshness::Settled(vec![recorded.clone()])
        );

        fs::write(&path, "two").unwrap();
        set_mtime(&path, THEN);
        assert_eq!(check(&[recorded], as_of, later), Freshness::Dirty);
    }

    #[test]
    fn the_recorder_notes_each_file_once() {
        let scratch = Scratch::new("recorder");
        let (path, expected) = scratch.file("yio.md", "## a\n", THEN);
        let recorder = Recorder::default();
        assert_eq!(recorder.read(Path::new(&path)).unwrap(), "## a\n");
        recorder.read(Path::new(&path)).unwrap();
        assert!(recorder.read(&scratch.0.join("missing.md")).is_err());

        let inputs = recorder.into_inputs();
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].path, path.replace('\\', "/"));
        assert_eq!((inputs[0].size, inputs[0].mtime, inputs[0].hash), (5, THEN, expected.hash));
    }
}
