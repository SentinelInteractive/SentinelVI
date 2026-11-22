//! Byte sources: memory-mapped windows for the hot path, positioned reads for
//! archives too large (or too risky) to map.
//!
//! Both variants are `Sync`, so a `&Source` can be shared with a rayon pool.

use std::borrow::Cow;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(windows)]
use std::os::windows::fs::FileExt as _;

/// Random-access, thread-safe view of a file.
pub trait Source: Sync {
    fn len(&self) -> u64;
    fn path(&self) -> &Path;

    /// Read exactly `len` bytes at `off`, zero-padded if the file ends early.
    fn read_at(&self, off: u64, len: usize) -> io::Result<Vec<u8>>;

    /// Borrow when the source is mapped, copy when it is not.
    fn window(&self, off: u64, len: usize) -> io::Result<Cow<'_, [u8]>>;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Whole-file mmap. Zero-copy window reads cost one bounds check.
pub struct MmapSource {
    path: PathBuf,
    map: memmap2::Mmap,
}

impl MmapSource {
    /// Map the file, refusing sizes above `max_len` so a 76 GB archive falls
    /// back to [`StreamSource`] instead of reserving 76 GB of address space.
    pub fn open(path: impl AsRef<Path>, max_len: u64) -> io::Result<Box<dyn Source>> {
        let path = path.as_ref().to_path_buf();
        let file = File::open(&path)?;
        let len = file.metadata()?.len();
        if len > max_len {
            return StreamSource::open(path).map(|s| Box::new(s) as Box<dyn Source>);
        }
        // SAFETY: the mapping is read-only and the file handle outlives it.
        let map = unsafe { memmap2::Mmap::map(&file)? };
        Ok(Box::new(MmapSource { path, map }))
    }
}

impl Source for MmapSource {
    #[inline]
    fn len(&self) -> u64 {
        self.map.len() as u64
    }

    #[inline]
    fn path(&self) -> &Path {
        &self.path
    }

    fn read_at(&self, off: u64, len: usize) -> io::Result<Vec<u8>> {
        let start = (off as usize).min(self.map.len());
        let end = start.saturating_add(len).min(self.map.len());
        let mut out = vec![0u8; len];
        out[..end - start].copy_from_slice(&self.map[start..end]);
        Ok(out)
    }

    fn window(&self, off: u64, len: usize) -> io::Result<Cow<'_, [u8]>> {
        let start = (off as usize).min(self.map.len());
        let end = start.saturating_add(len).min(self.map.len());
        Ok(Cow::Borrowed(&self.map[start..end]))
    }
}

/// Positioned reads through a shared handle. Used for archives in the tens of
/// gigabytes where mapping is wasteful and for parity with the Python carvers.
pub struct StreamSource {
    path: PathBuf,
    file: File,
    len: u64,
}

impl StreamSource {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = File::open(&path)?;
        let len = file.metadata()?.len();
        Ok(Self { path, file, len })
    }
}

impl Source for StreamSource {
    #[inline]
    fn len(&self) -> u64 {
        self.len
    }

    #[inline]
    fn path(&self) -> &Path {
        &self.path
    }

    fn read_at(&self, off: u64, len: usize) -> io::Result<Vec<u8>> {
        let mut out = vec![0u8; len];
        let mut done = 0usize;
        while done < len {
            let at = off + done as u64;
            if at >= self.len {
                break;
            }
            #[cfg(windows)]
            let n = self.file.seek_read(&mut out[done..], at)?;
            #[cfg(unix)]
            let n = {
                use std::os::unix::fs::FileExt as _;
                self.file.read_at(&mut out[done..], at)?
            };
            if n == 0 {
                break;
            }
            done += n;
        }
        Ok(out)
    }

    fn window(&self, off: u64, len: usize) -> io::Result<Cow<'_, [u8]>> {
        Ok(Cow::Owned(self.read_at(off, len)?))
    }
}

/// In-memory source, used by tests and by callers that already hold the bytes
/// (a carved nested archive, an embedded resource).
pub struct BytesSource {
    path: PathBuf,
    data: Vec<u8>,
}

impl BytesSource {
    pub fn new(name: impl Into<PathBuf>, data: Vec<u8>) -> Self {
        Self {
            path: name.into(),
            data,
        }
    }
}

impl Source for BytesSource {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn read_at(&self, off: u64, len: usize) -> io::Result<Vec<u8>> {
        let start = (off as usize).min(self.data.len());
        let end = start.saturating_add(len).min(self.data.len());
        let mut out = vec![0u8; len];
        out[..end - start].copy_from_slice(&self.data[start..end]);
        Ok(out)
    }

    fn window(&self, off: u64, len: usize) -> io::Result<Cow<'_, [u8]>> {
        let start = (off as usize).min(self.data.len());
        let end = start.saturating_add(len).min(self.data.len());
        Ok(Cow::Borrowed(&self.data[start..end]))
    }
}

/// Open `path` as an mmap when it fits in `max_mmap`, else as a stream.
pub fn open(path: impl AsRef<Path>, max_mmap: u64) -> io::Result<Box<dyn Source>> {
    MmapSource::open(path, max_mmap)
}
