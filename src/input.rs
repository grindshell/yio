//! What a cached result was computed from, and whether that still holds.
//!
//! Anything that looks at the outside world on behalf of a cached result
//! does it through this module, which hands back both what it saw and an
//! [`Input`] describing it. Checking the inputs later says whether the
//! result can be reused. See "Dirtiness" in `docs/cache.md`.

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
    /// The command-line arguments a plugin was told.
    Args { hash: u64 },
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
pub fn spell(path: &Path) -> String {
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

/// What a plugin is told about the run it is part of.
#[derive(Debug, Clone, Copy, Default)]
pub struct Vars<'a> {
    /// Variables that are looked up ahead of the real environment: the
    /// `YIO_*` values of the current run.
    pub env: &'a [(&'a str, String)],
    /// The command-line arguments that are for the plugin's `yio.md`.
    pub args: &'a [String],
}

impl Vars<'_> {
    /// Nothing told: the real environment and no arguments.
    #[cfg(test)]
    pub const NONE: Vars<'static> = Vars { env: &[], args: &[] };
}

/// Reads an environment variable, preferring `vars`.
pub fn env_var(name: &str, vars: Vars) -> (Option<String>, Input) {
    let value = match vars.env.iter().find(|(known, _)| *known == name) {
        Some((_, value)) => Some(value.clone()),
        None => std::env::var_os(name).map(|value| value.to_string_lossy().into_owned()),
    };
    let hash = value.as_ref().map(|value| hash(value.as_bytes()));
    (value, Input::Env { name: name.to_string(), hash })
}

/// The command-line arguments in `vars`.
pub fn args<'a>(vars: Vars<'a>) -> (&'a [String], Input) {
    (vars.args, Input::Args { hash: args_hash(vars.args) })
}

fn args_hash(args: &[String]) -> u64 {
    // No argument contains a NUL, so ending each with one keeps `a b` and
    // `ab` apart, and an empty argument from none at all.
    let mut bytes = Vec::new();
    for arg in args {
        bytes.extend_from_slice(arg.as_bytes());
        bytes.push(0);
    }
    hash(&bytes)
}

// Changes. Each of these alters the outside world and hands back an input
// describing the state it left behind. A result that made a change is
// reusable only while that state still holds: if a file it wrote is edited
// or deleted, the result is out of date and making it again puts the file
// back, the way a build tool treats its targets.

fn make_parent(path: &Path) -> io::Result<()> {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => fs::create_dir_all(parent),
        _ => Ok(()),
    }
}

/// Makes a file hold `contents`, creating the directories above it. A file
/// that already holds exactly that is left untouched, so its modification
/// time does not move and nothing that depends on it is disturbed.
pub fn write_file(path: &Path, contents: &[u8]) -> io::Result<Input> {
    let same_size = fs::metadata(path).is_ok_and(|found| found.len() == contents.len() as u64);
    if !(same_size && fs::read(path).is_ok_and(|current| current == contents)) {
        make_parent(path)?;
        fs::write(path, contents)?;
    }
    let metadata = fs::metadata(path)?;
    Ok(Input::File {
        path: spell(path),
        size: contents.len() as u64,
        mtime: modified(&metadata),
        hash: hash(contents),
    })
}

/// Makes a directory and any missing directories above it.
pub fn make_dir(path: &Path) -> io::Result<Input> {
    fs::create_dir_all(path)?;
    Ok(Input::Kind { path: spell(path), kind: PathKind::Dir })
}

/// Removes a file or an empty directory. A path that is already gone is
/// fine. A directory with something in it is an error: nothing is removed
/// recursively.
pub fn remove(path: &Path) -> io::Result<Input> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
        Ok(found) if found.is_dir() => fs::remove_dir(path)?,
        Ok(_) => fs::remove_file(path)?,
    }
    Ok(Input::Kind { path: spell(path), kind: PathKind::Missing })
}

/// Copies a file. Gives the inputs for the source and for the copy.
pub fn copy_file(from: &Path, to: &Path) -> io::Result<(Input, Input)> {
    let (contents, source) = read_file(from)?;
    Ok((source, write_file(to, &contents)?))
}

/// Moves a file or directory, creating the directories above its new place.
/// Gives the inputs for where it was and where it is now.
pub fn move_path(from: &Path, to: &Path) -> io::Result<(Input, Input)> {
    make_parent(to)?;
    fs::rename(from, to)?;
    Ok((Input::Kind { path: spell(from), kind: PathKind::Missing }, path_kind(to).1))
}

/// Drops inputs that another input already covers, so that checking them
/// later costs less. A file whose contents were read cannot change size or
/// stop being a file without its contents changing, so a `Size` or `Kind`
/// input for the same path adds nothing.
pub fn prune(inputs: &mut Vec<Input>) {
    let read: Vec<String> = inputs
        .iter()
        .filter_map(|input| match input {
            Input::File { path, .. } => Some(path.clone()),
            _ => None,
        })
        .collect();
    inputs.retain(|input| match input {
        Input::Size { path, .. } | Input::Kind { path, kind: PathKind::File } => {
            !read.contains(path)
        }
        _ => true,
    });
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
    for (input, verdict) in inputs.iter().zip(verdicts(inputs, as_of, vars)) {
        match verdict {
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

/// Below this many inputs, checking them one after another is quicker than
/// starting threads to share the work.
const PARALLEL_FROM: usize = 64;

/// Judges inputs, in order, stopping at the first that changed. The list
/// may therefore be shorter than `inputs`, in which case it ends in `Dirty`.
///
/// Almost all of the time goes to one `stat` per input, so a long list is
/// split across threads.
fn verdicts(inputs: &[Input], as_of: u64, vars: Vars) -> Vec<Verdict> {
    let judge = |part: &[Input]| {
        let mut verdicts = Vec::with_capacity(part.len());
        for input in part {
            let verdict = verdict(input, as_of, vars);
            let dirty = matches!(verdict, Verdict::Dirty);
            verdicts.push(verdict);
            if dirty {
                break;
            }
        }
        verdicts
    };

    let threads = std::thread::available_parallelism().map_or(1, |count| count.get()).min(8);
    if inputs.len() < PARALLEL_FROM || threads < 2 {
        return judge(inputs);
    }
    let share = inputs.len().div_ceil(threads);
    std::thread::scope(|scope| {
        let parts: Vec<_> = inputs.chunks(share).map(|part| scope.spawn(move || judge(part))).collect();
        let mut all = Vec::with_capacity(inputs.len());
        for part in parts {
            let verdicts = part.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic));
            let dirty = matches!(verdicts.last(), Some(Verdict::Dirty));
            all.extend(verdicts);
            if dirty {
                break;
            }
        }
        all
    })
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
        Input::Args { hash: expected } => clean_if(args_hash(vars.args) == *expected),
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
        assert_eq!(check(&[recorded], as_of, as_of + SECOND, Vars::NONE), Freshness::Clean);
    }

    #[test]
    fn a_changed_file_is_dirty() {
        let scratch = Scratch::new("input-changed");
        let (path, recorded) = file(&scratch, "yio.md", "one", THEN);
        let as_of = THEN + 10 * SECOND;
        let now = as_of + 100 * SECOND;

        fs::write(&path, "longer").unwrap();
        assert_eq!(check(std::slice::from_ref(&recorded), as_of, now, Vars::NONE), Freshness::Dirty);

        // Same size, later time: only the contents show the change.
        fs::write(&path, "two").unwrap();
        set_mtime(&path, THEN + 50 * SECOND);
        assert_eq!(check(std::slice::from_ref(&recorded), as_of, now, Vars::NONE), Freshness::Dirty);

        fs::remove_file(&path).unwrap();
        assert_eq!(check(&[recorded], as_of, now, Vars::NONE), Freshness::Dirty);
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
            check(std::slice::from_ref(&recorded), as_of, touched + 10 * SECOND, Vars::NONE),
            Freshness::Settled(vec![settled])
        );
        // Too recent: clean, but recording it would not help yet.
        assert_eq!(check(&[recorded], as_of, touched + SECOND, Vars::NONE), Freshness::Clean);
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
            check(std::slice::from_ref(&recorded), as_of, later, Vars::NONE),
            Freshness::Settled(vec![recorded.clone()])
        );

        fs::write(&path, "two").unwrap();
        set_mtime(&path, THEN);
        assert_eq!(check(&[recorded], as_of, later, Vars::NONE), Freshness::Dirty);
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
        assert_ne!(check(std::slice::from_ref(&recorded), as_of, now, Vars::NONE), Freshness::Dirty);

        // Editing a file in the directory does not change the listing.
        fs::write(dir.join("a.md"), "edited").unwrap();
        assert_ne!(check(std::slice::from_ref(&recorded), as_of, now, Vars::NONE), Freshness::Dirty);

        fs::write(dir.join("c.md"), "c").unwrap();
        assert_eq!(check(std::slice::from_ref(&recorded), as_of, now, Vars::NONE), Freshness::Dirty);

        // Back to the same names: unchanged, though it took a re-listing.
        fs::remove_file(dir.join("c.md")).unwrap();
        assert_ne!(check(std::slice::from_ref(&recorded), as_of, now, Vars::NONE), Freshness::Dirty);

        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(check(&[recorded], as_of, now, Vars::NONE), Freshness::Dirty);
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
        assert_eq!(check(&[recorded], as_of, as_of + SECOND, Vars::NONE), Freshness::Clean);
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
            let fresh = check(&[recorded], as_of, as_of + 100 * SECOND, Vars::NONE);
            assert_eq!(fresh, Freshness::Dirty, "round {round}");
        }
    }

    #[test]
    fn writing_a_file_records_what_it_left_behind() {
        let scratch = Scratch::new("input-write");
        let path = scratch.path("out/deep/index.md");
        let as_of = now();
        let later = as_of + 100 * SECOND;
        let written = write_file(&path, b"one").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"one");
        let fresh = || check(std::slice::from_ref(&written), as_of, later, Vars::NONE);
        assert_ne!(fresh(), Freshness::Dirty);

        // Edited by hand, or deleted: what was made is no longer there.
        fs::write(&path, "two").unwrap();
        assert_eq!(fresh(), Freshness::Dirty);
        fs::remove_file(&path).unwrap();
        assert_eq!(fresh(), Freshness::Dirty);
    }

    #[test]
    fn writing_the_same_contents_leaves_the_file_alone() {
        let scratch = Scratch::new("input-rewrite");
        let path = scratch.write("index.md", "same");
        set_mtime(&path, THEN);
        let Input::File { mtime, .. } = write_file(&path, b"same").unwrap() else { unreachable!() };
        assert_eq!(mtime, THEN);
        // Different contents of the same length are still written.
        let Input::File { mtime, .. } = write_file(&path, b"diff").unwrap() else { unreachable!() };
        assert_ne!(mtime, THEN);
        assert_eq!(fs::read(&path).unwrap(), b"diff");
    }

    #[test]
    fn making_and_removing_record_the_state_they_leave() {
        let scratch = Scratch::new("input-make-remove");
        let dir = scratch.path("a/b");
        let made = make_dir(&dir).unwrap();
        assert_eq!(made, make_dir(&dir).unwrap());
        assert_eq!(check(std::slice::from_ref(&made), 0, 0, Vars::NONE), Freshness::Clean);

        let file = scratch.write("a/b/file", "x");
        // A directory with something in it is not removed.
        assert!(remove(&dir).is_err());
        let gone = remove(&file).unwrap();
        assert!(!file.exists());
        assert_eq!(check(std::slice::from_ref(&gone), 0, 0, Vars::NONE), Freshness::Clean);
        // Removing what is already gone is fine.
        assert_eq!(remove(&file).unwrap(), gone);

        // An empty directory can be removed, and then making it no longer
        // holds.
        remove(&dir).unwrap();
        assert!(!dir.exists());
        assert_eq!(check(&[made], 0, 0, Vars::NONE), Freshness::Dirty);

        // If what was removed comes back, the removal no longer holds.
        scratch.write("a/b/file", "back");
        assert_eq!(check(&[gone], 0, 0, Vars::NONE), Freshness::Dirty);
    }

    #[test]
    fn copying_and_moving_record_both_ends() {
        let scratch = Scratch::new("input-copy-move");
        let from = scratch.write("from.txt", "payload");
        let to = scratch.path("made/to.txt");

        let (source, copy) = copy_file(&from, &to).unwrap();
        assert_eq!(fs::read(&to).unwrap(), b"payload");
        assert!(matches!(source, Input::File { size: 7, .. }));
        assert!(matches!(copy, Input::File { size: 7, .. }));
        assert!(copy_file(&scratch.path("missing"), &to).is_err());

        let moved = scratch.path("moved/there.txt");
        let (left, arrived) = move_path(&to, &moved).unwrap();
        assert!(!to.exists());
        assert_eq!(fs::read(&moved).unwrap(), b"payload");
        assert!(matches!(left, Input::Kind { kind: PathKind::Missing, .. }));
        assert!(matches!(arrived, Input::Kind { kind: PathKind::File, .. }));
        assert_eq!(check(&[left, arrived], 0, 0, Vars::NONE), Freshness::Clean);
        // There is nothing left to move a second time.
        assert!(move_path(&to, &moved).is_err());
    }

    #[test]
    fn a_long_list_of_inputs_is_judged_the_same_as_a_short_one() {
        // Long enough to be split across threads.
        let scratch = Scratch::new("input-many");
        let count = PARALLEL_FROM * 3 + 5;
        let mut recorded = Vec::new();
        for i in 0..count {
            recorded.push(file(&scratch, &format!("f{i:04}.txt"), &format!("contents {i}"), THEN).1);
        }
        let as_of = THEN + 10 * SECOND;
        let now = as_of + 100 * SECOND;
        assert_eq!(check(&recorded, as_of, now, Vars::NONE), Freshness::Clean);

        // One touched file, wherever it is in the list, is noticed and the
        // refreshed list keeps the original order.
        for touched in [0, count / 2, count - 1] {
            let path = scratch.path(&format!("f{touched:04}.txt"));
            set_mtime(&path, THEN + 50 * SECOND);
            let Freshness::Settled(current) = check(&recorded, as_of, now, Vars::NONE) else {
                panic!("expected the touched file {touched} to settle");
            };
            assert_eq!(current.len(), count);
            for (i, (before, after)) in recorded.iter().zip(&current).enumerate() {
                assert_eq!(before == after, i != touched, "input {i}");
            }
            set_mtime(&path, THEN);
        }

        // One changed file anywhere makes the whole list dirty.
        for changed in [0, count / 2, count - 1] {
            let path = scratch.path(&format!("f{changed:04}.txt"));
            let original = fs::read(&path).unwrap();
            fs::write(&path, "something else entirely").unwrap();
            assert_eq!(check(&recorded, as_of, now, Vars::NONE), Freshness::Dirty, "file {changed}");
            fs::write(&path, original).unwrap();
            set_mtime(&path, THEN);
        }
        assert_eq!(check(&recorded, as_of, now, Vars::NONE), Freshness::Clean);
    }

    #[test]
    fn pruning_drops_what_a_read_file_already_covers() {
        let file = |path: &str| Input::File { path: path.to_string(), size: 1, mtime: 2, hash: 3 };
        let size = |path: &str| Input::Size { path: path.to_string(), size: 1 };
        let kind = |path: &str, kind| Input::Kind { path: path.to_string(), kind };
        let dir = Input::Dir { path: "lore".to_string(), mtime: 1, hash: 2 };
        let env = Input::Env { name: "a.md".to_string(), hash: None };
        let mut inputs = vec![
            dir.clone(),
            size("a.md"),
            kind("a.md", PathKind::File),
            size("b.md"),
            kind("b.md", PathKind::File),
            kind("a.md", PathKind::Dir),
            env.clone(),
            file("a.md"),
        ];
        prune(&mut inputs);
        assert_eq!(
            inputs,
            [
                dir,
                size("b.md"),
                kind("b.md", PathKind::File),
                // Kept: this one says the path was *not* a file at some
                // point, which reading it later does not cover.
                kind("a.md", PathKind::Dir),
                env,
                file("a.md"),
            ]
        );
    }

    #[test]
    fn a_path_kind_changes_when_the_path_appears_or_changes_type() {
        let scratch = Scratch::new("input-kind");
        let path = scratch.path("thing");
        let (kind, missing) = path_kind(&path);
        assert_eq!(kind, PathKind::Missing);
        assert_eq!(check(std::slice::from_ref(&missing), 0, 0, Vars::NONE), Freshness::Clean);

        fs::write(&path, "x").unwrap();
        assert_eq!(check(&[missing], 0, 0, Vars::NONE), Freshness::Dirty);
        let (kind, is_file) = path_kind(&path);
        assert_eq!(kind, PathKind::File);
        // Contents are not part of what was observed.
        fs::write(&path, "different").unwrap();
        assert_eq!(check(std::slice::from_ref(&is_file), 0, 0, Vars::NONE), Freshness::Clean);

        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert_eq!(check(&[is_file], 0, 0, Vars::NONE), Freshness::Dirty);
        assert_eq!(path_kind(&path).0, PathKind::Dir);
    }

    #[test]
    fn a_size_changes_only_when_the_size_does() {
        let scratch = Scratch::new("input-size");
        let path = scratch.write("data", "12345");
        let (size, recorded) = file_size(&path).unwrap();
        assert_eq!(size, 5);
        fs::write(&path, "abcde").unwrap();
        assert_eq!(check(std::slice::from_ref(&recorded), 0, 0, Vars::NONE), Freshness::Clean);
        fs::write(&path, "abcdef").unwrap();
        assert_eq!(check(&[recorded], 0, 0, Vars::NONE), Freshness::Dirty);
        assert!(file_size(&scratch.path("missing")).is_err());
        assert!(file_size(scratch.root()).is_err());
    }

    #[test]
    fn an_environment_variable_changes_when_its_value_does() {
        let (value, set) = env_var("PATH", Vars::NONE);
        assert!(value.is_some());
        assert_eq!(check(std::slice::from_ref(&set), 0, 0, Vars::NONE), Freshness::Clean);
        let other = Input::Env { name: "PATH".to_string(), hash: Some(1) };
        assert_eq!(check(&[other], 0, 0, Vars::NONE), Freshness::Dirty);
        let gone = Input::Env { name: "PATH".to_string(), hash: None };
        assert_eq!(check(&[gone], 0, 0, Vars::NONE), Freshness::Dirty);

        let (value, unset) = env_var("YIO_TEST_SURELY_UNSET_VARIABLE", Vars::NONE);
        assert_eq!(value, None);
        assert_eq!(check(&[unset], 0, 0, Vars::NONE), Freshness::Clean);
    }
}
