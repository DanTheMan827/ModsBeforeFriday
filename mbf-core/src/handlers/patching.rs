use anyhow::{anyhow, Context, Result};
use log::{info, warn};

use crate::{
    host::Host,
    mod_man::ModManager,
    models::response::Response,
    resources::ResCache,
};

pub fn handle_get_downgraded_manifest<H: Host>(host: &mut H, version: String) -> Result<Response> {
    info!("Downloading manifest AXML file");
    let manifest_bytes = crate::resources::get_manifest_axml(host, &version)
        .context("HTTP GET for downgraded AndroidManifest.xml")?;
    info!("Converting into readable XML");
    let manifest_xml = super::mod_status::axml_bytes_to_xml_string(&manifest_bytes)?;

    Ok(Response::DowngradedManifest { manifest_xml })
}

pub fn handle_patch<H: Host>(
    host: &mut H,
    downgrade_to: Option<String>,
    remodding: bool,
    manifest_mod: String,
    device_pre_v51: bool,
    allow_no_core_mods: bool,
    override_core_mod_url: Option<String>,
    vr_splash_path: Option<String>,
) -> Result<Response> {
    let app_info = super::mod_status::get_app_info(host)?
        .ok_or_else(|| anyhow!("Cannot patch when app not installed"))?;

    let temp_dir = host.get_config().temp_dir.clone();
    host.create_dir_all(&temp_dir)?;

    let res_cache_dir = host.get_config().res_cache_dir.clone();
    let mut res_cache = ResCache::new(res_cache_dir);

    let patching_result = crate::patching::mod_beat_saber(
        host,
        &app_info,
        downgrade_to.clone(),
        manifest_mod,
        remodding,
        device_pre_v51,
        vr_splash_path.as_deref(),
        &mut res_cache,
    )
    .context("Modding the game");

    // Clean up temp and splash regardless of outcome.
    let _ = host.remove_dir_all(&temp_dir);
    if let Some(ref splash_path) = vr_splash_path {
        let _ = host.remove_file(splash_path);
    }

    let removed_dlc = patching_result?;
    crate::patching::install_modloader(host).context("Installing external modloader")?;

    let new_app_version = downgrade_to.unwrap_or(app_info.version);
    let qmods_dir_template = host.get_config().qmods_dir_template.clone();
    let mut mod_manager = ModManager::new(
        new_app_version,
        ResCache::new(host.get_config().res_cache_dir.clone()),
        &qmods_dir_template,
    );

    if !remodding {
        info!("Wiping all existing mods");
        mod_manager
            .wipe_all_mods(host)
            .context("Wiping existing mods")?;
        mod_manager.load_mods(host)?;

        let new_app_info = super::mod_status::get_app_info(host)?
            .ok_or_else(|| anyhow!("Beat Saber should be installed after patching"))?;

        match super::mod_status::install_core_mods(
            host,
            &mut res_cache,
            &mut mod_manager,
            new_app_info,
            override_core_mod_url,
        ) {
            Ok(_) => info!("Successfully installed all core mods"),
            Err(err) => {
                if allow_no_core_mods {
                    warn!("Failed to install core mods: {err}")
                } else {
                    return Err(err).context("Installing core mods");
                }
            }
        }
    }

    Ok(Response::Patched {
        installed_mods: super::mod_management::get_mod_models(mod_manager)?,
        did_remove_dlc: removed_dlc,
    })
}
