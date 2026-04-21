use std::io::Cursor;

use anyhow::{anyhow, Context, Result};
use log::{error, info, warn};
use mbf_axml::AxmlReader;
use mbf_zip::ZipFile;

use crate::{
    downgrading,
    host::Host,
    manifest::ManifestInfo,
    mod_man::ModManager,
    models::response::{self, CoreModsInfo, Response},
    patching,
    resources::{self, CoreMod, ResCache},
};

pub fn handle_get_mod_status<H: Host>(
    host: &mut H,
    override_core_mod_url: Option<String>,
) -> Result<Response> {
    try_delete_legacy_dirs(host);

    let res_cache_dir = host.get_config().res_cache_dir.clone();
    host.create_dir_all(&res_cache_dir)?;
    let mut res_cache = ResCache::new(res_cache_dir);

    info!("Searching for Beat Saber app");
    let app_info = get_app_info(host)?;

    let (core_mods, installed_mods) = match &app_info {
        Some(app_info) => {
            info!("Loading installed mods");
            let qmods_dir_template = host.get_config().qmods_dir_template.clone();
            let mut mod_manager = ModManager::new(app_info.version.clone(), ResCache::new(host.get_config().res_cache_dir.clone()), &qmods_dir_template);
            mod_manager.load_mods(host).context("Loading installed mods")?;

            (
                get_core_mods_info(
                    host,
                    &app_info.version,
                    &mod_manager,
                    override_core_mod_url,
                    &mut res_cache,
                    app_info.loader_installed.is_some(),
                )?,
                super::mod_management::get_mod_models(mod_manager)?,
            )
        }
        None => {
            warn!("Beat Saber is not installed!");
            (None, Vec::new())
        }
    };

    Ok(Response::ModStatus {
        app_info,
        core_mods,
        modloader_install_status: patching::get_modloader_status(host)?,
        installed_mods,
    })
}

pub fn get_app_info<H: Host>(host: &mut H) -> Result<Option<response::AppInfo>> {
    let apk_path = match patching::get_apk_path(host).context("Finding APK path")? {
        Some(path) => path,
        None => return Ok(None),
    };

    let apk_bytes = host.read_file(&apk_path)?;
    let mut apk = ZipFile::open(Cursor::new(apk_bytes)).context("Reading APK as ZIP")?;

    let modloader = patching::get_modloader_installed(&mut apk)?;
    let obb_present = patching::check_obb_present(host)?;

    let (manifest_info, manifest_xml) = get_manifest_info_and_xml(&mut apk)?;
    Ok(Some(response::AppInfo {
        loader_installed: modloader,
        version: manifest_info.package_version,
        obb_present,
        path: apk_path,
        manifest_xml,
    }))
}

fn get_manifest_info_and_xml<T: std::io::Read + std::io::Seek>(
    apk: &mut ZipFile<T>,
) -> Result<(ManifestInfo, String)> {
    let manifest = apk
        .read_file("AndroidManifest.xml")
        .context("Reading manifest from APK")?;

    let mut manifest_reader = Cursor::new(&manifest);
    let mut axml_reader = AxmlReader::new(&mut manifest_reader)?;
    let manifest_info =
        ManifestInfo::read(&mut axml_reader).context("Parsing manifest from AXML")?;

    let xml_str = axml_bytes_to_xml_string(&manifest).context("Converting manifest to XML")?;

    Ok((manifest_info, xml_str))
}

pub fn axml_bytes_to_xml_string(bytes: &[u8]) -> Result<String> {
    let mut cursor = Cursor::new(bytes);
    let mut axml_reader = AxmlReader::new(&mut cursor)?;

    let mut xml_output = Vec::new();
    let mut xml_writer = xml::EmitterConfig::new()
        .perform_indent(true)
        .create_writer(Cursor::new(&mut xml_output));

    mbf_axml::axml_to_xml(&mut xml_writer, &mut axml_reader).context("Converting AXML to XML")?;

    Ok(String::from_utf8(xml_output).expect("XML output should be valid UTF-8"))
}

fn get_core_mods_info<H: Host>(
    host: &mut H,
    apk_version: &str,
    mod_manager: &ModManager,
    override_core_mod_url: Option<String>,
    res_cache: &mut ResCache,
    is_patched: bool,
) -> Result<Option<CoreModsInfo>> {
    info!("Fetching core mod index");
    let core_mods = match resources::fetch_core_mods(host, res_cache, override_core_mod_url) {
        Ok(mods) => mods,
        Err(resources::JsonPullError::FetchError(e)) => {
            error!("Failed to fetch core mod index: assuming no internet connection: {e:?}");
            return Ok(None);
        }
        Err(resources::JsonPullError::ParseError(e)) => return Err(e),
    };

    let all_core_mods_installed = match core_mods.get(apk_version) {
        Some(cm) => get_core_mods_install_status(&cm.mods, mod_manager),
        None => response::InstallStatus::Missing,
    };

    let supported_versions: Vec<String> = core_mods
        .into_keys()
        .filter(|version| {
            let mut iter = version.split('.');
            let _major = iter.next().unwrap();
            let minor = iter.next().unwrap_or("0");
            minor.parse::<i64>().unwrap_or(0) >= 35
        })
        .collect();

    let is_version_supported = supported_versions.iter().any(|ver| ver == apk_version);

    let (downgrade_versions, newer_than_latest_diff) = if is_patched {
        (Vec::new(), false)
    } else {
        let diff_index_map = downgrading::get_all_accessible_versions(host, res_cache, apk_version)
            .context("Formatting downgrading information")?;

        let diff_list: resources::DiffIndex = resources::get_diff_index(host, res_cache)
            .unwrap_or_default();
        let newer_than_latest = is_version_newer_than_latest_diff(apk_version, &diff_list);
        (diff_index_map.into_keys().collect(), newer_than_latest)
    };

    Ok(Some(CoreModsInfo {
        supported_versions,
        core_mod_install_status: all_core_mods_installed,
        downgrade_versions,
        is_awaiting_diff: newer_than_latest_diff && !is_version_supported,
    }))
}

fn is_version_newer_than_latest_diff(
    apk_version: &str,
    diffs: &[resources::VersionDiffs],
) -> bool {
    let sem_apk_ver = match try_parse_bs_ver_as_semver(apk_version) {
        Some(ver) => ver,
        None => {
            warn!("APK version {apk_version} did not have a valid semver section");
            return false;
        }
    };

    for diff in diffs {
        if let Some(diff_version) = try_parse_bs_ver_as_semver(&diff.from_version) {
            if sem_apk_ver <= diff_version {
                return false;
            }
        }
    }

    true
}

fn try_parse_bs_ver_as_semver(version: &str) -> Option<semver::Version> {
    let segment = version.split('_').next()?;
    semver::Version::parse(segment).ok()
}

fn get_core_mods_install_status(
    core_mods: &[CoreMod],
    mod_man: &ModManager,
) -> response::InstallStatus {
    info!("Checking if core mods installed and up to date");
    mark_all_core_mods(mod_man, core_mods);

    let mut missing = false;
    let mut outdated = false;
    for core_mod in core_mods {
        match mod_man.get_mod(&core_mod.id) {
            Some(existing_mod) => {
                let mod_ref = existing_mod.borrow();
                if !mod_ref.installed() {
                    warn!("Core mod {} was present but not installed", core_mod.id);
                    missing = true;
                } else if mod_ref.manifest().version < core_mod.version {
                    warn!("Core mod {} is outdated", core_mod.id);
                    outdated = true;
                }
            }
            None => missing = true,
        }
    }

    if missing {
        response::InstallStatus::Missing
    } else if outdated {
        response::InstallStatus::NeedUpdate
    } else {
        response::InstallStatus::Ready
    }
}

pub fn mark_all_core_mods(mod_manager: &ModManager, core_mods: &[CoreMod]) {
    for core_mod in core_mods {
        mod_manager.set_mod_core(&core_mod.id);
    }
}

pub fn try_delete_legacy_dirs<H: Host>(host: &mut H) {
    let legacy_dirs = host.get_config().legacy_dirs.clone();
    for dir in &legacy_dirs {
        if host.file_exists(dir) {
            match host.remove_dir_all(dir) {
                Ok(_) => log::debug!("Successfully removed legacy dir {dir}"),
                Err(err) => warn!("Failed to remove legacy dir {dir}: {err}"),
            }
        }
    }
}

pub fn install_core_mods<H: Host>(
    host: &mut H,
    res_cache: &mut ResCache,
    mod_manager: &mut ModManager,
    app_info: response::AppInfo,
    override_core_mod_url: Option<String>,
) -> Result<()> {
    info!("Preparing core mods");
    let core_mod_index = resources::fetch_core_mods(host, res_cache, override_core_mod_url)?;

    let core_mods = core_mod_index
        .get(&app_info.version)
        .ok_or(anyhow!("No core mods existed for {}", app_info.version))?;

    for core_mod in &core_mods.mods {
        match mod_manager.get_mod(&core_mod.id) {
            Some(existing) => {
                let existing_ref = existing.borrow();
                if existing_ref.manifest().version >= core_mod.version {
                    info!(
                        "Core mod {} already installed with sufficient version",
                        core_mod.id
                    );
                    continue;
                }
            }
            None => {}
        }

        info!("Downloading {} v{}", core_mod.id, core_mod.version);
        let core_mod_vec = host
            .http_get(&core_mod.download_url)
            .context("Downloading core mod")?;
        mod_manager.try_load_new_mod(host, Cursor::new(core_mod_vec))?;
    }

    info!("Installing core mods");
    mod_manager.load_mods(host).context("Loading core mods")?;
    let core_mods_list: Vec<CoreMod> = core_mods.mods.clone();
    for core_mod in &core_mods_list {
        mod_manager.install_mod(host, &core_mod.id)?;
    }
    mark_all_core_mods(mod_manager, &core_mods_list);

    Ok(())
}

pub fn get_app_version_only<H: Host>(host: &mut H) -> Result<String> {
    let apk_id = host.get_config().apk_id.clone();
    let output = host
        .run_command("dumpsys", &["package", &apk_id])
        .context("Invoking dumpsys")?;
    let stdout = String::from_utf8(output.stdout).context("Converting dumpsys output to UTF-8")?;

    let version_offset = match stdout.find("versionName=") {
        Some(offset) => offset,
        None => return Err(anyhow!("Beat Saber was not installed")),
    } + 12;

    let newline_offset = version_offset
        + stdout[version_offset..]
            .find('\n')
            .ok_or(anyhow!("No newline after version name"))?;

    Ok(stdout[version_offset..newline_offset].trim().to_string())
}
