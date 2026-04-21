use std::io::Cursor;

use anyhow::{anyhow, Context, Result};
use log::{info, warn};
use mbf_axml::AxmlWriter;
use mbf_zip::{signing, FileCompression, ZipFile};
use serde::{Deserialize, Serialize};

use crate::{
    data_fix::fix_colour_schemes,
    downgrading,
    host::Host,
    models::response::{AppInfo, InstallStatus, ModLoader},
    resources::{self, ResCache},
};

const DEBUG_CERT_PEM: &[u8] = include_bytes!("../../mbf-agent/src/debug_cert.pem");
const LIB_MAIN: &[u8] = include_bytes!("../../mbf-agent/libs/libmain.so");
const MODLOADER: &[u8] = include_bytes!("../../mbf-agent/libs/libsl2.so");
const LEGACY_OVRPLATFORMLOADER: &[u8] = include_bytes!("../../mbf-agent/libs/libovrplatformloader.so");

const MODLOADER_NAME: &str = "libsl2.so";
const MOD_TAG_PATH: &str = "modded.json";

const LIB_MAIN_PATH: &str = "lib/arm64-v8a/libmain.so";
const LIB_UNITY_PATH: &str = "lib/arm64-v8a/libunity.so";
const LIB_OVR_PATH: &str = "lib/arm64-v8a/libovrplatformloader.so";

const STORE_ALIGNMENT: u16 = 4;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ModTag {
    pub patcher_name: String,
    pub patcher_version: Option<String>,
    pub modloader_name: String,
    pub modloader_version: Option<String>,
}

pub fn mod_beat_saber<H: Host>(
    host: &mut H,
    app_info: &AppInfo,
    downgrade_to: Option<String>,
    manifest_mod: String,
    manifest_only: bool,
    device_pre_v51: bool,
    vr_splash_path: Option<&str>,
    res_cache: &mut ResCache,
) -> Result<bool> {
    let config = host.get_config().clone();

    let libunity_path = if manifest_only {
        None
    } else {
        info!("Downloading unstripped libunity.so");
        save_libunity(host, res_cache, &config.temp_dir, downgrade_to.as_ref().unwrap_or(&app_info.version))
            .context("Preparing libunity.so")?
    };

    kill_app(host).context("Killing Beat Saber")?;

    info!("Copying APK to temporary location");
    let temp_apk_path = format!("{}/mbf-tmp.apk", config.temp_dir);
    host.copy_file(&app_info.path, &temp_apk_path)
        .context("Copying APK to temp")?;

    host.run_command("chmod", &["644", &temp_apk_path])
        .context("Marking APK as writable")?;

    info!("Saving OBB files");
    let obb_backup = format!("{}/obbs", config.temp_dir);
    host.create_dir_all(&obb_backup)?;
    let mut obb_backups = save_obbs(host, &config.obb_dir, &obb_backup, downgrade_to.is_none())
        .context("Saving OBB files")?;

    let contains_dlc = has_file_with_no_extension(host, &config.obb_dir)
        .context("Checking for DLC")?;

    if let Some(to_version) = downgrade_to.clone() {
        obb_backups = downgrading::get_and_apply_diff_sequence(
            host,
            &app_info.version,
            &to_version,
            &config.temp_dir,
            &temp_apk_path,
            obb_backups,
            res_cache,
        )
        .context("Downgrading game")?;
    }

    patch_and_reinstall(
        host,
        libunity_path,
        &temp_apk_path,
        obb_backups,
        manifest_mod,
        manifest_only,
        device_pre_v51,
        vr_splash_path,
    )
    .context("Patching and reinstalling APK")?;

    Ok(contains_dlc && downgrade_to.is_some())
}

pub fn kill_app<H: Host>(host: &mut H) -> Result<()> {
    info!("Killing Beat Saber");
    let apk_id = host.get_config().apk_id.clone();
    host.run_command("am", &["force-stop", &apk_id])?;
    Ok(())
}

fn patch_and_reinstall<H: Host>(
    host: &mut H,
    libunity_path: Option<String>,
    temp_apk_path: &str,
    obb_paths: Vec<String>,
    manifest_mod: String,
    manifest_only: bool,
    device_pre_v51: bool,
    vr_splash_path: Option<&str>,
) -> Result<()> {
    info!("Patching APK");
    patch_apk_in_place(host, temp_apk_path, libunity_path, manifest_mod, manifest_only, device_pre_v51, vr_splash_path)
        .context("Patching APK")?;

    let config = host.get_config().clone();
    if host.file_exists(&config.player_data_path) {
        info!("Backing up player data");
        backup_player_data(host).context("Backing up player data")?;
    }

    if host.file_exists(&config.datakeeper_player_data) {
        info!("Fixing colour schemes in backed up PlayerData.dat");
        match fix_colour_schemes(host, &config.datakeeper_player_data) {
            Ok(_) => {}
            Err(err) => warn!("Failed to fix colour schemes: {err}"),
        }
    }

    reinstall_modded_app(host, temp_apk_path).context("Reinstalling modded APK")?;
    host.remove_file(temp_apk_path)?;

    info!("Restoring OBB files");
    let obb_dir = host.get_config().obb_dir.clone();
    restore_obb_files(host, &obb_dir, obb_paths).context("Restoring OBB files")?;

    Ok(())
}

pub fn backup_player_data<H: Host>(host: &mut H) -> Result<()> {
    let config = host.get_config().clone();
    info!("Copying to {}", &config.aux_data_backup);

    if let Some(parent) = std::path::Path::new(&config.aux_data_backup).parent() {
        host.create_dir_all(&parent.to_string_lossy())?;
    }
    host.copy_file(&config.player_data_path, &config.aux_data_backup)?;

    if host.file_exists(&config.datakeeper_player_data) {
        warn!("Did not backup PlayerData.dat to datakeeper folder as there was already one there.");
    } else {
        info!("Copying to {}", &config.datakeeper_player_data);
        if let Some(parent) = std::path::Path::new(&config.datakeeper_player_data).parent() {
            host.create_dir_all(&parent.to_string_lossy())?;
        }
        host.copy_file(&config.player_data_path, &config.datakeeper_player_data)?;
    }

    Ok(())
}

fn reinstall_modded_app<H: Host>(host: &mut H, temp_apk_path: &str) -> Result<()> {
    info!("Reinstalling modded app");
    let apk_id = host.get_config().apk_id.clone();

    host.run_command("pm", &["uninstall", &apk_id])
        .context("Uninstalling vanilla APK")?;

    let is_pico = host
        .run_command("getprop", &["ro.product.manufacturer"])
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_lowercase().contains("pico"))
        .unwrap_or(false);

    if is_pico {
        host.run_command("pm", &["install", "-i", "com.picovr.store", temp_apk_path])
            .context("Installing modded APK")?;
    } else {
        host.run_command("pm", &["install", temp_apk_path])
            .context("Installing modded APK")?;
    }

    info!("Granting external storage permission");
    host.run_command("appops", &["set", "--uid", &apk_id, "MANAGE_EXTERNAL_STORAGE", "allow"])?;

    let sdk_output = host.run_command("getprop", &["ro.build.version.sdk"])
        .context("Getting Android SDK version")?;
    let sdk_str = String::from_utf8_lossy(&sdk_output.stdout).trim().to_string();
    let sdk_version: i32 = sdk_str.parse().unwrap_or(30);

    if sdk_version < 30 {
        info!("Granting external storage permissions (Quest 1)");
        host.run_command("pm", &["grant", &apk_id, "android.permission.WRITE_EXTERNAL_STORAGE"])?;
        host.run_command("pm", &["grant", &apk_id, "android.permission.READ_EXTERNAL_STORAGE"])?;
    }

    Ok(())
}

fn save_libunity<H: Host>(
    host: &mut H,
    res_cache: &mut ResCache,
    temp_dir: &str,
    version: &str,
) -> Result<Option<String>> {
    let apk_id = host.get_config().apk_id.clone();
    let url = match resources::get_libunity_url(host, res_cache, &apk_id, version)? {
        Some(url) => url,
        None => return Ok(None),
    };

    let libunity_path = format!("{}/libunity.so", temp_dir);
    host.http_get_file(&url, &libunity_path)
        .context("Downloading unstripped libunity.so")?;

    Ok(Some(libunity_path))
}

fn save_obbs<H: Host>(
    host: &mut H,
    obb_dir: &str,
    obb_backups_path: &str,
    include_dlc: bool,
) -> Result<Vec<String>> {
    let mut paths = Vec::new();
    for entry in host.list_dir(obb_dir).context("Listing OBB directory")? {
        if entry.is_dir {
            continue;
        }
        let path = &entry.path;
        let has_obb_ext = std::path::Path::new(path)
            .extension()
            .map(|e| e.eq_ignore_ascii_case("obb"))
            .unwrap_or(false);
        let no_ext = std::path::Path::new(path).extension().is_none();

        if (include_dlc || has_obb_ext) && (has_obb_ext || no_ext || include_dlc) {
            if let Some(file_name) = std::path::Path::new(path).file_name() {
                let obb_backup_path = format!("{}/{}", obb_backups_path, file_name.to_string_lossy());
                host.copy_file(path, &obb_backup_path)?;
                paths.push(obb_backup_path);
            }
        }
    }
    Ok(paths)
}

fn has_file_with_no_extension<H: Host>(host: &mut H, obb_dir: &str) -> Result<bool> {
    for entry in host.list_dir(obb_dir).unwrap_or_default() {
        if entry.is_dir {
            continue;
        }
        if std::path::Path::new(&entry.path).extension().is_none() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn restore_obb_files<H: Host>(
    host: &mut H,
    restore_dir: &str,
    obb_backups: Vec<String>,
) -> Result<()> {
    host.create_dir_all(restore_dir)?;
    for backup_path in obb_backups {
        if let Some(file_name) = std::path::Path::new(&backup_path).file_name() {
            let dest = format!("{}/{}", restore_dir, file_name.to_string_lossy());
            info!("Restoring {backup_path}");
            host.copy_file(&backup_path, &dest)?;
            host.remove_file(&backup_path)?;
        }
    }
    Ok(())
}

pub fn get_modloader_path<H: Host>(host: &mut H) -> Result<String> {
    let modloader_dir = host.get_config().modloader_dir.clone();
    host.create_dir_all(&modloader_dir)?;
    Ok(format!("{}/{}", modloader_dir, MODLOADER_NAME))
}

pub fn install_modloader<H: Host>(host: &mut H) -> Result<()> {
    let loader_path = get_modloader_path(host)?;
    info!("Installing modloader to {loader_path}");
    host.write_file(&loader_path, MODLOADER)?;
    Ok(())
}

pub fn get_modloader_status<H: Host>(host: &mut H) -> Result<InstallStatus> {
    let loader_path = get_modloader_path(host)?;
    info!("Checking if modloader is up to date");
    if host.file_exists(&loader_path) {
        let existing = host.read_file(&loader_path)
            .context("Reading existing modloader")?;
        if existing == MODLOADER {
            Ok(InstallStatus::Ready)
        } else {
            Ok(InstallStatus::NeedUpdate)
        }
    } else {
        Ok(InstallStatus::Missing)
    }
}

fn patch_apk_in_place<H: Host>(
    host: &mut H,
    path: &str,
    libunity_path: Option<String>,
    manifest_mod: String,
    manifest_only: bool,
    device_pre_v51: bool,
    vr_splash_path: Option<&str>,
) -> Result<()> {
    let mut zip = ZipFile::open_rw_path(path)
        .context("Opening temporary APK for writing")?;
    zip.set_store_alignment(STORE_ALIGNMENT);

    info!("Applying manifest mods");
    patch_manifest(&mut zip, manifest_mod).context("Patching manifest")?;

    let (priv_key, cert) = signing::load_cert_and_priv_key(DEBUG_CERT_PEM);

    if !manifest_only {
        info!("Adding libmainloader");
        zip.delete_file(LIB_MAIN_PATH);
        zip.write_file(LIB_MAIN_PATH, &mut Cursor::new(LIB_MAIN), FileCompression::Deflate)?;
        add_modded_tag(&mut zip, ModTag {
            patcher_name: "ModsBeforeFriday".to_string(),
            patcher_version: Some("0.1.0".to_string()),
            modloader_name: "Scotland2".to_string(),
            modloader_version: None,
        })?;

        info!("Adding unstripped libunity.so");
        match libunity_path {
            Some(unity_path) => {
                let unity_bytes = host.read_file(&unity_path)
                    .context("Opening unstripped libunity.so")?;
                zip.write_file(LIB_UNITY_PATH, &mut Cursor::new(unity_bytes), FileCompression::Deflate)?;
            }
            None => warn!("No unstripped unity added to the APK!"),
        }

        if device_pre_v51 {
            info!("Replacing ovrplatformloader");
            zip.write_file(LIB_OVR_PATH, &mut Cursor::new(LEGACY_OVRPLATFORMLOADER), FileCompression::Deflate)?;
        }
    }

    if let Some(splash_path) = vr_splash_path {
        info!("Applying custom splash screen");
        let splash_bytes = host.read_file(splash_path).context("Opening vr splash image")?;
        zip.write_file("assets/vr_splash.png", &mut Cursor::new(splash_bytes), FileCompression::Store)?;
    }

    info!("Signing");
    zip.save_and_sign_v2(&cert, &priv_key).context("Saving/signing APK")?;

    Ok(())
}

fn add_modded_tag<T: std::io::Read + std::io::Write + std::io::Seek>(
    to: &mut ZipFile<T>,
    tag: ModTag,
) -> Result<()> {
    let saved_tag = serde_json::to_vec_pretty(&tag)?;
    to.write_file(MOD_TAG_PATH, &mut Cursor::new(saved_tag), FileCompression::Deflate)?;
    Ok(())
}

pub fn get_modloader_installed<T: std::io::Read + std::io::Seek>(
    apk: &mut ZipFile<T>,
) -> Result<Option<ModLoader>> {
    if apk.contains_file(MOD_TAG_PATH) {
        let tag_data = apk.read_file(MOD_TAG_PATH).context("Reading mod tag")?;
        let mod_tag: ModTag = match serde_json::from_slice(&tag_data) {
            Ok(tag) => tag,
            Err(err) => {
                warn!("Mod tag was invalid JSON: {err}... Assuming unknown modloader");
                return Ok(Some(ModLoader::Unknown));
            }
        };

        Ok(Some(
            if mod_tag.modloader_name.eq_ignore_ascii_case("QuestLoader") {
                ModLoader::QuestLoader
            } else if mod_tag.modloader_name.eq_ignore_ascii_case("Scotland2") {
                ModLoader::Scotland2
            } else {
                ModLoader::Unknown
            },
        ))
    } else if apk.iter_entry_names().any(|e| e.contains("modded")) {
        Ok(Some(ModLoader::Unknown))
    } else {
        Ok(None)
    }
}

pub fn check_obb_present<H: Host>(host: &mut H) -> Result<bool> {
    let obb_dir = host.get_config().obb_dir.clone();
    if !host.file_exists(&obb_dir) {
        return Ok(false);
    }

    Ok(host.list_dir(&obb_dir).unwrap_or_default().iter().any(|e| {
        !e.is_dir
            && std::path::Path::new(&e.path)
                .extension()
                .map(|ext| ext.eq_ignore_ascii_case("obb"))
                .unwrap_or(false)
    }))
}

fn patch_manifest<T: std::io::Read + std::io::Write + std::io::Seek>(
    zip: &mut ZipFile<T>,
    additional_properties: String,
) -> Result<()> {
    let mut xml_reader = xml::EventReader::new(Cursor::new(additional_properties.as_bytes()));
    let mut data_output = Cursor::new(Vec::new());
    let mut axml_writer = AxmlWriter::new(&mut data_output);

    mbf_axml::xml_to_axml(&mut axml_writer, &mut xml_reader)
        .context("Converting XML back to AXML")?;
    axml_writer.finish().context("Saving AXML manifest")?;

    zip.delete_file("AndroidManifest.xml");
    zip.write_file("AndroidManifest.xml", &mut data_output, FileCompression::Deflate)
        .context("Writing modified manifest")?;

    Ok(())
}

pub fn get_apk_path<H: Host>(host: &mut H) -> Result<Option<String>> {
    let apk_id = host.get_config().apk_id.clone();
    let pm_output = host
        .run_command("pm", &["path", &apk_id])
        .context("Working out APK path")?;
    if 8 > pm_output.stdout.len() {
        Ok(None)
    } else {
        Ok(Some(
            std::str::from_utf8(pm_output.stdout.split_at(8).1)?
                .trim_end()
                .to_owned(),
        ))
    }
}
