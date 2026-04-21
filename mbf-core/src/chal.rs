//! C-compatible API for registering a HAL implementation with mbf-core.
//!
//! # Overview
//!
//! Non-Rust callers (C, C++, Kotlin/JNI, etc.) can provide a concrete HAL
//! implementation by:
//!
//! 1. Filling a [`CHal`] struct with function pointers.
//! 2. Calling [`mbf_core_register_hal`] once, before any other mbf-core function.
//!
//! # Error reporting
//!
//! All fallible callbacks return `0` on success and a non-zero value on failure.
//! Before returning an error, the callback should call [`mbf_core_set_error`]
//! with a human-readable UTF-8 message.  For HTTP status errors (4xx/5xx) the
//! callback must also call [`mbf_core_set_http_status_error`] so that
//! mbf-core can distinguish them from network errors and skip retries.
//!
//! # Memory ownership
//!
//! Some callbacks allocate buffers that Rust reads and then releases.
//! Rust calls the [`CHal::free_bytes`], [`CHal::free_string`], and
//! [`CHal::free_string_array`] callbacks to perform those releases.
//! Conversely, buffers that mbf-core allocates and passes back to C callers
//! must be freed using the exported [`mbf_core_free_string`] /
//! [`mbf_core_free_bytes`] functions.

use std::{
    ffi::{c_char, c_int, c_void, CStr, CString},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    ptr,
};

use anyhow::{anyhow, Result};

use crate::hal::{Hal, HttpGetResult, HttpStatusError, ReadSeek, ReadWriteSeek, WriteSeek};

// ── Thread-local state ────────────────────────────────────────────────────────

thread_local! {
    static C_ERROR: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
    static C_HTTP_STATUS: std::cell::RefCell<Option<u16>> = std::cell::RefCell::new(None);
}

fn set_c_error_str(msg: impl Into<String>) {
    C_ERROR.with(|e| *e.borrow_mut() = Some(msg.into()));
}

fn take_c_error() -> String {
    C_ERROR.with(|e| {
        e.borrow_mut()
            .take()
            .unwrap_or_else(|| "unknown C HAL error".to_owned())
    })
}

// ── Helper ────────────────────────────────────────────────────────────────────

fn path_to_cstr(path: &Path) -> Result<CString> {
    CString::new(path.to_string_lossy().as_bytes())
        .map_err(|_| anyhow!("Path contained a null byte: {path:?}"))
}

// ── require_fn! macro ─────────────────────────────────────────────────────────

macro_rules! require_fn {
    ($opt:expr, $name:literal) => {
        $opt.ok_or_else(|| anyhow!(concat!($name, " callback not set in CHal")))?
    };
}

// ── C-implemented file handle ─────────────────────────────────────────────────

/// Wraps an opaque C file handle together with the operation vtable from the
/// [`CHal`] struct.
struct CFileHandle {
    handle: *mut c_void,
    file_read: Option<unsafe extern "C" fn(*mut c_void, *mut u8, usize) -> isize>,
    file_write: unsafe extern "C" fn(*mut c_void, *const u8, usize) -> c_int,
    file_seek: unsafe extern "C" fn(*mut c_void, i64, c_int, *mut u64) -> c_int,
    file_set_len: Option<unsafe extern "C" fn(*mut c_void, u64) -> c_int>,
    file_close: unsafe extern "C" fn(*mut c_void),
}

// SAFETY: The C implementation is responsible for thread safety.
unsafe impl Send for CFileHandle {}
unsafe impl Sync for CFileHandle {}

impl Drop for CFileHandle {
    fn drop(&mut self) {
        unsafe { (self.file_close)(self.handle) }
    }
}

impl Read for CFileHandle {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let f = self.file_read.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                "file_read not set in CHal",
            )
        })?;
        let n = unsafe { f(self.handle, buf.as_mut_ptr(), buf.len()) };
        if n < 0 {
            Err(io::Error::new(io::ErrorKind::Other, "C file_read error"))
        } else {
            Ok(n as usize)
        }
    }
}

impl Write for CFileHandle {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let rc =
            unsafe { (self.file_write)(self.handle, buf.as_ptr(), buf.len()) };
        if rc != 0 {
            Err(io::Error::new(io::ErrorKind::Other, "C file_write error"))
        } else {
            Ok(buf.len())
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Seek for CFileHandle {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let (offset, whence) = match pos {
            SeekFrom::Start(n) => (n as i64, 0),
            SeekFrom::Current(n) => (n, 1),
            SeekFrom::End(n) => (n, 2),
        };
        let mut out_pos: u64 = 0;
        let rc = unsafe {
            (self.file_seek)(self.handle, offset, whence, &mut out_pos)
        };
        if rc != 0 {
            Err(io::Error::new(io::ErrorKind::Other, "C file_seek error"))
        } else {
            Ok(out_pos)
        }
    }
}

impl mbf_zip::WriteSeekLen for CFileHandle {
    fn set_len(&mut self, len: u64) -> io::Result<()> {
        let f = self.file_set_len.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                "file_set_len not set in CHal",
            )
        })?;
        let rc = unsafe { f(self.handle, len) };
        if rc != 0 {
            Err(io::Error::new(
                io::ErrorKind::Other,
                "C file_set_len error",
            ))
        } else {
            Ok(())
        }
    }
}

impl ReadWriteSeek for CFileHandle {}

// ── C-implemented read-only file handle ──────────────────────────────────────

/// Wraps an opaque C file handle for read-only access.
struct CReadOnlyFileHandle {
    handle: *mut c_void,
    file_read: unsafe extern "C" fn(*mut c_void, *mut u8, usize) -> isize,
    file_seek: unsafe extern "C" fn(*mut c_void, i64, c_int, *mut u64) -> c_int,
    file_close: unsafe extern "C" fn(*mut c_void),
}

// SAFETY: The C implementation is responsible for thread safety.
unsafe impl Send for CReadOnlyFileHandle {}

impl Drop for CReadOnlyFileHandle {
    fn drop(&mut self) {
        unsafe { (self.file_close)(self.handle) }
    }
}

impl Read for CReadOnlyFileHandle {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = unsafe { (self.file_read)(self.handle, buf.as_mut_ptr(), buf.len()) };
        if n < 0 {
            Err(io::Error::new(io::ErrorKind::Other, "C file_read error"))
        } else {
            Ok(n as usize)
        }
    }
}

impl Seek for CReadOnlyFileHandle {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let (offset, whence) = match pos {
            SeekFrom::Start(n) => (n as i64, 0),
            SeekFrom::Current(n) => (n, 1),
            SeekFrom::End(n) => (n, 2),
        };
        let mut out_pos: u64 = 0;
        let rc = unsafe {
            (self.file_seek)(self.handle, offset, whence, &mut out_pos)
        };
        if rc != 0 {
            Err(io::Error::new(io::ErrorKind::Other, "C file_seek error"))
        } else {
            Ok(out_pos)
        }
    }
}

// ── C-implemented HTTP response reader ───────────────────────────────────────

struct CHttpReader {
    handle: *mut c_void,
    http_read: unsafe extern "C" fn(*mut c_void, *mut u8, usize) -> isize,
    http_free: unsafe extern "C" fn(*mut c_void),
}

// SAFETY: The C implementation is responsible for thread safety.
unsafe impl Send for CHttpReader {}

impl Drop for CHttpReader {
    fn drop(&mut self) {
        unsafe { (self.http_free)(self.handle) }
    }
}

impl Read for CHttpReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n =
            unsafe { (self.http_read)(self.handle, buf.as_mut_ptr(), buf.len()) };
        if n < 0 {
            Err(io::Error::new(io::ErrorKind::Other, "C http_read error"))
        } else {
            Ok(n as usize)
        }
    }
}

// ── CHal: C-compatible function pointer table ─────────────────────────────────

/// C-compatible HAL function pointer table.
///
/// Fill this struct with your implementation's function pointers and call
/// [`mbf_core_register_hal`] to register it with mbf-core.
///
/// All pointers stored inside this struct must remain valid for the entire
/// lifetime of the process.
///
/// See the [module documentation](self) for error reporting and memory
/// ownership conventions.
#[repr(C)]
pub struct CHal {
    // ── Memory management callbacks ──────────────────────────────────────────
    /// Free a byte buffer previously passed *to* Rust via an out-parameter
    /// (e.g. from `exec_command` or `read_file`).
    /// mbf-core calls this after it has finished reading the buffer.
    pub free_bytes: Option<unsafe extern "C" fn(ptr: *mut u8, len: usize)>,

    /// Free a C string previously passed *to* Rust via an out-parameter
    /// (e.g. the filename from `http_get`).
    pub free_string: Option<unsafe extern "C" fn(ptr: *mut c_char)>,

    /// Free a string array previously passed *to* Rust via `read_dir_paths`.
    pub free_string_array:
        Option<unsafe extern "C" fn(ptr: *mut *mut c_char, count: usize)>,

    // ── Process execution ────────────────────────────────────────────────────
    /// Execute a command and capture its stdout.
    ///
    /// * `cmd`      — null-terminated executable name or path.
    /// * `args`     — null-terminated array of null-terminated argument strings.
    /// * `out_data` — on success, set to a newly-allocated buffer of stdout bytes.
    /// * `out_len`  — on success, set to the number of bytes in `*out_data`.
    ///
    /// mbf-core calls [`CHal::free_bytes`] to release the buffer.
    pub exec_command: Option<
        unsafe extern "C" fn(
            cmd: *const c_char,
            args: *const *const c_char,
            out_data: *mut *mut u8,
            out_len: *mut usize,
        ) -> c_int,
    >,

    // ── Networking ───────────────────────────────────────────────────────────
    /// Begin an HTTP GET request.
    ///
    /// * `url`                   — null-terminated URL.
    /// * `range_start`           — if > 0 include `Range: bytes=<range_start>-`.
    /// * `out_handle`            — on success, set to an opaque response handle.
    /// * `out_accepts_ranges`    — set to 1 if the server supports byte ranges.
    /// * `out_filename`          — set to a newly-allocated C string if a filename
    ///                              was provided in the response headers, else NULL.
    ///                              mbf-core calls [`CHal::free_string`] to release it.
    /// * `out_content_length`    — set to Content-Length if available.
    /// * `out_has_content_length`— set to 1 if `*out_content_length` is valid.
    ///
    /// HTTP status errors (4xx/5xx): return non-zero **and** call
    /// [`mbf_core_set_http_status_error`] with the status code.
    pub http_get: Option<
        unsafe extern "C" fn(
            url: *const c_char,
            range_start: usize,
            out_handle: *mut *mut c_void,
            out_accepts_ranges: *mut c_int,
            out_filename: *mut *mut c_char,
            out_content_length: *mut usize,
            out_has_content_length: *mut c_int,
        ) -> c_int,
    >,

    /// Read bytes from an HTTP response handle obtained via [`CHal::http_get`].
    ///
    /// Returns bytes read; `0` = EOF; negative = error.
    pub http_read: Option<
        unsafe extern "C" fn(
            handle: *mut c_void,
            buf: *mut u8,
            len: usize,
        ) -> isize,
    >,

    /// Close and release an HTTP response handle.
    pub http_free: Option<unsafe extern "C" fn(handle: *mut c_void)>,

    // ── Filesystem ───────────────────────────────────────────────────────────
    /// Returns `1` if `path` exists (file or directory), `0` otherwise.
    pub path_exists: Option<unsafe extern "C" fn(path: *const c_char) -> c_int>,

    /// Read an entire file into a newly-allocated buffer.
    ///
    /// On success sets `*out_data` / `*out_len`.
    /// mbf-core calls [`CHal::free_bytes`] to release the buffer.
    pub read_file: Option<
        unsafe extern "C" fn(
            path: *const c_char,
            out_data: *mut *mut u8,
            out_len: *mut usize,
        ) -> c_int,
    >,

    /// Write `len` bytes from `data` to `path`, creating or truncating the file.
    pub write_file: Option<
        unsafe extern "C" fn(
            path: *const c_char,
            data: *const u8,
            len: usize,
        ) -> c_int,
    >,

    /// Open `path` for reading only.
    ///
    /// On success sets `*out_handle` to an opaque file handle.
    /// Use the `file_read`, `file_seek`, and `file_close` callbacks to operate on it.
    /// If NULL, [`CHal::open_file_rw`] is used as a fallback.
    pub open_file_read: Option<
        unsafe extern "C" fn(
            path: *const c_char,
            out_handle: *mut *mut c_void,
        ) -> c_int,
    >,

    /// Open `path` for reading **and** writing.
    ///
    /// On success sets `*out_handle` to an opaque file handle.
    /// Use the `file_*` callbacks to operate on it; call [`CHal::file_close`]
    /// when done.
    pub open_file_rw: Option<
        unsafe extern "C" fn(
            path: *const c_char,
            out_handle: *mut *mut c_void,
        ) -> c_int,
    >,

    /// Open `path` for writing, creating or truncating the file.
    ///
    /// On success sets `*out_handle` to an opaque file handle.
    pub open_file_write: Option<
        unsafe extern "C" fn(
            path: *const c_char,
            out_handle: *mut *mut c_void,
        ) -> c_int,
    >,

    /// Read bytes from a file handle.
    ///
    /// Returns bytes read; `0` = EOF; negative = error.
    pub file_read: Option<
        unsafe extern "C" fn(
            handle: *mut c_void,
            buf: *mut u8,
            len: usize,
        ) -> isize,
    >,

    /// Write bytes to a file handle.  Returns `0` on success.
    pub file_write: Option<
        unsafe extern "C" fn(
            handle: *mut c_void,
            data: *const u8,
            len: usize,
        ) -> c_int,
    >,

    /// Seek a file handle.
    ///
    /// * `whence` — `0` = start, `1` = current position, `2` = end.
    /// * `out_pos` — set to the new position from the start of the file.
    pub file_seek: Option<
        unsafe extern "C" fn(
            handle: *mut c_void,
            offset: i64,
            whence: c_int,
            out_pos: *mut u64,
        ) -> c_int,
    >,

    /// Truncate or extend a file handle to exactly `len` bytes.
    pub file_set_len:
        Option<unsafe extern "C" fn(handle: *mut c_void, len: u64) -> c_int>,

    /// Close and release a file handle obtained via `open_file_rw` or
    /// `open_file_write`.
    pub file_close: Option<unsafe extern "C" fn(handle: *mut c_void)>,

    /// Copy a file from `from` to `to`, overwriting `to` if it exists.
    pub copy_file: Option<
        unsafe extern "C" fn(
            from: *const c_char,
            to: *const c_char,
        ) -> c_int,
    >,

    /// Remove the file at `path`.
    pub remove_file:
        Option<unsafe extern "C" fn(path: *const c_char) -> c_int>,

    /// Recursively remove `path` and all of its contents.
    pub remove_dir_all:
        Option<unsafe extern "C" fn(path: *const c_char) -> c_int>,

    /// Recursively create `path` and all necessary parent directories.
    pub create_dir_all:
        Option<unsafe extern "C" fn(path: *const c_char) -> c_int>,

    /// List the direct children of the directory at `path`.
    ///
    /// On success sets `*out_paths` to a newly-allocated array of
    /// null-terminated C strings and `*out_count` to the number of entries.
    /// mbf-core calls [`CHal::free_string_array`] to release the array.
    pub read_dir_paths: Option<
        unsafe extern "C" fn(
            path: *const c_char,
            out_paths: *mut *mut *mut c_char,
            out_count: *mut usize,
        ) -> c_int,
    >,

    /// Clear the read-only attribute on `path`.
    pub set_permissions_writable:
        Option<unsafe extern "C" fn(path: *const c_char) -> c_int>,

    /// Returns `1` if `path` is a directory, `0` otherwise.
    pub is_dir: Option<unsafe extern "C" fn(path: *const c_char) -> c_int>,
}

// SAFETY: CHal is a plain struct of C function pointers, which are Send + Sync.
unsafe impl Send for CHal {}
unsafe impl Sync for CHal {}

// ── Impl Hal for CHal ─────────────────────────────────────────────────────────

impl Hal for CHal {
    fn exec_command(&self, cmd: &str, args: &[&str]) -> Result<Vec<u8>> {
        let f = require_fn!(self.exec_command, "exec_command");
        let cmd_c = CString::new(cmd)?;
        let arg_cstrings: Vec<CString> = args
            .iter()
            .map(|a| CString::new(*a))
            .collect::<std::result::Result<_, _>>()?;
        let mut arg_ptrs: Vec<*const c_char> =
            arg_cstrings.iter().map(|s| s.as_ptr()).collect();
        arg_ptrs.push(ptr::null()); // null-terminate argv

        let mut out_data: *mut u8 = ptr::null_mut();
        let mut out_len: usize = 0;
        let rc = unsafe {
            f(
                cmd_c.as_ptr(),
                arg_ptrs.as_ptr(),
                &mut out_data,
                &mut out_len,
            )
        };
        if rc != 0 {
            return Err(anyhow!(take_c_error()));
        }
        let data =
            unsafe { std::slice::from_raw_parts(out_data, out_len).to_vec() };
        if let Some(free_fn) = self.free_bytes {
            unsafe { free_fn(out_data, out_len) };
        }
        Ok(data)
    }

    fn http_get(&self, url: &str, range_start: usize) -> Result<HttpGetResult> {
        let f = require_fn!(self.http_get, "http_get");
        let http_read = require_fn!(self.http_read, "http_read");
        let http_free = require_fn!(self.http_free, "http_free");

        let url_c = CString::new(url)?;
        let mut handle: *mut c_void = ptr::null_mut();
        let mut accepts_ranges: c_int = 0;
        let mut filename_ptr: *mut c_char = ptr::null_mut();
        let mut content_length: usize = 0;
        let mut has_content_length: c_int = 0;

        let rc = unsafe {
            f(
                url_c.as_ptr(),
                range_start,
                &mut handle,
                &mut accepts_ranges,
                &mut filename_ptr,
                &mut content_length,
                &mut has_content_length,
            )
        };

        if rc != 0 {
            let status = C_HTTP_STATUS.with(|s| s.borrow_mut().take());
            if let Some(code) = status {
                return Err(anyhow::Error::new(HttpStatusError { status: code }));
            }
            return Err(anyhow!(take_c_error()));
        }

        let filename = if filename_ptr.is_null() {
            None
        } else {
            let s = unsafe {
                CStr::from_ptr(filename_ptr)
                    .to_string_lossy()
                    .into_owned()
            };
            if let Some(free_fn) = self.free_string {
                unsafe { free_fn(filename_ptr) };
            }
            Some(s)
        };

        Ok(HttpGetResult {
            reader: Box::new(CHttpReader {
                handle,
                http_read,
                http_free,
            }),
            accepts_ranges: accepts_ranges != 0,
            filename,
            content_length: if has_content_length != 0 {
                Some(content_length)
            } else {
                None
            },
        })
    }

    fn path_exists(&self, path: &Path) -> bool {
        let f = match self.path_exists {
            Some(f) => f,
            None => return false,
        };
        let p = match path_to_cstr(path) {
            Ok(p) => p,
            Err(_) => return false,
        };
        unsafe { f(p.as_ptr()) != 0 }
    }

    fn read_file(&self, path: &Path) -> Result<Vec<u8>> {
        let f = require_fn!(self.read_file, "read_file");
        let p = path_to_cstr(path)?;
        let mut out_data: *mut u8 = ptr::null_mut();
        let mut out_len: usize = 0;
        let rc = unsafe { f(p.as_ptr(), &mut out_data, &mut out_len) };
        if rc != 0 {
            return Err(anyhow!(take_c_error()));
        }
        let data =
            unsafe { std::slice::from_raw_parts(out_data, out_len).to_vec() };
        if let Some(free_fn) = self.free_bytes {
            unsafe { free_fn(out_data, out_len) };
        }
        Ok(data)
    }

    fn write_file(&self, path: &Path, data: &[u8]) -> Result<()> {
        let f = require_fn!(self.write_file, "write_file");
        let p = path_to_cstr(path)?;
        let rc = unsafe { f(p.as_ptr(), data.as_ptr(), data.len()) };
        if rc != 0 {
            Err(anyhow!(take_c_error()))
        } else {
            Ok(())
        }
    }

    fn open_file_read(&self, path: &Path) -> Result<Box<dyn ReadSeek>> {
        let file_read = require_fn!(self.file_read, "file_read");
        let file_seek = require_fn!(self.file_seek, "file_seek");
        let file_close = require_fn!(self.file_close, "file_close");

        let p = path_to_cstr(path)?;
        let mut handle: *mut c_void = ptr::null_mut();

        // Prefer the dedicated read-only opener; fall back to open_file_rw.
        let rc = if let Some(f) = self.open_file_read {
            unsafe { f(p.as_ptr(), &mut handle) }
        } else {
            let f = require_fn!(self.open_file_rw, "open_file_read or open_file_rw");
            unsafe { f(p.as_ptr(), &mut handle) }
        };

        if rc != 0 {
            return Err(anyhow!(take_c_error()));
        }
        Ok(Box::new(CReadOnlyFileHandle {
            handle,
            file_read,
            file_seek,
            file_close,
        }))
    }

    fn open_file_rw(&self, path: &Path) -> Result<Box<dyn ReadWriteSeek>> {
        let f = require_fn!(self.open_file_rw, "open_file_rw");
        let file_write = require_fn!(self.file_write, "file_write");
        let file_seek = require_fn!(self.file_seek, "file_seek");
        let file_close = require_fn!(self.file_close, "file_close");

        let p = path_to_cstr(path)?;
        let mut handle: *mut c_void = ptr::null_mut();
        let rc = unsafe { f(p.as_ptr(), &mut handle) };
        if rc != 0 {
            return Err(anyhow!(take_c_error()));
        }
        Ok(Box::new(CFileHandle {
            handle,
            file_read: self.file_read,
            file_write,
            file_seek,
            file_set_len: self.file_set_len,
            file_close,
        }))
    }

    fn open_file_write(&self, path: &Path) -> Result<Box<dyn WriteSeek>> {
        let f = require_fn!(self.open_file_write, "open_file_write");
        let file_write = require_fn!(self.file_write, "file_write");
        let file_seek = require_fn!(self.file_seek, "file_seek");
        let file_close = require_fn!(self.file_close, "file_close");

        let p = path_to_cstr(path)?;
        let mut handle: *mut c_void = ptr::null_mut();
        let rc = unsafe { f(p.as_ptr(), &mut handle) };
        if rc != 0 {
            return Err(anyhow!(take_c_error()));
        }
        Ok(Box::new(CFileHandle {
            handle,
            file_read: self.file_read,
            file_write,
            file_seek,
            file_set_len: self.file_set_len,
            file_close,
        }))
    }

    fn copy_file(&self, from: &Path, to: &Path) -> Result<()> {
        let f = require_fn!(self.copy_file, "copy_file");
        let from_c = path_to_cstr(from)?;
        let to_c = path_to_cstr(to)?;
        let rc = unsafe { f(from_c.as_ptr(), to_c.as_ptr()) };
        if rc != 0 {
            Err(anyhow!(take_c_error()))
        } else {
            Ok(())
        }
    }

    fn remove_file(&self, path: &Path) -> Result<()> {
        let f = require_fn!(self.remove_file, "remove_file");
        let p = path_to_cstr(path)?;
        let rc = unsafe { f(p.as_ptr()) };
        if rc != 0 {
            Err(anyhow!(take_c_error()))
        } else {
            Ok(())
        }
    }

    fn remove_dir_all(&self, path: &Path) -> Result<()> {
        let f = require_fn!(self.remove_dir_all, "remove_dir_all");
        let p = path_to_cstr(path)?;
        let rc = unsafe { f(p.as_ptr()) };
        if rc != 0 {
            Err(anyhow!(take_c_error()))
        } else {
            Ok(())
        }
    }

    fn create_dir_all(&self, path: &Path) -> Result<()> {
        let f = require_fn!(self.create_dir_all, "create_dir_all");
        let p = path_to_cstr(path)?;
        let rc = unsafe { f(p.as_ptr()) };
        if rc != 0 {
            Err(anyhow!(take_c_error()))
        } else {
            Ok(())
        }
    }

    fn read_dir_paths(&self, path: &Path) -> Result<Vec<PathBuf>> {
        let f = require_fn!(self.read_dir_paths, "read_dir_paths");
        let p = path_to_cstr(path)?;
        let mut out_paths: *mut *mut c_char = ptr::null_mut();
        let mut out_count: usize = 0;
        let rc =
            unsafe { f(p.as_ptr(), &mut out_paths, &mut out_count) };
        if rc != 0 {
            return Err(anyhow!(take_c_error()));
        }
        let mut result = Vec::with_capacity(out_count);
        for i in 0..out_count {
            let s = unsafe {
                CStr::from_ptr(*out_paths.add(i))
                    .to_string_lossy()
                    .into_owned()
            };
            result.push(PathBuf::from(s));
        }
        if let Some(free_fn) = self.free_string_array {
            unsafe { free_fn(out_paths, out_count) };
        }
        Ok(result)
    }

    fn set_permissions_writable(&self, path: &Path) -> Result<()> {
        let f =
            require_fn!(self.set_permissions_writable, "set_permissions_writable");
        let p = path_to_cstr(path)?;
        let rc = unsafe { f(p.as_ptr()) };
        if rc != 0 {
            Err(anyhow!(take_c_error()))
        } else {
            Ok(())
        }
    }

    fn is_dir(&self, path: &Path) -> bool {
        let f = match self.is_dir {
            Some(f) => f,
            None => return false,
        };
        let p = match path_to_cstr(path) {
            Ok(p) => p,
            Err(_) => return false,
        };
        unsafe { f(p.as_ptr()) != 0 }
    }
}

// ── Exported C API ────────────────────────────────────────────────────────────

/// Register a C-implemented HAL with mbf-core.
///
/// This **must** be called exactly once before any other mbf-core function.
/// All function pointers stored inside `hal` must remain valid for the entire
/// lifetime of the process.
///
/// # Safety
///
/// All function pointers in `hal` must be valid and safe to call concurrently
/// from multiple threads.
#[no_mangle]
pub unsafe extern "C" fn mbf_core_register_hal(hal: CHal) {
    crate::set_hal(Box::new(hal));
}

/// Set the human-readable error message for the most recent failed C HAL call.
///
/// Call this from your HAL implementation *before* returning a non-zero error
/// code so that mbf-core can surface a meaningful diagnostic.
///
/// # Safety
///
/// `message` must be a valid, null-terminated UTF-8 string.
#[no_mangle]
pub unsafe extern "C" fn mbf_core_set_error(message: *const c_char) {
    if message.is_null() {
        return;
    }
    let msg = CStr::from_ptr(message).to_string_lossy().into_owned();
    set_c_error_str(msg);
}

/// Signal that a [`CHal::http_get`] failure was caused by an HTTP status error
/// (i.e. the server responded with a 4xx or 5xx code).
///
/// Call this *in addition to* returning a non-zero code from `http_get`.
/// mbf-core uses this information to avoid retrying requests that definitively
/// failed at the server level.
#[no_mangle]
pub extern "C" fn mbf_core_set_http_status_error(status: u16) {
    C_HTTP_STATUS.with(|s| *s.borrow_mut() = Some(status));
}

/// Free a C string that was allocated by mbf-core and returned to a C caller.
///
/// # Safety
///
/// `ptr` must have been allocated by mbf-core and must not be accessed after
/// this call.
#[no_mangle]
pub unsafe extern "C" fn mbf_core_free_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        drop(CString::from_raw(ptr));
    }
}

/// Free a byte buffer that was allocated by mbf-core and returned to a C caller.
///
/// # Safety
///
/// `ptr` must have been allocated by mbf-core, `len` must match the length
/// originally returned, and `ptr` must not be accessed after this call.
#[no_mangle]
pub unsafe extern "C" fn mbf_core_free_bytes(ptr: *mut u8, len: usize) {
    if !ptr.is_null() {
        drop(Vec::from_raw_parts(ptr, len, len));
    }
}
