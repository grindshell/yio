//! What a cached result was computed from, and whether that still holds.
//!
//! Anything that looks at the outside world on behalf of a cached result
//! does it through this module, which hands back both what it saw and an
//! [`Input`] describing it. Checking the inputs later says whether the
//! result can be reused. See "Dirtiness" in `docs/scripting-language.md`.

use std::cell::RefCell;
use std::fs;
use std::io;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// How far apart two writes must be for their modification times to be sure
/// to differ, on the coarsest file system worth supporting (FAT, at two
/// seconds).
const RESOLUTION_NS: u64 = 2_000_000_000;

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

/// FNV-1a. It only has to notice that something changed, not resist
/// tampering.
pub fn hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// One thing a result depended on, and what it looked like at the time.
/// Paths are relative to the current directory, with forward slashes.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    /// The contents of a file.
    File { path: String, size: u64, mtime: u64, hash: u64 },
    /// The names in a directory. `hash` covers the sorted names.
    Dir { path: String, mtime: u64, hash: u64 },
    /// What a path is, including that nothing is there.
    Kind { path: String, kind: PathKind },
    /// The size of a file, and nothing else about it.
    Size { path: String, size: u64 },
    /// The value of an environment variable. `None` means it was unset.
    Env { name: String, hash: Option<u64> },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathKind {
    Missing,
    File,
    Dir,
    /// Something else, such as a device.
    Other,
}

/// A path the way inputs spell it.
fn spell(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Reads a file.
pub fn read_file(path: &Path) -> io::Result<(Vec<u8>, Input)> {
    // The metadata is taken first. If the file changes between the two
    // calls, the recorded time is older than the contents, so the next
    // check falls back to comparing hashes instead of trusting it.
    let metadata = fs::metadata(path)?;
    let bytes = fs::read(path)?;
    let input = Input::File {
        path: spell(path),
        size: bytes.len() as u64,
        mtime: modified(&metadata),
        hash: hash(&bytes),
    };
    Ok((bytes, input))
}

/// Lists the names in a directory, sorted.
pub fn list_dir(path: &Path) -> io::Result<(Vec<String>, Input)> {
    let metadata = fs::metadata(path)?;
    let names = entry_names(path)?;
    let input = Input::Dir { path: spell(path), mtime: modified(&metadata), hash: names_hash(&names) };
    Ok((names, input))
}

fn entry_names(path: &Path) -> io::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in fs::read_dir(path)? {
        names.push(entry?.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    Ok(names)
}

fn names_hash(names: &[String]) -> u64 {
    // No file name contains a NUL, so joined names cannot collide.
    hash(names.join("\0").as_bytes())
}

/// Says what a path is.
pub fn path_kind(path: &Path) -> (PathKind, Input) {
    let kind = match fs::metadata(path) {
        Err(_) => PathKind::Missing,
        Ok(metadata) if metadata.is_file() => PathKind::File,
        Ok(metadata) if metadata.is_dir() => PathKind::Dir,
        Ok(_) => PathKind::Other,
    };
    (kind, Input::Kind { path: spell(path), kind })
}

/// The size of a file in bytes.
pub fn file_size(path: &Path) -> io::Result<(u64, Input)> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(io::Error::other("not a file"));
    }
    let size = metadata.len();
    Ok((size, Input::Size { path: spell(path), size }))
}

/// Variables that are looked up ahead of the real environment: the `YIO_*`
/// values of the current run.
pub type Vars<'a> = &'a [(&'a str, String)];

/// Reads an environment variable, preferring `vars`.
pub fn env_var(name: &str, vars: Vars) -> (Option<String>, Input) {
    let value = match vars.iter().find(|(known, _)| *known == name) {
        Some((_, value)) => Some(value.clone()),
        None => std::env::var_os(name).map(|value| value.to_string_lossy().into_owned()),
    };
    let hash = value.as_ref().map(|value| hash(value.as_bytes()));
    (value, Input::Env { name: name.to_string(), hash })
}

/// Reads text files on behalf of the loader and remembers each one.
#[derive(Default)]
pub struct Recorder {
    inputs: RefCell<Vec<Input>>,
}

impl Recorder {
    pub fn read(&self, path: &Path) -> io::Result<String> {
        let (bytes, input) = read_file(path)?;
        let text = String::from_utf8(bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "file is not valid UTF-8"))?;
        let mut inputs = self.inputs.borrow_mut();
        if !inputs.contains(&input) {
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

/// Compares `inputs` with the world as it is now. `as_of` is when the
/// inputs were recorded, `now` is the time of this check, and `vars` are
/// the variables that stand ahead of the environment for this run.
pub fn check(inputs: &[Input], as_of: u64, now: u64, vars: Vars) -> Freshness {
    let mut current = Vec::with_capacity(inputs.len());
    let mut settle = false;
    for input in inputs {
        match verdict(input, as_of, vars) {
            Verdict::Clean => current.push(input.clone()),
            Verdict::Reread { input, mtime } => {
                // Re-recording only helps once the time is old enough to
                // be trusted on the next check.
                settle |= mtime.saturating_add(RESOLUTION_NS) < now;
                current.push(input);
            }
            Verdict::Dirty => return Freshness::Dirty,
        }
    }
    if settle {
        Freshness::Settled(current)
    } else {
        Freshness::Clean
    }
}

enum Verdict {
    Clean,
    /// Unchanged, but the modification time could not show it, so the
    /// contents were compared. Carries the input as it stands now.
    Reread { input: Input, mtime: u64 },
    Dirty,
}

fn verdict(input: &Input, as_of: u64, vars: Vars) -> Verdict {
    // A modification time proves the contents are unchanged only if it is
    // the recorded one and is not so close to the recording that a later
    // write could have left it the same.
    let proves = |mtime: u64, recorded: u64| {
        mtime == recorded && mtime.saturating_add(RESOLUTION_NS) < as_of
    };
    let clean_if = |clean: bool| if clean { Verdict::Clean } else { Verdict::Dirty };

    match input {
        Input::File { path, size, mtime: recorded, hash: expected } => {
            let Ok(metadata) = fs::metadata(path) else {
                return Verdict::Dirty;
            };
            if !metadata.is_file() || metadata.len() != *size {
                return Verdict::Dirty;
            }
            let mtime = modified(&metadata);
            if proves(mtime, *recorded) {
                return Verdict::Clean;
            }
            // Comparing contents also keeps `touch` and same-content
            // rewrites from counting as changes.
            match fs::read(path) {
                Ok(bytes) if hash(&bytes) == *expected => Verdict::Reread {
                    input: Input::File { path: path.clone(), size: *size, mtime, hash: *expected },
                    mtime,
                },
                _ => Verdict::Dirty,
            }
        }
        Input::Dir { path, mtime: recorded, hash: expected } => {
            let Ok(metadata) = fs::metadata(path) else {
                return Verdict::Dirty;
            };
            if !metadata.is_dir() {
                return Verdict::Dirty;
            }
            let mtime = modified(&metadata);
            if proves(mtime, *recorded) {
                return Verdict::Clean;
            }
            match entry_names(Path::new(path)) {
                Ok(names) if names_hash(&names) == *expected => Verdict::Reread {
                    input: Input::Dir { path: path.clone(), mtime, hash: *expected },
                    mtime,
                },
                _ => Verdict::Dirty,
            }
        }
        Input::Kind { path, kind } => clean_if(path_kind(Path::new(path)).0 == *kind),
        Input::Size { path, size } => {
            clean_if(file_size(Path::new(path)).is_ok_and(|(current, _)| current == *size))
        }
        Input::Env { name, hash: expected } => {
            let (_, current) = env_var(name, vars);
            clean_if(matches!(current, Input::Env { hash, .. } if hash == *expected))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{SECOND, Scratch, THEN, set_mtime};

    /// Writes a file with a set modification time and returns its path and
    /// the input that reading it then would have recorded.
    fn file(scratch: &Scratch, name: &str, text: &str, mtime: u64) -> (String, Input) {
        let path = scratch.write(name, text);
        set_mtime(&path, mtime);
        let path = path.to_string_lossy().into_owned();
        let input = Input::File {
            path: path.clone(),
            size: text.len() as u64,
            mtime,
            hash: hash(text.as_bytes()),
        };
        (path, input)
    }

    #[test]
    fn an_untouched_file_is_clean_without_being_read() {
        let scratch = Scratch::new("input-untouched");
        let (_, recorded) = file(&scratch, "yio.md", "one", THEN);
        // A wrong hash proves the file was not read: only the stat is used.
        let Input::File { path, size, mtime, hash } = recorded else { unreachable!() };
        let recorded = Input::File { path, size, mtime, hash: hash ^ 1 };
        let as_of = THEN + 10 * SECOND;
        assert_eq!(check(&[recorded], as_of, as_of + SECOND, &[]), Freshness::Clean);
    }

    #[test]
    fn a_changed_file_is_dirty() {
        let scratch = Scratch::new("input-changed");
        let (path, recorded) = file(&scratch, "yio.md", "one", THEN);
        let as_of = THEN + 10 * SECOND;
        let now = as_of + 100 * SECOND;

        fs::write(&path, "longer").unwrap();
        assert_eq!(check(std::slice::from_ref(&recorded), as_of, now, &[]), Freshness::Dirty);

        // Same size, later time: only the contents show the change.
        fs::write(&path, "two").unwrap();
        set_mtime(&path, THEN + 50 * SECOND);
        assert_eq!(check(std::slice::from_ref(&recorded), as_of, now, &[]), Freshness::Dirty);

        fs::remove_file(&path).unwrap();
        assert_eq!(check(&[recorded], as_of, now, &[]), Freshness::Dirty);
    }

    #[test]
    fn a_touched_file_is_still_clean() {
        let scratch = Scratch::new("input-touched");
        let (path, recorded) = file(&scratch, "yio.md", "one", THEN);
        let as_of = THEN + 10 * SECOND;
        let touched = THEN + 50 * SECOND;
        set_mtime(&path, touched);

        // Old enough to trust: worth recording the new time.
        let Input::File { size, hash, .. } = recorded else { unreachable!() };
        let settled = Input::File { path: path.clone(), size, mtime: touched, hash };
        assert_eq!(
            check(std::slice::from_ref(&recorded), as_of, touched + 10 * SECOND, &[]),
            Freshness::Settled(vec![settled])
        );
        // Too recent: clean, but recording it would not help yet.
        assert_eq!(check(&[recorded], as_of, touched + SECOND, &[]), Freshness::Clean);
    }

    #[test]
    fn a_file_modified_just_before_recording_is_compared_by_content() {
        let scratch = Scratch::new("input-racy");
        let (path, recorded) = file(&scratch, "yio.md", "one", THEN);
        // Recorded one second after the file was written: a second write in
        // the same timestamp tick would look identical to `stat`.
        let as_of = THEN + SECOND;
        let later = THEN + 100 * SECOND;
        assert_eq!(
            check(std::slice::from_ref(&recorded), as_of, later, &[]),
            Freshness::Settled(vec![recorded.clone()])
        );

        fs::write(&path, "two").unwrap();
        set_mtime(&path, THEN);
        assert_eq!(check(&[recorded], as_of, later, &[]), Freshness::Dirty);
    }

    #[test]
    fn the_recorder_notes_each_file_once() {
        let scratch = Scratch::new("input-recorder");
        let (path, expected) = file(&scratch, "yio.md", "## a\n", THEN);
        let recorder = Recorder::default();
        assert_eq!(recorder.read(Path::new(&path)).unwrap(), "## a\n");
        recorder.read(Path::new(&path)).unwrap();
        assert!(recorder.read(&scratch.path("missing.md")).is_err());

        let Input::File { size, hash, .. } = expected else { unreachable!() };
        let path = path.replace('\\', "/");
        assert_eq!(recorder.into_inputs(), [Input::File { path, size, mtime: THEN, hash }]);
    }

    #[test]
    fn a_listing_changes_when_names_do_but_not_when_contents_do() {
        let scratch = Scratch::new("input-listing");
        scratch.write("lore/b.md", "b");
        scratch.write("lore/a.md", "a");
        let dir = scratch.path("lore");
        // The real time of recording, as in a real run. The directory was
        // changed moments ago, so its modification time cannot be trusted
        // and every check below compares the names.
        let as_of = super::now();
        let now = as_of + 100 * SECOND;
        let (names, recorded) = list_dir(&dir).unwrap();
        assert_eq!(names, ["a.md", "b.md"]);
        assert_ne!(check(std::slice::from_ref(&recorded), as_of, now, &[]), Freshness::Dirty);

        // Editing a file in the directory does not change the listing.
        fs::write(dir.join("a.md"), "edited").unwrap();
        assert_ne!(check(std::slice::from_ref(&recorded), as_of, now, &[]), Freshness::Dirty);

        fs::write(dir.join("c.md"), "c").unwrap();
        assert_eq!(check(std::slice::from_ref(&recorded), as_of, now, &[]), Freshness::Dirty);

        // Back to the same names: unchanged, though it took a re-listing.
        fs::remove_file(dir.join("c.md")).unwrap();
        assert_ne!(check(std::slice::from_ref(&recorded), as_of, now, &[]), Freshness::Dirty);

        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(check(&[recorded], as_of, now, &[]), Freshness::Dirty);
    }

    #[test]
    fn a_settled_listing_is_clean_without_being_listed_again() {
        let scratch = Scratch::new("input-listing-settled");
        scratch.write("lore/a.md", "a");
        let (_, recorded) = list_dir(&scratch.path("lore")).unwrap();
        // A wrong hash proves the names were not read: only the stat is
        // used. Nothing touches the directory after this point, so it is
        // safe to pretend the listing was recorded long after it changed.
        let Input::Dir { path, mtime, hash } = recorded else { unreachable!() };
        let recorded = Input::Dir { path, mtime, hash: hash ^ 1 };
        let as_of = mtime + 10 * SECOND;
        assert_eq!(check(&[recorded], as_of, as_of + SECOND, &[]), Freshness::Clean);
    }

    #[test]
    fn a_change_in_the_same_clock_tick_as_the_recording_is_still_noticed() {
        // File times come from a clock that can stand still for several
        // milliseconds, so a directory can change twice and show one time.
        // This is why a time close to the recording is never trusted.
        let scratch = Scratch::new("input-listing-tick");
        let dir = scratch.path("lore");
        for round in 0..50 {
            let _ = fs::remove_dir_all(&dir);
            scratch.write("lore/a.md", "a");
            let as_of = super::now();
            let (_, recorded) = list_dir(&dir).unwrap();
            fs::write(dir.join("b.md"), "b").unwrap();
            let fresh = check(&[recorded], as_of, as_of + 100 * SECOND, &[]);
            assert_eq!(fresh, Freshness::Dirty, "round {round}");
        }
    }

    #[test]
    fn a_path_kind_changes_when_the_path_appears_or_changes_type() {
        let scratch = Scratch::new("input-kind");
        let path = scratch.path("thing");
        let (kind, missing) = path_kind(&path);
        assert_eq!(kind, PathKind::Missing);
        assert_eq!(check(std::slice::from_ref(&missing), 0, 0, &[]), Freshness::Clean);

        fs::write(&path, "x").unwrap();
        assert_eq!(check(&[missing], 0, 0, &[]), Freshness::Dirty);
        let (kind, is_file) = path_kind(&path);
        assert_eq!(kind, PathKind::File);
        // Contents are not part of what was observed.
        fs::write(&path, "different").unwrap();
        assert_eq!(check(std::slice::from_ref(&is_file), 0, 0, &[]), Freshness::Clean);

        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert_eq!(check(&[is_file], 0, 0, &[]), Freshness::Dirty);
        assert_eq!(path_kind(&path).0, PathKind::Dir);
    }

    #[test]
    fn a_size_changes_only_when_the_size_does() {
        let scratch = Scratch::new("input-size");
        let path = scratch.write("data", "12345");
        let (size, recorded) = file_size(&path).unwrap();
        assert_eq!(size, 5);
        fs::write(&path, "abcde").unwrap();
        assert_eq!(check(std::slice::from_ref(&recorded), 0, 0, &[]), Freshness::Clean);
        fs::write(&path, "abcdef").unwrap();
        assert_eq!(check(&[recorded], 0, 0, &[]), Freshness::Dirty);
        assert!(file_size(&scratch.path("missing")).is_err());
        assert!(file_size(scratch.root()).is_err());
    }

    #[test]
    fn an_environment_variable_changes_when_its_value_does() {
        let (value, set) = env_var("PATH", &[]);
        assert!(value.is_some());
        assert_eq!(check(std::slice::from_ref(&set), 0, 0, &[]), Freshness::Clean);
        let other = Input::Env { name: "PATH".to_string(), hash: Some(1) };
        assert_eq!(check(&[other], 0, 0, &[]), Freshness::Dirty);
        let gone = Input::Env { name: "PATH".to_string(), hash: None };
        assert_eq!(check(&[gone], 0, 0, &[]), Freshness::Dirty);

        let (value, unset) = env_var("YIO_TEST_SURELY_UNSET_VARIABLE", &[]);
        assert_eq!(value, None);
        assert_eq!(check(&[unset], 0, 0, &[]), Freshness::Clean);
    }
}
