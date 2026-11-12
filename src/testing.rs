//! Helpers shared by tests that need real files.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

pub const SECOND: u64 = 1_000_000_000;
/// An arbitrary modification time, in 2023, as nanoseconds since the epoch.
pub const THEN: u64 = 1_700_000_000 * SECOND;

/// A directory under the system's temporary directory that is removed when
/// the test ends. Names must be unique across tests, which run in parallel.
pub struct Scratch(PathBuf);

impl Scratch {
    pub fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("yio-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    pub fn root(&self) -> &Path {
        &self.0
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    /// Writes a file, creating its directories, and returns its path.
    pub fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.path(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Sets a file's modification time, in nanoseconds since the epoch.
pub fn set_mtime(path: impl AsRef<Path>, mtime: u64) {
    let file = fs::File::options().write(true).open(path).unwrap();
    file.set_modified(UNIX_EPOCH + Duration::from_nanos(mtime)).unwrap();
}
