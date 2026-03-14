// Disk-based write buffer for FUSE file handles.
//
// Temporary files are used instead of in-memory Vec<u8> to avoid OOM
// when writing large files. Each open file handle with write access
// gets its own WriteBuffer backed by a file in the cache directory.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// A disk-backed write buffer for a single file handle.
///
/// Data is written to a temporary file `{cache_dir}/.write_{inode}_{fh}`.
/// On `finalize()`, the temp file is renamed to the final cache path.
/// If dropped without `finalize()`, the temp file is cleaned up.
pub struct WriteBuffer {
    file: File,
    path: PathBuf,
    len: u64,
    finalized: bool,
}

impl WriteBuffer {
    /// Create a new write buffer, optionally pre-filled with existing data.
    pub fn new(
        cache_dir: &Path,
        inode: u64,
        fh: u64,
        initial_data: Option<&[u8]>,
    ) -> io::Result<Self> {
        let path = cache_dir.join(format!(".write_{inode}_{fh}"));
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)?;

        let len = if let Some(data) = initial_data {
            file.write_all(data)?;
            data.len() as u64
        } else {
            0
        };

        Ok(Self {
            file,
            path,
            len,
            finalized: false,
        })
    }

    /// Write data at the given offset.
    pub fn write_at(&mut self, offset: u64, data: &[u8]) -> io::Result<u32> {
        // Extend file if offset is beyond current length
        let end = offset + data.len() as u64;
        if end > self.len {
            self.file.set_len(end)?;
            self.len = end;
        }

        self.file.seek(SeekFrom::Start(offset))?;
        self.file.write_all(data)?;

        Ok(data.len() as u32)
    }

    /// Read data from the buffer (for read-after-write consistency).
    pub fn read_at(&mut self, offset: u64, size: u32) -> io::Result<Vec<u8>> {
        if offset >= self.len {
            return Ok(Vec::new());
        }

        let available = (self.len - offset).min(size as u64) as usize;
        let mut buf = vec![0u8; available];
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read_exact(&mut buf)?;
        Ok(buf)
    }

    /// Current length of the buffered data.
    pub fn len(&self) -> u64 {
        self.len
    }

    /// Truncate or extend the buffer to `new_size`.
    pub fn truncate(&mut self, new_size: u64) -> io::Result<()> {
        self.file.set_len(new_size)?;
        self.len = new_size;
        Ok(())
    }

    /// Move the temp file to the final cache location.
    pub fn finalize(mut self, target: &Path) -> io::Result<()> {
        // Flush any buffered writes before renaming.
        self.file.flush()?;
        std::fs::rename(&self.path, target)?;
        self.finalized = true;
        Ok(())
    }
}

impl Drop for WriteBuffer {
    fn drop(&mut self) {
        if !self.finalized {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_and_read_back() {
        let tmp = tempfile::tempdir().unwrap();
        let mut wb = WriteBuffer::new(tmp.path(), 10, 1, None).unwrap();

        wb.write_at(0, b"hello").unwrap();
        assert_eq!(wb.len(), 5);

        let data = wb.read_at(0, 10).unwrap();
        assert_eq!(data, b"hello");
    }

    #[test]
    fn write_at_offset() {
        let tmp = tempfile::tempdir().unwrap();
        let mut wb = WriteBuffer::new(tmp.path(), 10, 1, None).unwrap();

        wb.write_at(0, b"hello").unwrap();
        wb.write_at(5, b" world").unwrap();
        assert_eq!(wb.len(), 11);

        let data = wb.read_at(0, 20).unwrap();
        assert_eq!(data, b"hello world");
    }

    #[test]
    fn write_with_initial_data() {
        let tmp = tempfile::tempdir().unwrap();
        let mut wb = WriteBuffer::new(tmp.path(), 10, 1, Some(b"existing")).unwrap();

        assert_eq!(wb.len(), 8);
        let data = wb.read_at(0, 20).unwrap();
        assert_eq!(data, b"existing");

        wb.write_at(8, b" more").unwrap();
        let data = wb.read_at(0, 20).unwrap();
        assert_eq!(data, b"existing more");
    }

    #[test]
    fn truncate_shrinks() {
        let tmp = tempfile::tempdir().unwrap();
        let mut wb = WriteBuffer::new(tmp.path(), 10, 1, Some(b"hello world")).unwrap();

        wb.truncate(5).unwrap();
        assert_eq!(wb.len(), 5);

        let data = wb.read_at(0, 10).unwrap();
        assert_eq!(data, b"hello");
    }

    #[test]
    fn truncate_extends() {
        let tmp = tempfile::tempdir().unwrap();
        let mut wb = WriteBuffer::new(tmp.path(), 10, 1, Some(b"hi")).unwrap();

        wb.truncate(10).unwrap();
        assert_eq!(wb.len(), 10);
    }

    #[test]
    fn finalize_moves_file() {
        let tmp = tempfile::tempdir().unwrap();
        let mut wb = WriteBuffer::new(tmp.path(), 10, 1, None).unwrap();
        wb.write_at(0, b"data").unwrap();

        let target = tmp.path().join("10");
        let temp_path = wb.path.clone();
        wb.finalize(&target).unwrap();

        assert!(target.exists());
        assert!(!temp_path.exists());
    }

    #[test]
    fn drop_cleans_up() {
        let tmp = tempfile::tempdir().unwrap();
        let temp_path;
        {
            let wb = WriteBuffer::new(tmp.path(), 10, 1, None).unwrap();
            temp_path = wb.path.clone();
            assert!(temp_path.exists());
        }
        assert!(!temp_path.exists());
    }

    #[test]
    fn read_beyond_eof() {
        let tmp = tempfile::tempdir().unwrap();
        let mut wb = WriteBuffer::new(tmp.path(), 10, 1, Some(b"hi")).unwrap();

        let data = wb.read_at(100, 10).unwrap();
        assert!(data.is_empty());
    }
}
