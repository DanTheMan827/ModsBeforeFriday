//! Native HAL implementation using std::fs, std::process::Command, and ureq.

use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Seek, Write},
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{anyhow, Context, Result};
use mbf_core::hal::{Hal, HttpGetResult, HttpStatusError, ReadSeek, ReadWriteSeek, WriteSeek};

pub struct NativeHal;

impl NativeHal {
    pub fn new() -> Self {
        NativeHal
    }
}

/// Wraps a `File` so it implements the `ReadWriteSeek` supertrait.
struct ReadWriteSeekFile(File);

impl Read for ReadWriteSeekFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl Write for ReadWriteSeekFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl Seek for ReadWriteSeekFile {
    fn seek(&mut self, pos: io::SeekFrom) -> io::Result<u64> {
        self.0.seek(pos)
    }
}

impl mbf_zip::WriteSeekLen for ReadWriteSeekFile {
    fn set_len(&mut self, len: u64) -> io::Result<()> {
        self.0.set_len(len)
    }
}

impl mbf_core::hal::ReadWriteSeek for ReadWriteSeekFile {}

/// Wraps a `File` so it implements the `WriteSeek` supertrait.
struct WriteSeekFile(File);

impl Write for WriteSeekFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl Seek for WriteSeekFile {
    fn seek(&mut self, pos: io::SeekFrom) -> io::Result<u64> {
        self.0.seek(pos)
    }
}

impl Hal for NativeHal {
    fn exec_command(&self, cmd: &str, args: &[&str]) -> Result<Vec<u8>> {
        let output = Command::new(cmd)
            .args(args)
            .output()
            .with_context(|| format!("Executing command '{cmd}'"))?;
        Ok(output.stdout)
    }

    fn http_get(&self, url: &str, range_start: usize) -> Result<HttpGetResult> {
        let agent = mbf_res_man::default_agent::get_agent();
        let mut req = agent.get(url);
        if range_start > 0 {
            req = req.set("Range", &format!("bytes={range_start}-"));
        }

        let resp = req.call().map_err(|e| match e {
            ureq::Error::Status(code, _) => {
                anyhow::Error::new(HttpStatusError { status: code })
            }
            ureq::Error::Transport(t) => anyhow::anyhow!("{t}"),
        })?;

        let accepts_ranges = resp
            .header("Accept-Ranges")
            .map(|v| v.eq_ignore_ascii_case("bytes"))
            .unwrap_or(false);

        let filename = resp
            .header("Content-Disposition")
            .and_then(|v| {
                v.split(';')
                    .find_map(|part| {
                        let part = part.trim();
                        part.strip_prefix("filename=").map(|f| f.trim_matches('"').to_string())
                    })
            });

        let content_length = resp
            .header("Content-Length")
            .and_then(|v| v.parse::<usize>().ok());

        let reader: Box<dyn Read + Send> = Box::new(resp.into_reader());

        Ok(HttpGetResult {
            reader,
            accepts_ranges,
            filename,
            content_length,
        })
    }

    fn path_exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn read_file(&self, path: &Path) -> Result<Vec<u8>> {
        std::fs::read(path).with_context(|| format!("Reading file {}", path.display()))
    }

    fn write_file(&self, path: &Path, data: &[u8]) -> Result<()> {
        std::fs::write(path, data).with_context(|| format!("Writing file {}", path.display()))
    }

    fn open_file_read(&self, path: &Path) -> Result<Box<dyn ReadSeek>> {
        let file = std::fs::File::open(path)
            .with_context(|| format!("Opening file for reading: {}", path.display()))?;
        Ok(Box::new(file))
    }

    fn open_file_rw(&self, path: &Path) -> Result<Box<dyn ReadWriteSeek>> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(path)
            .with_context(|| format!("Opening file read/write: {}", path.display()))?;
        Ok(Box::new(ReadWriteSeekFile(file)))
    }

    fn open_file_write(&self, path: &Path) -> Result<Box<dyn WriteSeek>> {
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .with_context(|| format!("Opening file for writing: {}", path.display()))?;
        Ok(Box::new(WriteSeekFile(file)))
    }

    fn copy_file(&self, from: &Path, to: &Path) -> Result<()> {
        std::fs::copy(from, to)
            .with_context(|| format!("Copying {} to {}", from.display(), to.display()))?;
        Ok(())
    }

    fn remove_file(&self, path: &Path) -> Result<()> {
        std::fs::remove_file(path).with_context(|| format!("Removing file {}", path.display()))
    }

    fn remove_dir_all(&self, path: &Path) -> Result<()> {
        std::fs::remove_dir_all(path).with_context(|| format!("Removing dir {}", path.display()))
    }

    fn create_dir_all(&self, path: &Path) -> Result<()> {
        std::fs::create_dir_all(path).with_context(|| format!("Creating dir {}", path.display()))
    }

    fn read_dir_paths(&self, path: &Path) -> Result<Vec<PathBuf>> {
        let mut paths = Vec::new();
        for entry in std::fs::read_dir(path).with_context(|| format!("Reading dir {}", path.display()))? {
            match entry {
                Ok(e) => paths.push(e.path()),
                Err(_) => continue,
            }
        }
        Ok(paths)
    }

    fn set_permissions_writable(&self, path: &Path) -> Result<()> {
        let mut perms = std::fs::metadata(path)
            .with_context(|| format!("Getting metadata for {}", path.display()))?
            .permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        std::fs::set_permissions(path, perms)
            .with_context(|| format!("Setting permissions on {}", path.display()))
    }

    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }
}
