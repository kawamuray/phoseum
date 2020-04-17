use failure::Fail;
use log::{debug, info, warn};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::{fs, io};

#[derive(Fail, Debug)]
pub enum Error {
    #[fail(display = "Error in I/O with disks: {}", _0)]
    IO(#[fail(cause)] io::Error),
    #[fail(display = "Invalid path: {:?}: {}", path, reason)]
    InvalidPath { path: PathBuf, reason: &'static str },
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::IO(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, PartialEq)]
struct Entry {
    size: u64,
    users: usize,
}

impl Entry {
    fn new(size: u64) -> Self {
        Self { size, users: 0 }
    }
}

/// Representation of local filesystem directory to cache media files.
///
/// This storage offers management of media files with:
/// * Filesystem size usage limit
/// * Transparent eviction of files which are not under use
pub struct Storage {
    dir: PathBuf,
    capacity: u64,
    using: u64,
    residents: HashMap<PathBuf, Entry>,
}

impl Storage {
    pub fn open<P: Into<PathBuf>>(path: P, capacity: u64) -> Result<Self> {
        let path = path.into();
        let residents = Self::scan_residents(&path)?;
        let using = residents.values().map(|e| e.size).sum();
        info!(
            "Finish loading storage {}, {} entries using {} bytes",
            path.display(),
            residents.len(),
            using
        );
        Ok(Storage {
            dir: path,
            capacity,
            using,
            residents,
        })
    }

    fn scan_residents(path: &Path) -> io::Result<HashMap<PathBuf, Entry>> {
        let mut residents = HashMap::new();
        for dentry in fs::read_dir(path)? {
            let dentry = dentry?;
            let meta = dentry.metadata()?;
            let filename: PathBuf = dentry.path().file_name().expect("filename").into();
            if !meta.file_type().is_file() {
                warn!(
                    "Skipping an entry in storage which is not a file: {}",
                    filename.display()
                );
                continue;
            }

            let size = meta.len();
            debug!("Found resident {} of {} bytes", filename.display(), size);
            residents.insert(filename, Entry::new(size));
        }
        Ok(residents)
    }

    /// Acquire the size specified in local storage.
    ///
    /// Acquisition may fails when free capacity cannot contain
    /// attempted size.
    /// Returns the list containing path to acquired file for those succeeds,
    /// None on failure.
    pub fn acquire(&mut self, path: &Path, size: u64, reserved: &HashSet<&Path>) -> Result<bool> {
        let filename = Self::valid_filename(path)?;

        if let Some(entry) = self.residents.get_mut(&filename) {
            debug!("Adding users of {}/{}", filename.display(), entry.users);
            entry.users += 1;
            return Ok(true);
        }

        if self.using + size > self.capacity {
            let evicted = self.try_evict(size, &reserved)?;
            if !evicted {
                return Ok(false);
            }
        }

        debug!(
            "Acquire {} bytes for {}, using = {}",
            size,
            filename.display(),
            self.using
        );
        let entry = self
            .residents
            .entry(filename)
            .or_insert_with(|| Entry::new(size));
        entry.size = size;
        entry.users += 1;

        self.using += size;
        Ok(true)
    }

    /// Release the size specified from local storage.
    ///
    /// This release locked size in capacity so that new acquisition can
    /// take it.
    /// Actual files are not removed by this operation. They might be removed
    /// when acquire needs to free up space to let acquisition take it.
    pub fn release(&mut self, path: &Path) -> Result<()> {
        let entry = self
            .residents
            .get_mut(path)
            .ok_or_else(|| Error::InvalidPath {
                path: path.to_path_buf(),
                reason: "doesn't exists",
            })?;
        if entry.users > 0 {
            entry.users -= 1;
        }
        debug!("Release {}", path.display());
        Ok(())
    }

    fn try_evict(&mut self, acquire_size: u64, reserved: &HashSet<&Path>) -> io::Result<bool> {
        let mut sizes: Vec<_> = self.residents.iter().collect();
        // It may end up wasting network bandwidth to evict 300MB file
        // to acquire 1MB file. Prefer to evict smaller files as possible.
        sizes.sort_unstable_by_key(|(_, entry)| entry.size);

        let mut evicted = Vec::new();
        let mut freed = 0;
        for (path, entry) in sizes {
            if entry.users > 0 || reserved.contains(&path.as_ref()) {
                continue;
            }

            freed += entry.size;
            evicted.push(path);

            if freed >= acquire_size {
                break;
            }
        }
        if freed < acquire_size {
            return Ok(false);
        }

        let keys_to_remove: Vec<_> = evicted.into_iter().cloned().collect();
        for path in keys_to_remove {
            fs::remove_file(self.filepath(&path).expect("filepath"))?;
            let entry = self.residents.remove(&path).unwrap();
            self.using -= entry.size;
            debug!(
                "Evict file {} to free {} bytes, using = {}",
                path.display(),
                entry.size,
                self.using
            )
        }

        Ok(true)
    }

    pub fn filepath<P: AsRef<Path>>(&self, filename: P) -> Result<PathBuf> {
        let filename = Self::valid_filename(filename.as_ref())?;
        Ok(self.dir.join(filename))
    }

    fn valid_filename(path: &Path) -> Result<PathBuf> {
        if path.is_absolute()
            || path
                .parent()
                .and_then(Path::to_str)
                .map(|p| p != "")
                .unwrap_or(false)
        {
            return Err(Error::InvalidPath {
                path: path.to_path_buf(),
                reason: "must contain only filename",
            });
        }

        Ok(path
            .file_name()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| Error::InvalidPath {
                path: path.to_path_buf(),
                reason: "empty filename",
            })?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::fs::File;
    use std::path::PathBuf;
    use tempfile;

    fn new_storage(cap: u64) -> (Storage, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (Storage::open(dir.path(), cap).unwrap(), dir)
    }

    fn create_file(dir: &Path, name: &str, size: u64) {
        let path = dir.join(name);
        let file = File::create(path).expect("create file");
        file.set_len(size).expect("set file size");
    }

    fn file_exists(dir: &Path, name: &str) -> bool {
        dir.join(name).exists()
    }

    #[test]
    fn test_acquire() {
        let (mut storage, dir) = new_storage(20);

        let reserved = HashSet::new();
        assert!(storage.acquire(&PathBuf::from("a"), 10, &reserved).unwrap());
        create_file(dir.path(), "a", 10);
        assert!(storage.acquire(&PathBuf::from("b"), 5, &reserved).unwrap());
        create_file(dir.path(), "b", 5);
        assert!(!storage.acquire(&PathBuf::from("c"), 6, &reserved).unwrap());
        assert!(file_exists(dir.path(), "a"));
        assert!(file_exists(dir.path(), "b"));
    }

    #[test]
    fn test_acquire_with_eviction() {
        let (mut storage, dir) = new_storage(20);

        let reserved = HashSet::new();
        assert!(storage.acquire(&PathBuf::from("a"), 10, &reserved).unwrap());
        create_file(dir.path(), "a", 10);
        assert!(storage.acquire(&PathBuf::from("b"), 5, &reserved).unwrap());
        create_file(dir.path(), "b", 5);

        storage.release(&PathBuf::from("a")).unwrap();
        // Even after release, file remains until it evicted on next acquire
        assert!(file_exists(dir.path(), "a"));
        assert!(storage.acquire(&PathBuf::from("c"), 6, &reserved).unwrap());
        assert!(!file_exists(dir.path(), "a"));
        assert!(file_exists(dir.path(), "b"));
    }

    #[test]
    fn test_acquire_multi_users() {
        let (mut storage, dir) = new_storage(20);

        let reserved = HashSet::new();
        assert!(storage.acquire(&PathBuf::from("a"), 20, &reserved).unwrap());
        create_file(dir.path(), "a", 10);
        // Acquire twice
        assert!(storage.acquire(&PathBuf::from("a"), 20, &reserved).unwrap());
        // Release once
        storage.release(&PathBuf::from("a")).unwrap();

        // New acquire should fail by lack of the space
        assert!(!storage.acquire(&PathBuf::from("b"), 5, &reserved).unwrap());
        // File should remain undeleted
        assert!(file_exists(dir.path(), "a"));
    }

    #[test]
    fn test_acquire_with_reserved() {
        let (mut storage, dir) = new_storage(20);

        let mut reserved = HashSet::new();
        assert!(storage.acquire(&PathBuf::from("a"), 10, &reserved).unwrap());
        create_file(dir.path(), "a", 10);
        assert!(storage.acquire(&PathBuf::from("b"), 10, &reserved).unwrap());
        create_file(dir.path(), "b", 10);
        storage.release(&PathBuf::from("a")).unwrap();
        storage.release(&PathBuf::from("b")).unwrap();

        let path_a = PathBuf::from("a");
        reserved.insert(&path_a);

        // Even after release, file remains until it evicted on next acquire
        assert!(storage.acquire(&PathBuf::from("c"), 10, &reserved).unwrap());
        create_file(dir.path(), "c", 10);
        // b should be deleted over a because it's not reserved
        assert!(file_exists(dir.path(), "a"));
        assert!(!file_exists(dir.path(), "b"));

        // Another acquire fail by reserved a
        assert!(!storage.acquire(&PathBuf::from("d"), 10, &reserved).unwrap());
    }

    #[test]
    fn test_filepath() {
        let (storage, dir) = new_storage(20);

        assert_eq!(dir.path().join("a"), storage.filepath("a").unwrap());
        assert_eq!(dir.path().join("b"), storage.filepath("b").unwrap());
    }

    #[test]
    fn test_valid_filename() {
        assert!(Storage::valid_filename(&PathBuf::from("a")).is_ok());
        assert!(Storage::valid_filename(&PathBuf::from("../a")).is_err());
        assert!(Storage::valid_filename(&PathBuf::from("/ab")).is_err());
        assert!(Storage::valid_filename(&PathBuf::from("/a/bb")).is_err());
        assert!(Storage::valid_filename(&PathBuf::from("./")).is_err());
    }

    #[test]
    fn test_scan_residents() {
        let dir = tempfile::tempdir().unwrap();

        create_file(dir.path(), "a", 10);
        create_file(dir.path(), "b", 5);
        // Result must not contain non-regular file entry
        fs::create_dir(dir.path().join("c")).unwrap();

        let mut expected = HashMap::new();
        expected.insert(PathBuf::from("a"), Entry::new(10));
        expected.insert(PathBuf::from("b"), Entry::new(5));

        let residents = Storage::scan_residents(dir.path()).unwrap();
        assert_eq!(expected, residents);
    }

    #[test]
    fn test_acquire_after_init() {
        let dir = tempfile::tempdir().unwrap();

        create_file(dir.path(), "a", 10);
        create_file(dir.path(), "b", 20);

        let reserved = HashSet::new();
        let mut storage = Storage::open(dir.path(), 20).unwrap();
        assert!(storage.acquire(&PathBuf::from("c"), 5, &reserved).unwrap());
        assert!(!file_exists(dir.path(), "a"));
        assert!(file_exists(dir.path(), "b"));
    }
}
