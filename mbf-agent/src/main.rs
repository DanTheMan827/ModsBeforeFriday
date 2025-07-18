mod axml;
mod data_fix;
mod downloads;
mod handlers;
mod manifest;
mod mod_man;
mod models;
mod patching;
mod paths;

use anyhow::{Context, Result};
#[cfg(feature = "cli")]
use clap::{ command, Parser };
use downloads::DownloadConfig;
use log::{debug, error, warn, Level};
use mbf_res_man::res_cache::ResCache;
use models::{request, response};
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, BufReader, Write},
    panic,
    path::Path,
    process::Command,
    sync,
};

/// The ID of the APK file that MBF manages.
pub const APK_ID: &str = "com.beatgames.beatsaber";

#[cfg(feature = "request_timing")]
use log::info;
#[cfg(feature = "request_timing")]
use std::time::Instant;

/// Attempts to delete legacy directories no longer used by MBF to free up space
/// Logs on failure
pub fn try_delete_legacy_dirs() {
    for dir in paths::LEGACY_DIRS {
        if Path::new(dir).exists() {
            match std::fs::remove_dir_all(dir) {
                Ok(_) => debug!("Successfully removed legacy dir {dir}"),
                Err(err) => warn!("Failed to remove legacy dir {dir}: {err}"),
            }
        }
    }
}

static DOWNLOAD_CFG: sync::OnceLock<DownloadConfig> = sync::OnceLock::new();

/// Gets the default config used for downloads in MBF
pub fn get_dl_cfg() -> &'static DownloadConfig<'static> {
    DOWNLOAD_CFG.get_or_init(|| {
        DownloadConfig {
            max_disconnections: 10,
            // If downloads data successfully for 10 seconds, reset disconnection attempts
            disconnection_reset_time: Some(std::time::Duration::from_secs_f32(10.0)),
            disconnect_wait_time: std::time::Duration::from_secs_f32(5.0),
            progress_update_interval: Some(std::time::Duration::from_secs_f32(2.0)),
            ureq_agent: mbf_res_man::default_agent::get_agent(),
        }
    })
}

/// Creates a ResCache for downloading files using mbf_res_man
/// This should be reused where possible.
pub fn load_res_cache() -> Result<ResCache<'static>> {
    std::fs::create_dir_all(paths::RES_CACHE).expect("Failed to create resource cache folder");
    Ok(ResCache::new(
        paths::RES_CACHE.into(),
        mbf_res_man::default_agent::get_agent(),
    ))
}

pub fn get_apk_path() -> Result<Option<String>> {
    let pm_output = Command::new("pm")
        .args(["path", APK_ID])
        .output()
        .context("Working out APK path")?;
    if 8 > pm_output.stdout.len() {
        // App not installed
        Ok(None)
    } else {
        Ok(Some(
            std::str::from_utf8(pm_output.stdout.split_at(8).1)?
                .trim_end()
                .to_owned(),
        ))
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct ModTag {
    patcher_name: String,
    patcher_version: Option<String>,
    modloader_name: String,
    modloader_version: Option<String>,
}

struct ResponseLogger {}

impl log::Log for ResponseLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= Level::Debug
    }

    fn log(&self, record: &log::Record) {
        // Skip logs that are not from mbf_agent, mbf_zip, etc.
        // ...as these are spammy logs from ureq or rustls, and we do nto want them.
        match record.module_path() {
            Some(module_path) => {
                if !module_path.starts_with("mbf") {
                    return;
                }
            }
            None => return,
        }

        // Ignore errors, logging should be infallible and we don't want to panic
        let _result = write_response(response::Response::LogMsg {
            message: format!("{}", record.args()),
            level: match record.level() {
                Level::Debug => response::LogLevel::Debug,
                Level::Info => response::LogLevel::Info,
                Level::Warn => response::LogLevel::Warn,
                Level::Error => response::LogLevel::Error,
                Level::Trace => response::LogLevel::Trace,
            },
        });
    }

    fn flush(&self) {
        let _ = std::io::stdout().flush();
    }
}

fn write_response(response: response::Response) -> Result<()> {
    let mut lock = std::io::stdout().lock();
    serde_json::to_writer(&mut lock, &response).context("Serializing JSON response")?;
    writeln!(lock)?;
    Ok(())
}

static LOGGER: ResponseLogger = ResponseLogger {};

#[cfg(not(feature = "cli"))]
fn main() -> Result<()> {
    #[cfg(feature = "request_timing")]
    let start_time = Instant::now();

    log::set_logger(&LOGGER).expect("Failed to set up logging");
    log::set_max_level(log::LevelFilter::Debug);

    let mut reader = BufReader::new(std::io::stdin());
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let req: request::Request = serde_json::from_str(&line)?;

    // Set a panic hook that writes the panic as a JSON Log
    // (we don't do this in catch_unwind as we get an `Any` there, which doesn't implement Display)
    panic::set_hook(Box::new(|info| {
        error!("Request failed due to a panic!: {info}")
    }));

    match std::panic::catch_unwind(|| handlers::handle_request(req)) {
        Ok(resp) => match resp {
            Ok(resp) => {
                #[cfg(feature = "request_timing")]
                {
                    let req_time = Instant::now() - start_time;
                    info!("Request complete in {}ms", req_time.as_millis());
                }

                write_response(resp)?;
            }
            Err(err) => error!("{err:?}"),
        },
        Err(_) => {} // Panic will be outputted above
    };

    Ok(())
}

#[cfg(feature = "cli")]
use std::path::PathBuf;

#[cfg(feature = "cli")]
fn parse_key_val(s: &str) -> Result<(String, String), String> {
    s.split_once('=')
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .ok_or_else(|| format!("Invalid KEY=VALUE format: `{}`", s))
}

#[cfg(feature = "cli")]
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct CliArgs {
    /// The APK file to patch
    #[arg(index = 1)]
    apk: PathBuf,
    
    /// Path to the unstripped libunity.so file
    #[clap(long)]
    libunity_path: Option<PathBuf>,
    
    /// Only update the manifest, do not apply any other patches.
    #[clap(long)]
    manifest_only: bool,
    
    
    /// Permission to add to the manifest.
    #[clap(long = "add-permission", value_name = "PERMISSION", num_args = 1..)]
    add_permissions: Vec<String>,
    
    /// Permission to remove from the manifest.
    #[clap(long = "remove-permission", value_name = "PERMISSION", num_args = 1..)]
    remove_permissions: Vec<String>,
    
    /// Feature to add to the manifest.
    #[clap(long = "add-feature", value_name = "FEATURE", num_args = 1..)]
    add_features: Vec<String>,
    
    /// Feature to remove from the manifest.
    #[clap(long = "remove-feature", value_name = "FEATURE", num_args = 1..)]
    remove_features: Vec<String>,
    
    /// Metadata to add to the manifest.
    #[clap(long = "add-metadata", value_name = "KEY=VALUE", num_args = 1.., value_parser = parse_key_val)]
    add_metadata: Vec<(String, String)>,
    
    /// Metadata to remove from the manifest.
    #[clap(long = "remove-metadata", value_name = "METADATA_KEY", num_args = 1..)]
    remove_metadata: Vec<String>,
    
    /// Native library to add to the manifest.
    #[clap(long = "add-library", value_name = "LIBRARY", num_args = 1..)]
    add_libraries: Vec<String>,
    
    /// Native library to remove from the manifest.
    #[clap(long = "remove-library", value_name = "LIBRARY", num_args = 1..)]
    remove_libraries: Vec<String>,
    
    /// Path to a replacement splash image shown at launch.
    #[clap(long)]
    vr_splash_path: Option<PathBuf>,
    
    /// Microphone Access
    #[clap(long = "microphone")]
    patch_microphone: bool,
    
    /// Passthrough to headset cameras
    #[clap(long = "passthrough")]
    patch_passthrough: bool,
    
    /// Body tracking support
    #[clap(long = "body-tracking")]
    patch_body_tracking: bool,
    
    /// Hand tracking support
    #[clap(long = "hand-tracking")]
    patch_hand_tracking: bool,
    
    /// Bluetooth support
    #[clap(long = "bluetooth")]
    patch_bluetooth: bool,
    
    /// MRC workaround
    #[clap(long)]
    mrc_workaround: bool,
}

#[cfg(feature = "cli")]
mod android_manifest;

#[cfg(feature = "cli")]
fn main() -> Result<()> {
    use android_manifest::AndroidManifest;
    use crate::handlers::mod_status::get_manifest_info_and_xml;
    use mbf_zip::ZipFile;
    
    let mut args = CliArgs::parse();
    
    let (_manifest_info, manifest_xml) = {
        let apk_reader = std::fs::File::open(&args.apk).context("Opening APK file")?;
        let mut apk = ZipFile::open(apk_reader).context("Reading APK as ZIP")?;

        get_manifest_info_and_xml(&mut apk)?
    };
    println!("Decoded manifest: {}", manifest_xml);
    let mut manifest = AndroidManifest::new(&manifest_xml).unwrap();
    println!("Parsed manifest: {}", manifest.to_string());
    
    manifest.apply_patching_manifest_mod();
    
    if args.patch_microphone {
        args.add_permissions.push("android.permission.RECORD_AUDIO".into());
    }
    
    if args.patch_passthrough {
        args.add_permissions.push("com.oculus.feature.PASSTHROUGH".into());
    }
    
    if args.patch_body_tracking {
        args.add_permissions.push("com.oculus.permission.BODY_TRACKING".into());
        args.add_features.push("com.oculus.software.body_tracking".into())
    }
    
    if args.patch_hand_tracking {
        args.add_permissions.push("com.oculus.permission.HAND_TRACKING".into());
        args.add_features.push("oculus.software.handtracking".into());
        args.add_metadata.push(("com.oculus.handtracking.frequency".into(), "MAX".into()));
        args.add_metadata.push(("com.oculus.handtracking.version".into(), "V2.0".into()));
    }
    
    if args.patch_bluetooth {
        args.add_permissions.push("android.permission.BLUETOOTH".into());
        args.add_permissions.push("android.permission.BLUETOOTH_CONNECT".into());
    }
    
    if args.mrc_workaround {
        args.add_libraries.push("libOVRMrcLib.oculus.so".into());
    }
    
    // Add the specified permissions.
    if !args.add_permissions.is_empty() {
        for perm in &args.add_permissions {
            if !manifest.has_permission(perm) {
                println!("Adding permission: {}", perm);
                manifest.add_permission(perm);
            } else {
                println!("Permission {} already exists in manifest, skipping addition", perm);
            }
        }
    }
    
    // Remove the specified permissions.
    if !args.remove_permissions.is_empty() {
        for perm in &args.remove_permissions {
            if manifest.has_permission(perm) {
                println!("Removing permission: {}", perm);
                manifest.remove_permission(perm);
            } else {
                println!("Permission {} not found in manifest, skipping removal", perm);
            }
        }
    }
    
    // Add the specified features.
    if !args.add_features.is_empty() {
        for feat in &args.add_features {
            if !manifest.has_feature(feat) {
                println!("Adding feature: {}", feat);
                manifest.add_feature(feat);
            } else {
                println!("Feature {} already exists in manifest, skipping addition", feat);
            }
        }
    }
    
    // Remove the specified features.
    if !args.remove_features.is_empty() {
        for feat in &args.remove_features {
            if manifest.has_feature(feat) {
                println!("Removing feature: {}", feat);
                manifest.remove_feature(feat);
            } else {
                println!("Feature {} not found in manifest, skipping removal", feat);
            }
        }
    }
    
    // Add the specified metadata.
    if !args.add_metadata.is_empty() {
        for (name, value) in &args.add_metadata {
            if !manifest.has_metadata(name) {
                println!("Adding metadata: {}={}", name, value);
                manifest.set_metadata(name, value);
            } else {
                println!("Metadata {} already exists in manifest, skipping addition", name);
            }
        }
    }
    
    // Remove the specified metadata.
    if !args.remove_metadata.is_empty() {
        for name in &args.remove_metadata {
            if manifest.has_metadata(name) {
                println!("Removing metadata: {}", name);
                manifest.remove_metadata(name);
            } else {
                println!("Metadata {} not found in manifest, skipping removal", name);
            }
        }
    }
    
    // Add the specified native libraries.
    if !args.add_libraries.is_empty() {
        for lib in &args.add_libraries {
            if !manifest.has_native_library(lib) {
                println!("Adding native library: {}", lib);
                manifest.add_native_library(lib);
            } else {
                println!("Native library {} already exists in manifest, skipping addition", lib);
            }
        }
    }
    
    // Remove the specified native libraries.
    if !args.remove_libraries.is_empty() {
        for lib in &args.remove_libraries {
            if manifest.has_native_library(lib) {
                println!("Removing native library: {}", lib);
                manifest.remove_native_library(lib);
            } else {
                println!("Native library {} not found in manifest, skipping removal", lib);
            }
        }
    }
    
    println!("Final manifest: {}", manifest.to_string());
    
    let vr_splash_path = {
      if (args.vr_splash_path.is_some() && args.vr_splash_path.as_ref().unwrap().exists()) {
          let leaked: &'static str = Box::leak(args.vr_splash_path.unwrap().to_string_lossy().to_string().into_boxed_str());
          
          Some(leaked)
      } else {
          None
      }
    };
    
    return patching::patch_apk_in_place(args.apk, args.libunity_path, manifest.to_string(), args.manifest_only, vr_splash_path)
        .context("Patching APK");
}