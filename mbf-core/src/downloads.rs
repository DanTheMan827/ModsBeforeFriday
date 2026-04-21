//! Module that allows downloading of files in a reasonably flexible and reliable way

use anyhow::{Context, Result};
use log::{error, info, warn};
use std::{
    io::{self, Cursor, Read, Seek, Write},
    path::Path,
    time::Instant,
};

use crate::hal::HttpStatusError;

/// Various configuration settings for the file downloader.
pub struct DownloadConfig {
    pub max_disconnections: u32,
    pub disconnection_reset_time: Option<std::time::Duration>,
    pub disconnect_wait_time: std::time::Duration,
    pub progress_update_interval: Option<std::time::Duration>,
}

enum DownloadFileError {
    InitialRequest(anyhow::Error),
    LostConnDuringDownload(io::Error),
}

fn download_file_to_stream<T: FnMut(usize, Option<usize>) -> ()>(
    _cfg: &DownloadConfig,
    file_offset: usize,
    url: &str,
    out_supports_ranges: &mut bool,
    out_filename: &mut Option<String>,
    mut progress_update: T,
    to: impl Write,
) -> Result<(), DownloadFileError> {
    let result = crate::hal().http_get(url, file_offset)
        .map_err(|e| DownloadFileError::InitialRequest(e))?;

    *out_supports_ranges = result.accepts_ranges;
    *out_filename = result.filename;
    let content_length = result.content_length;
    let mut reader = result.reader;

    if content_length.is_none() {
        warn!("No Content-Length header provided, so MBF cannot update you on the download progress");
    }

    copy_stream_progress(&mut reader, to, |bytes_written| {
        progress_update(bytes_written, content_length)
    })
    .map_err(|err| DownloadFileError::LostConnDuringDownload(err))?;

    Ok(())
}

fn copy_stream_progress<T: FnMut(usize) -> ()>(
    from: &mut impl Read,
    mut to: impl Write,
    mut progress: T,
) -> Result<(), io::Error> {
    let mut buffer = vec![0u8; 8192];
    let mut total_read = 0;
    loop {
        let bytes_read = from.read(&mut buffer)?;
        to.write_all(&buffer[0..bytes_read])?;
        if bytes_read == 0 {
            break Ok(());
        } else {
            total_read += bytes_read;
            progress(total_read);
        }
    }
}

pub fn download_with_attempts(
    cfg: &DownloadConfig,
    mut to: impl Write + Seek,
    url: &str,
) -> Result<Option<String>> {
    let mut failed_attempts = 0;
    let mut bytes_valid: usize = 0;
    let mut file_name: Option<String> = None;
    let mut ranges_supported = false;

    loop {
        if failed_attempts > 0 {
            if ranges_supported {
                info!("Continuing download");
            } else {
                warn!("Restarting entire download as the server has no support for resuming: Attempt {}", failed_attempts + 1);
            }
        }

        to.seek(io::SeekFrom::Start(bytes_valid as u64))?;
        let bytes_valid_before_req = bytes_valid;

        let request_time = Instant::now();
        let mut last_progress_update = Instant::now();
        let result = download_file_to_stream(
            cfg,
            bytes_valid,
            url,
            &mut ranges_supported,
            &mut file_name,
            |bytes_written, total_bytes| {
                bytes_valid = bytes_valid_before_req + bytes_written;

                let now = Instant::now();

                if let Some(reset_time) = cfg.disconnection_reset_time {
                    if (now - request_time) > reset_time && failed_attempts > 0 {
                        info!("Resetting failed attempts as download appears to be completing successfully");
                        failed_attempts = 0;
                    }
                }

                match (cfg.progress_update_interval, total_bytes) {
                    (Some(interval), Some(length)) => {
                        if now.duration_since(last_progress_update) > interval {
                            last_progress_update = now;
                            info!(
                                "Progress: {:.2}%",
                                ((bytes_written + bytes_valid_before_req) as f32
                                    / (bytes_valid_before_req + length) as f32)
                                    * 100.0
                            );
                        }
                    }
                    _ => {}
                }
            },
            &mut to,
        );

        match result {
            Ok(_) => return Ok(file_name),
            Err(err) => {
                failed_attempts += 1;
                let dl_failed = failed_attempts > cfg.max_disconnections;

                if !ranges_supported {
                    bytes_valid = 0;
                }

                match err {
                    DownloadFileError::InitialRequest(err) => {
                        if err.downcast_ref::<HttpStatusError>().is_some() {
                            return Err(err);
                        }
                        if dl_failed {
                            return Err(err).context("Downloading file: all attempts exhausted");
                        }
                        error!("Failed to make initial request: {err}");
                    }
                    DownloadFileError::LostConnDuringDownload(io_error) => {
                        if dl_failed {
                            return Err(io_error).context(
                                "Lost connection mid download and ran out of download attempts",
                            );
                        }
                        error!("Failed to complete file download: {io_error}");
                    }
                };

                info!("Waiting briefly for the connection to (hopefully) come back");
                std::thread::sleep(cfg.disconnect_wait_time);
            }
        }
    }
}

pub fn download_file_with_attempts(
    cfg: &DownloadConfig,
    to: impl AsRef<Path>,
    url: &str,
) -> Result<Option<String>> {
    let writer = crate::hal().open_file_write(to.as_ref())
        .context("Creating destination file")?;
    download_with_attempts(cfg, writer, url)
}

pub fn download_to_vec_with_attempts(cfg: &DownloadConfig, url: &str) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    download_with_attempts(cfg, Cursor::new(&mut output), url)?;
    Ok(output)
}
