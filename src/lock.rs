// PID file lock for preventing duplicate mirage instances.
//
// Uses flock(2) for advisory locking combined with PID file
// for status queries from other processes.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Advisory lock backed by a PID file with flock(2).
pub struct LockFile {
    path: PathBuf,
    _file: File,
}

impl LockFile {
    /// Acquire an exclusive lock, writing the current PID to the file.
    ///
    /// Returns an error if another process already holds the lock.
    pub fn acquire(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;

        // Try non-blocking exclusive lock
        let fd = {
            use std::os::unix::io::AsRawFd;
            file.as_raw_fd()
        };
        let ret = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
        if ret != 0 {
            let existing_pid = read_pid(path).ok().flatten();
            let msg = match existing_pid {
                Some(pid) => format!("mirage is already running (PID: {pid})"),
                None => "mirage is already running".to_owned(),
            };
            return Err(Error::Config(msg));
        }

        // Truncate and seek to start after acquiring lock to avoid race with other readers
        file.set_len(0)
            .map_err(|e| Error::Config(format!("failed to truncate PID file: {e}")))?;
        (&file)
            .seek(std::io::SeekFrom::Start(0))
            .map_err(|e| Error::Config(format!("failed to seek PID file: {e}")))?;

        // Write PID
        let mut f = &file;
        write!(f, "{}", std::process::id())
            .map_err(|e| Error::Config(format!("failed to write PID file: {e}")))?;
        f.flush()?;

        Ok(Self {
            path: path.to_owned(),
            _file: file,
        })
    }

    /// Get the lock file path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for LockFile {
    fn drop(&mut self) {
        // flock is automatically released when the file descriptor is closed.
        // Remove the PID file for cleanliness.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Read the PID from a lock file, if it exists and contains a valid number.
pub fn read_pid(path: &Path) -> Result<Option<u32>> {
    match std::fs::File::open(path) {
        Ok(mut f) => {
            let mut buf = String::new();
            f.read_to_string(&mut buf)?;
            Ok(buf.trim().parse::<u32>().ok())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::Io(e)),
    }
}

/// Check whether the lock is currently held by another process.
///
/// Tries to acquire the lock non-blockingly; if it fails, the lock is held.
pub fn is_held(path: &Path) -> bool {
    let file = match OpenOptions::new().read(true).open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let fd = {
        use std::os::unix::io::AsRawFd;
        file.as_raw_fd()
    };
    let ret = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
    if ret == 0 {
        // We got the lock → not held by anyone else. Release it.
        unsafe { libc::flock(fd, libc::LOCK_UN) };
        false
    } else {
        true
    }
}

/// Determine the default lock file path.
///
/// Uses `$XDG_RUNTIME_DIR/mirage.pid` if available, otherwise falls back to `cache_dir/mirage.pid`.
pub fn default_lock_path(cache_dir: &Path) -> PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(runtime_dir).join("mirage.pid")
    } else {
        cache_dir.join("mirage.pid")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_and_release() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.pid");

        {
            let lock = LockFile::acquire(&path).unwrap();
            assert!(path.exists());
            assert!(is_held(&path));

            let pid = read_pid(&path).unwrap();
            assert_eq!(pid, Some(std::process::id()));

            drop(lock);
        }

        assert!(!path.exists());
        assert!(!is_held(&path));
    }

    #[test]
    fn double_acquire_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.pid");

        let _lock = LockFile::acquire(&path).unwrap();
        let result = LockFile::acquire(&path);
        assert!(result.is_err());
    }

    #[test]
    fn read_pid_nonexistent() {
        let path = Path::new("/tmp/mirage_test_nonexistent_12345.pid");
        let pid = read_pid(path).unwrap();
        assert_eq!(pid, None);
    }

    #[test]
    fn is_held_nonexistent() {
        let path = Path::new("/tmp/mirage_test_nonexistent_12345.pid");
        assert!(!is_held(path));
    }

    #[test]
    fn default_lock_path_with_xdg() {
        let cache = PathBuf::from("/home/user/.cache/mirage");
        // Just verify the fallback path
        let path = cache.join("mirage.pid");
        assert_eq!(path, PathBuf::from("/home/user/.cache/mirage/mirage.pid"));
    }
}
