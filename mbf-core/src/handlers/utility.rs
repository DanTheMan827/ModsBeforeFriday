//! Handles requests relating to some buttons in the options page of MBF.

use std::path::Path;

use crate::{data_fix, mod_man::ModManager, models::response::Response, parameters::PARAMETERS, patching};
use anyhow::{anyhow, Context, Result};
use log::{debug, info, warn};

pub(super) fn handle_quick_fix(
    override_core_mod_url: Option<String>,
    wipe_existing_mods: bool,
) -> Result<Response> {
    let app_info = super::mod_status::get_app_info()?
        .ok_or(anyhow!("Cannot quick fix when app is not installed"))?;
    let res_cache = crate::load_res_cache()?;

    let mut mod_manager = ModManager::new(app_info.version.clone(), &res_cache);
    if wipe_existing_mods {
        info!("Wiping all existing mods");
        mod_manager
            .wipe_all_mods()
            .context("Wiping existing mods")?;
    }
    mod_manager.load_mods()?;

    super::install_core_mods(
        &res_cache,
        &mut mod_manager,
        app_info,
        override_core_mod_url,
    )?;
    patching::install_modloader()?;
    Ok(Response::Mods {
        installed_mods: super::mod_management::get_mod_models(mod_manager)?,
    })
}

pub(super) fn handle_fix_player_data() -> Result<Response> {
    patching::kill_app()?;

    let mut did_work = false;
    if crate::hal().path_exists(Path::new(&PARAMETERS.datakeeper_player_data)) {
        info!("Fixing color scheme issues");
        data_fix::fix_colour_schemes(&PARAMETERS.datakeeper_player_data)?;
        did_work = true;
    }

    if crate::hal().path_exists(Path::new(&PARAMETERS.player_data)) {
        info!("Backing up player data");
        patching::backup_player_data()?;

        info!("Removing (potentially faulty) PlayerData.dat in game files");
        debug!("(removing {})", &PARAMETERS.player_data);
        crate::hal().remove_file(Path::new(&PARAMETERS.player_data)).context("Deleting faulty player data")?;
        if crate::hal().path_exists(Path::new(&PARAMETERS.player_data_bak)) {
            crate::hal().remove_file(Path::new(&PARAMETERS.player_data_bak))?;
        }
        did_work = true;
    } else {
        warn!("No player data found to \"fix\"");
    }

    Ok(Response::FixedPlayerData { existed: did_work })
}
