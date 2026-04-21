use anyhow::{anyhow, Context, Result};
use log::{info, warn};

use crate::{
    host::Host,
    mod_man::ModManager,
    models::response::Response,
    resources::ResCache,
};

pub fn handle_quick_fix<H: Host>(
    host: &mut H,
    override_core_mod_url: Option<String>,
    wipe_existing_mods: bool,
) -> Result<Response> {
    let app_info = super::mod_status::get_app_info(host)?
        .ok_or_else(|| anyhow!("Cannot quick fix when app is not installed"))?;

    let res_cache_dir = host.get_config().res_cache_dir.clone();
    let mut res_cache = ResCache::new(res_cache_dir);
    let qmods_dir_template = host.get_config().qmods_dir_template.clone();

    let mut mod_manager = ModManager::new(
        app_info.version.clone(),
        ResCache::new(host.get_config().res_cache_dir.clone()),
        &qmods_dir_template,
    );

    if wipe_existing_mods {
        info!("Wiping all existing mods");
        mod_manager
            .wipe_all_mods(host)
            .context("Wiping existing mods")?;
    }
    mod_manager.load_mods(host)?;

    super::mod_status::install_core_mods(
        host,
        &mut res_cache,
        &mut mod_manager,
        app_info,
        override_core_mod_url,
    )?;
    crate::patching::install_modloader(host)?;

    Ok(Response::Mods {
        installed_mods: super::mod_management::get_mod_models(mod_manager)?,
    })
}

pub fn handle_fix_player_data<H: Host>(host: &mut H) -> Result<Response> {
    crate::patching::kill_app(host)?;

    let config = host.get_config().clone();
    let mut did_work = false;

    if host.file_exists(&config.datakeeper_player_data) {
        info!("Fixing color scheme issues");
        crate::data_fix::fix_colour_schemes(host, &config.datakeeper_player_data)?;
        did_work = true;
    }

    if host.file_exists(&config.player_data_path) {
        info!("Backing up player data");
        crate::patching::backup_player_data(host)?;

        info!("Removing (potentially faulty) PlayerData.dat in game files");
        host.remove_file(&config.player_data_path)
            .context("Deleting faulty player data")?;
        if host.file_exists(&config.player_data_bak_path) {
            host.remove_file(&config.player_data_bak_path)?;
        }
        did_work = true;
    } else {
        warn!("No player data found to \"fix\"");
    }

    Ok(Response::FixedPlayerData { existed: did_work })
}
