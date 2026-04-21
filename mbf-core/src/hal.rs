//! Host Abstraction Layer (HAL) trait and global accessor.

use std::{
    io::{Read, Seek, Write},
    path::{Path, PathBuf},
    sync::OnceLock,
};

use anyhow::Result;
use mbf_zip::WriteSeekLen;

/// A combined `Read + Seek + Send` supertrait for use as a trait object (read-only file access).
pub trait ReadSeek: Read + Seek + Send {}
impl<T: Read + Seek + Send> ReadSeek for T {}

/// A combined `Read + Write + Seek + Send + WriteSeekLen` supertrait for use as a trait object.
pub trait ReadWriteSeek: WriteSeekLen + Send {}

impl mbf_zip::WriteSeekLen for Box<dyn ReadWriteSeek> {
    fn set_len(&mut self, len: u64) -> std::io::Result<()> {
        (**self).set_len(len)
    }
}

impl ReadWriteSeek for Box<dyn ReadWriteSeek> {}

/// A combined `Write + Seek + Send` supertrait for use as a trait object.
pub trait WriteSeek: Write + Seek + Send {}
impl<T: Write + Seek + Send> WriteSeek for T {}

/// Returned (wrapped in `anyhow::Error`) by [`Hal::http_get`] when the server
/// responds with a non-2xx HTTP status.  Unlike a transport error, this should
/// **not** be retried.
#[derive(Debug)]
pub struct HttpStatusError {
    pub status: u16,
}
impl std::fmt::Display for HttpStatusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Request failed with HTTP status {}", self.status)
    }
}
impl std::error::Error for HttpStatusError {}

/// Result of an HTTP GET request performed by the HAL.
pub struct HttpGetResult {
    /// A readable stream of the response body.
    pub reader: Box<dyn Read + Send>,
    /// Whether the server indicated it supports byte-range requests.
    pub accepts_ranges: bool,
    /// The filename extracted from the Content-Disposition header, if present.
    pub filename: Option<String>,
    /// The number of bytes in the response body, if the server provided it.
    pub content_length: Option<usize>,
}

/// The Host Abstraction Layer.
pub trait Hal: Send + Sync + 'static {
    fn exec_command(&self, cmd: &str, args: &[&str]) -> Result<Vec<u8>>;
    fn http_get(&self, url: &str, range_start: usize) -> Result<HttpGetResult>;
    fn path_exists(&self, path: &Path) -> bool;
    fn read_file(&self, path: &Path) -> Result<Vec<u8>>;
    fn write_file(&self, path: &Path, data: &[u8]) -> Result<()>;
    fn open_file_read(&self, path: &Path) -> Result<Box<dyn ReadSeek>>;
    fn open_file_rw(&self, path: &Path) -> Result<Box<dyn ReadWriteSeek>>;
    fn open_file_write(&self, path: &Path) -> Result<Box<dyn WriteSeek>>;
    fn copy_file(&self, from: &Path, to: &Path) -> Result<()>;
    fn remove_file(&self, path: &Path) -> Result<()>;
    fn remove_dir_all(&self, path: &Path) -> Result<()>;
    fn create_dir_all(&self, path: &Path) -> Result<()>;
    fn read_dir_paths(&self, path: &Path) -> Result<Vec<PathBuf>>;
    fn set_permissions_writable(&self, path: &Path) -> Result<()>;
    fn is_dir(&self, path: &Path) -> bool;
}

static HAL: OnceLock<Box<dyn Hal>> = OnceLock::new();

/// Initialise the global HAL. Must be called exactly once before any other mbf-core function.
pub fn set_hal(hal: Box<dyn Hal>) {
    if HAL.set(hal).is_err() {
        panic!("HAL already initialised");
    }
}

/// Returns a reference to the global HAL.
pub fn hal() -> &'static dyn Hal {
    match HAL.get() {
        Some(h) => &**h,
        None => panic!("HAL not initialised – call set_hal() before using mbf-core"),
    }
}
