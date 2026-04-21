//! Responsible for handling all requests sent to the backend from the frontend.

use std::io::Cursor;

use crate::{
    downloads,
    mod_man::ModManager,
    models::{
        request::Request,
        request::RequestEnum,
        response::{self, Response},
    }, parameters::PARAMETERS,
};
use anyhow::{anyhow, Context, Result};
use log::info;
use mbf_res_man::res_cache::ResCache;

mod import;
mod mod_management;
mod mod_status;
mod patching;
mod utility;

pub fn handle_request(request: Request) -> Result<Response> {
    match request.request {
        RequestEnum::GetModStatus {
            override_core_mod_url,
        } => mod_status::handle_get_mod_status(override_core_mod_url),
        RequestEnum::Patch {
            downgrade_to,
            remodding,
            manifest_mod,
            allow_no_core_mods,
            device_pre_v51,
            override_core_mod_url,
            vr_splash_path,
        } => patching::handle_patch(
            downgrade_to,
            remodding,
            manifest_mod,
            device_pre_v51,
            allow_no_core_mods,
            override_core_mod_url,
            vr_splash_path,
        ),
        RequestEnum::GetDowngradedManifest { version } => {
            patching::handle_get_downgraded_manifest(version)
        }
        RequestEnum::RemoveMod { id } => mod_management::handle_remove_mod(id),
        RequestEnum::SetModsEnabled { statuses } => mod_management::handle_set_mods_enabled(statuses),
        RequestEnum::Import { from_path } => import::handle_import(from_path, None),
        RequestEnum::ImportUrl { from_url } => import::handle_import_mod_url(from_url),
        RequestEnum::FixPlayerData => utility::handle_fix_player_data(),
        RequestEnum::QuickFix {
            override_core_mod_url,
            wipe_existing_mods,
        } => utility::handle_quick_fix(override_core_mod_url, wipe_existing_mods),
    }
}

fn get_app_version_only() -> Result<String> {
    let stdout = crate::hal().exec_command("dumpsys", &["package", &PARAMETERS.apk_id])
        .context("Invoking dumpsys")?;
    let dumpsys_stdout =
        String::from_utf8(stdout).context("Converting dumpsys output to UTF-8")?;

    let version_offset = match dumpsys_stdout.find("versionName=") {
        Some(offset) => offset,
        None => return Err(anyhow!("Beat Saber was not installed")),
    } + 12;

    let newline_offset = version_offset
        + dumpsys_stdout[version_offset..]
            .find('\n')
            .ok_or(anyhow!("No newline after version name"))?;

    Ok(dumpsys_stdout[version_offset..newline_offset]
        .trim()
        .to_string())
}

fn install_core_mods(
    res_cache: &ResCache,
    mod_manager: &mut ModManager,
    app_info: response::AppInfo,
    override_core_mod_url: Option<String>,
) -> Result<()> {
    info!("Preparing core mods");
    let core_mod_index =
        mbf_res_man::external_res::fetch_core_mods(&res_cache, override_core_mod_url)?;

    let core_mods = core_mod_index
        .get(&app_info.version)
        .ok_or(anyhow!("No core mods existed for {}", app_info.version))?;

    for core_mod in &core_mods.mods {
        match mod_manager.get_mod(&core_mod.id) {
            Some(existing) => {
                let existing_ref = existing.borrow();
                if existing_ref.manifest().version >= core_mod.version {
                    info!(
                        "Core mod {} was already installed with new enough version: {}",
                        core_mod.id,
                        existing_ref.manifest().version
                    );
                    continue;
                }
            }
            None => {}
        }

        info!("Downloading {} v{}", core_mod.id, core_mod.version);

        let core_mod_vec =
            downloads::download_to_vec_with_attempts(&crate::get_dl_cfg(), &core_mod.download_url)
                .context("Downloading core mod")?;
        let result = mod_manager.try_load_new_mod(Cursor::new(core_mod_vec));
        result?;
    }

    info!("Installing core mods");
    mod_manager
        .load_mods()
        .context("Loading core mods - is one invalid? If so, this is a BIG problem")?;
    for core_mod in &core_mods.mods {
        mod_manager.install_mod(&core_mod.id)?;
    }
    mod_status::mark_all_core_mods(&mod_manager, &core_mods.mods);

    Ok(())
}
