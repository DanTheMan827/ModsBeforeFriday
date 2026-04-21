use std::{
    ffi::OsStr, io::{Cursor, Read, Seek, Write}, path::{Path, PathBuf}
};

use crate::{
    data_fix::fix_colour_schemes, downgrading, downloads, models::response::{AppInfo, InstallStatus, ModLoader}, parameters::PARAMETERS, ModTag
};
use mbf_axml::{self, AxmlWriter};
use anyhow::{Context, Result};
use log::{debug, info, warn};
use mbf_res_man::{
    external_res,
    res_cache::ResCache,
};
use mbf_zip::{signing, FileCompression, WriteSeekLen, ZipFile};

const DEBUG_CERT_PEM: &[u8] = include_bytes!("debug_cert.pem");
const LIB_MAIN: &[u8] = include_bytes!("../libs/libmain.so");
const MODLOADER: &[u8] = include_bytes!("../libs/libsl2.so");
const LEGACY_OVRPLATFORMLOADER: &[u8] = include_bytes!("../libs/libovrplatformloader.so");

const MODLOADER_NAME: &str = "libsl2.so";
const MOD_TAG_PATH: &str = "modded.json";

const LIB_MAIN_PATH: &str = "lib/arm64-v8a/libmain.so";
const LIB_UNITY_PATH: &str = "lib/arm64-v8a/libunity.so";
const LIB_OVR_PATH: &str = "lib/arm64-v8a/libovrplatformloader.so";

const STORE_ALIGNMENT: u16 = 4;

pub fn mod_beat_saber(
    temp_path: &Path,
    app_info: &AppInfo,
    downgrade_to: Option<String>,
    manifest_mod: String,
    manifest_only: bool,
    device_pre_v51: bool,
    vr_splash_path: Option<&str>,
    res_cache: &ResCache,
) -> Result<bool> {
    let libunity_path = if manifest_only {
        None
    } else {
        info!("Downloading unstripped libunity.so (this could take a minute)");
        save_libunity(res_cache, temp_path, downgrade_to.as_ref().unwrap_or(&app_info.version))
            .context("Preparing libunity.so")?
    };

    kill_app().context("Killing Beat Saber")?;

    info!("Copying APK to temporary location");
    let temp_apk_path = temp_path.join("mbf-tmp.apk");
    crate::hal().copy_file(Path::new(&app_info.path), &temp_apk_path).context("Copying APK to temp")?;

    info!("Marking APK as writable");
    crate::hal().set_permissions_writable(&temp_apk_path).context("Marking APK as writable")?;

    info!("Saving OBB files");
    let obb_backup = temp_path.join("obbs");
    crate::hal().create_dir_all(&obb_backup)?;
    let mut obb_backups =
        save_obbs(Path::new(&PARAMETERS.obb_dir), &obb_backup,
        downgrade_to.is_none()).context("Saving OBB files")?;

    let contains_dlc = has_file_with_no_extension(&PARAMETERS.obb_dir).context("Checking for DLC")?;

    if let Some(to_version) = downgrade_to.clone() {
        obb_backups = downgrading::get_and_apply_diff_sequence(&app_info.version,
            &to_version,
            temp_path,
            &temp_apk_path,
            obb_backups, res_cache).context("Downgrading game")?;
    }

    patch_and_reinstall(
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

fn has_file_with_no_extension(obb_dir: impl AsRef<Path>) -> Result<bool> {
    for path in crate::hal().read_dir_paths(obb_dir.as_ref())? {
        if path.extension().is_none() {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn kill_app() -> Result<()> {
    info!("Killing Beat Saber");
    crate::hal().exec_command("am", &["force-stop", &PARAMETERS.apk_id])?;
    Ok(())
}

fn patch_and_reinstall(
    libunity_path: Option<PathBuf>,
    temp_apk_path: &Path,
    obb_paths: Vec<PathBuf>,
    manifest_mod: String,
    manifest_only: bool,
    device_pre_v51: bool,
    vr_splash_path: Option<&str>,
) -> Result<()> {
    info!("Patching APK");
    patch_apk_in_place(
        &temp_apk_path,
        libunity_path,
        manifest_mod,
        manifest_only,
        device_pre_v51,
        vr_splash_path,
    )
    .context("Patching APK")?;

    if crate::hal().path_exists(Path::new(&PARAMETERS.player_data)) {
        info!("Backing up player data");
        backup_player_data().context("Backing up player data")?;
    } else {
        info!("No player data to backup");
    }

    if crate::hal().path_exists(Path::new(&PARAMETERS.datakeeper_player_data)) {
        info!("Fixing colour schemes in backed up PlayerData.dat");
        match fix_colour_schemes(&PARAMETERS.datakeeper_player_data) {
            Ok(_) => {}
            Err(err) => warn!("Failed to fix colour schemes: {err}"),
        }
    }

    reinstall_modded_app(&temp_apk_path).context("Reinstalling modded APK")?;
    crate::hal().remove_file(temp_apk_path)?;

    info!("Restoring OBB files");
    restore_obb_files(Path::new(&PARAMETERS.obb_dir), obb_paths).context("Restoring OBB files")?;

    Ok(())
}

pub fn backup_player_data() -> Result<()> {
    info!("Copying to {}", &PARAMETERS.aux_data_backup);

    crate::hal().create_dir_all(Path::new(&PARAMETERS.aux_data_backup).parent().unwrap())?;
    crate::hal().copy_file(Path::new(&PARAMETERS.player_data), Path::new(&PARAMETERS.aux_data_backup))?;

    if crate::hal().path_exists(Path::new(&PARAMETERS.datakeeper_player_data)) {
        warn!("Did not backup PlayerData.dat to datakeeper folder as there was already a PlayerData.dat there.
            The player data is still safe in {}", &PARAMETERS.aux_data_backup);
    } else {
        info!("Copying to {}", &PARAMETERS.datakeeper_player_data);
        crate::hal().create_dir_all(Path::new(&PARAMETERS.datakeeper_player_data).parent().unwrap())?;
        crate::hal().copy_file(Path::new(&PARAMETERS.player_data), Path::new(&PARAMETERS.datakeeper_player_data))?;
    }

    Ok(())
}

fn reinstall_modded_app(
    temp_apk_path: &Path
) -> Result<()> {
    info!("Reinstalling modded app");
    crate::hal().exec_command("pm", &["uninstall", &PARAMETERS.apk_id])
        .context("Uninstalling vanilla APK")?;

    let is_pico = get_android_device_manufacturer()
        .map(|v| v.to_lowercase().contains("pico"))
        .unwrap_or(false);

    if is_pico {
        crate::hal().exec_command("pm", &[
            "install",
            "-i", "com.picovr.store",
            &temp_apk_path.to_string_lossy()
        ])
        .context("Installing modded APK")?;
    } else {
        crate::hal().exec_command("pm", &["install", &temp_apk_path.to_string_lossy()])
            .context("Installing modded APK")?;
    }

    info!("Granting external storage permission");
    crate::hal().exec_command("appops", &["set", "--uid", &PARAMETERS.apk_id, "MANAGE_EXTERNAL_STORAGE", "allow"])?;

    let sdk_version = get_android_sdk_version()?;
    if sdk_version < 30 {
        info!("Granting WRITE_EXTERNAL_STORAGE and READ_EXTERNAL_STORAGE (Quest 1)");
        crate::hal().exec_command("pm", &["grant", &PARAMETERS.apk_id, "android.permission.WRITE_EXTERNAL_STORAGE"])?;
        crate::hal().exec_command("pm", &["grant", &PARAMETERS.apk_id, "android.permission.READ_EXTERNAL_STORAGE"])?;
    }

    Ok(())
}

fn save_libunity(
    res_cache: &ResCache,
    temp_path: impl AsRef<Path>,
    version: &str,
) -> Result<Option<PathBuf>> {
    let url = match external_res::get_libunity_url(res_cache, &PARAMETERS.apk_id, version)? {
        Some(url) => url,
        None => return Ok(None),
    };

    let libunity_path = temp_path.as_ref().join("libunity.so");
    downloads::download_file_with_attempts(&crate::get_dl_cfg(), &libunity_path, &url)
        .context("Downloading unstripped libunity.so")?;

    Ok(Some(libunity_path))
}

fn save_obbs(obb_dir: &Path, obb_backups_path: &Path, include_dlc: bool) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for path in crate::hal().read_dir_paths(obb_dir)? {
        if (include_dlc || path.extension() == Some(OsStr::new("obb"))) && !crate::hal().is_dir(&path) {
            debug!("Saving OBB path {:?}", path);
            let obb_backup_path = obb_backups_path.join(path.file_name().unwrap());
            crate::hal().copy_file(&path, &obb_backup_path)?;
            paths.push(obb_backup_path);
        }
    }
    Ok(paths)
}

fn restore_obb_files(restore_dir: &Path, obb_backups: Vec<PathBuf>) -> Result<()> {
    crate::hal().create_dir_all(restore_dir)?;
    for backup_path in obb_backups {
        info!("Restoring {:?}", backup_path);
        crate::hal().copy_file(
            &backup_path,
            &restore_dir.join(backup_path.file_name().unwrap()),
        )?;
        crate::hal().remove_file(&backup_path)?;
    }
    Ok(())
}

pub fn get_modloader_path() -> Result<PathBuf> {
    let modloaders_path = format!("{}/", &PARAMETERS.modloader_dir);
    crate::hal().create_dir_all(Path::new(&modloaders_path))?;
    Ok(PathBuf::from(modloaders_path).join(MODLOADER_NAME))
}

pub fn install_modloader() -> Result<()> {
    let loader_path = get_modloader_path()?;
    info!("Installing modloader to {loader_path:?}");
    crate::hal().write_file(&loader_path, MODLOADER)?;
    Ok(())
}

pub fn get_modloader_status() -> Result<InstallStatus> {
    let loader_path = get_modloader_path()?;

    info!("Checking if modloader is up to date");
    if crate::hal().path_exists(&loader_path) {
        let existing_loader_bytes = crate::hal().read_file(&loader_path)
            .context("Reading existing modloader to check if up to date")?;

        if existing_loader_bytes == MODLOADER {
            Ok(InstallStatus::Ready)
        } else {
            Ok(InstallStatus::NeedUpdate)
        }
    } else {
        Ok(InstallStatus::Missing)
    }
}

fn patch_apk_in_place(
    path: impl AsRef<Path>,
    libunity_path: Option<PathBuf>,
    manifest_mod: String,
    manifest_only: bool,
    device_pre_v51: bool,
    vr_splash_path: Option<&str>,
) -> Result<()> {
    let file = crate::hal().open_file_rw(path.as_ref())
        .context("Opening temporary APK for writing")?;

    let mut zip = ZipFile::open(file).unwrap();
    zip.set_store_alignment(STORE_ALIGNMENT);

    info!("Applying manifest mods");
    patch_manifest(&mut zip, manifest_mod).context("Patching manifest")?;

    let (priv_key, cert) = signing::load_cert_and_priv_key(DEBUG_CERT_PEM);

    if !manifest_only {
        info!("Adding libmainloader");
        zip.delete_file(LIB_MAIN_PATH);
        zip.write_file(
            LIB_MAIN_PATH,
            &mut Cursor::new(LIB_MAIN),
            FileCompression::Deflate,
        )?;
        add_modded_tag(
            &mut zip,
            ModTag {
                patcher_name: "ModsBeforeFriday".to_string(),
                patcher_version: Some("0.1.0".to_string()),
                modloader_name: "Scotland2".to_string(),
                modloader_version: None,
            },
        )?;

        info!("Adding unstripped libunity.so (this may take up to a minute)");
        match libunity_path {
            Some(unity_path) => {
                let unity_bytes = crate::hal().read_file(&unity_path).context("Opening unstripped libunity.so")?;
                zip.write_file(LIB_UNITY_PATH, &mut Cursor::new(unity_bytes), FileCompression::Deflate)?;
            }
            None => warn!("No unstripped unity added to the APK! This might cause issues later"),
        }

        if device_pre_v51 {
            info!("Replacing ovrplatformloader");
            zip.write_file(
                LIB_OVR_PATH,
                &mut Cursor::new(LEGACY_OVRPLATFORMLOADER),
                FileCompression::Deflate
            )?;
        }
    }

    if let Some(splash_path) = vr_splash_path {
        info!("Applying custom splash screen");
        let vr_splash_bytes = crate::hal().read_file(Path::new(splash_path)).context("Opening vr splash image")?;
        zip.write_file(
            "assets/vr_splash.png",
            &mut Cursor::new(vr_splash_bytes),
            FileCompression::Store,
        )?;
    }

    info!("Signing");
    zip.save_and_sign_v2(&cert, &priv_key)
        .context("Saving/signing APK")?;

    Ok(())
}

fn add_modded_tag<R: WriteSeekLen>(to: &mut ZipFile<R>, tag: ModTag) -> Result<()> {
    let saved_tag = serde_json::to_vec_pretty(&tag)?;
    to.write_file(
        MOD_TAG_PATH,
        &mut Cursor::new(saved_tag),
        FileCompression::Deflate,
    )?;
    Ok(())
}

pub fn get_modloader_installed<R: Read + Seek>(apk: &mut ZipFile<R>) -> Result<Option<ModLoader>> {
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
    } else if apk.iter_entry_names().any(|entry| entry.contains("modded")) {
        Ok(Some(ModLoader::Unknown))
    } else {
        Ok(None)
    }
}

pub fn check_obb_present() -> Result<bool> {
    if !crate::hal().path_exists(Path::new(&PARAMETERS.obb_dir)) {
        return Ok(false);
    }

    Ok(crate::hal().read_dir_paths(Path::new(&PARAMETERS.obb_dir))?.iter().any(|path| {
        path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("obb"))
    }))
}

fn patch_manifest<R: WriteSeekLen>(zip: &mut ZipFile<R>, additional_properties: String) -> Result<()> {
    let mut xml_reader = xml::EventReader::new(Cursor::new(additional_properties.as_bytes()));

    let mut data_output = Cursor::new(Vec::new());
    let mut axml_writer = AxmlWriter::new(&mut data_output);

    mbf_axml::xml_to_axml(&mut axml_writer, &mut xml_reader)
        .context("Converting XML back to (binary) AXML")?;
    axml_writer
        .finish()
        .context("Saving AXML (binary) manifest")?;

    zip.delete_file("AndroidManifest.xml");
    zip.write_file(
        "AndroidManifest.xml",
        &mut data_output,
        FileCompression::Deflate,
    )
    .context("Writing modified manifest")?;

    Ok(())
}

fn get_android_sdk_version() -> Result<i32> {
    let output = crate::hal().exec_command("getprop", &["ro.build.version.sdk"])
        .context("Getting Android SDK version")?;

    let sdk_version_str = String::from_utf8_lossy(&output).trim().to_string();
    let sdk_version: i32 = sdk_version_str.parse().context("Parsing Android SDK version number")?;
    Ok(sdk_version)
}

fn get_android_device_manufacturer() -> Result<String> {
    let output = crate::hal().exec_command("getprop", &["ro.product.manufacturer"])
        .context("Getting Android device manufacturer")?;

    let manufacturer = String::from_utf8_lossy(&output).trim().to_string();
    Ok(manufacturer)
}
