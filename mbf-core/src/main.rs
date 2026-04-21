pub mod hal;
pub mod chal;
pub use hal::{set_hal, hal, Hal, HttpGetResult, ReadSeek, ReadWriteSeek, WriteSeek};
pub use chal::CHal;

pub mod data_fix;
pub mod downloads;
pub mod handlers;
pub mod manifest;
pub mod mod_man;
pub mod models;
pub mod patching;
pub mod parameters;
pub mod downgrading;

use anyhow::{Context, Result};
use downloads::DownloadConfig;
use log::{debug, warn};
use mbf_res_man::res_cache::ResCache;
use parameters::PARAMETERS;
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    sync,
};

/// Attempts to delete legacy directories no longer used by MBF to free up space
pub fn try_delete_legacy_dirs() {
    for dir in &PARAMETERS.legacy_dirs {
        if crate::hal().path_exists(Path::new(dir)) {
            match crate::hal().remove_dir_all(Path::new(dir)) {
                Ok(_) => debug!("Successfully removed legacy dir {dir}"),
                Err(err) => warn!("Failed to remove legacy dir {dir}: {err}"),
            }
        }
    }
}

static DOWNLOAD_CFG: sync::OnceLock<DownloadConfig> = sync::OnceLock::new();

/// Gets the default config used for downloads in MBF
pub fn get_dl_cfg() -> &'static DownloadConfig {
    DOWNLOAD_CFG.get_or_init(|| {
        DownloadConfig {
            max_disconnections: 10,
            disconnection_reset_time: Some(std::time::Duration::from_secs_f32(10.0)),
            disconnect_wait_time: std::time::Duration::from_secs_f32(5.0),
            progress_update_interval: Some(std::time::Duration::from_secs_f32(2.0)),
        }
    })
}

/// Creates a ResCache for downloading files using mbf_res_man
pub fn load_res_cache() -> Result<ResCache<'static>> {
    crate::hal().create_dir_all(Path::new(&PARAMETERS.res_cache)).expect("Failed to create resource cache folder");
    Ok(ResCache::new(
        (&PARAMETERS.res_cache).into(),
        mbf_res_man::default_agent::get_agent(),
    ))
}

pub fn get_apk_path() -> Result<Option<String>> {
    let pm_output = crate::hal().exec_command("pm", &["path", &PARAMETERS.apk_id])
        .context("Working out APK path")?;
    if 8 > pm_output.len() {
        Ok(None)
    } else {
        Ok(Some(
            std::str::from_utf8(pm_output.split_at(8).1)?
                .trim_end()
                .to_owned(),
        ))
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ModTag {
    patcher_name: String,
    patcher_version: Option<String>,
    modloader_name: String,
    modloader_version: Option<String>,
}
