use std::collections::HashMap;

use anyhow::{Context, Result};
use log::info;

use crate::{
    host::Host,
    mod_man::ModManager,
    models::response::{ModModel, Response},
    resources::ResCache,
};

use super::mod_status;

pub fn handle_set_mods_enabled<H: Host>(
    host: &mut H,
    statuses: HashMap<String, bool>,
) -> Result<Response> {
    let res_cache = ResCache::new(host.get_config().res_cache_dir.clone());
    let qmods_dir_template = host.get_config().qmods_dir_template.clone();

    let app_version = mod_status::get_app_version_only(host)?;
    let mut mod_manager = ModManager::new(app_version, res_cache, &qmods_dir_template);
    mod_manager.load_mods(host).context("Loading installed mods")?;

    let mut error = String::new();

    for (id, new_status) in statuses {
        let mod_rc = match mod_manager.get_mod(&id) {
            Some(m) => m.clone(),
            None => {
                error.push_str(&format!("Mod with ID {id} did not exist\n"));
                continue;
            }
        };

        let already_installed = mod_rc.borrow().installed();
        if new_status && !already_installed {
            match mod_manager.install_mod(host, &id) {
                Ok(_) => info!("Installed {id}"),
                Err(err) => error.push_str(&format!("Failed to install {id}: {err}\n")),
            }
        } else if !new_status && already_installed {
            match mod_manager.uninstall_mod(host, &id) {
                Ok(_) => info!("Uninstalled {id}"),
                Err(err) => error.push_str(&format!("Failed to uninstall {id}: {err}\n")),
            }
        }
    }

    Ok(Response::ModSyncResult {
        installed_mods: get_mod_models(mod_manager)?,
        failures: if !error.is_empty() {
            let trimmed = error.trim_end_matches('\n').to_string();
            Some(trimmed)
        } else {
            None
        },
    })
}

pub fn handle_remove_mod<H: Host>(host: &mut H, id: String) -> Result<Response> {
    let res_cache = ResCache::new(host.get_config().res_cache_dir.clone());
    let qmods_dir_template = host.get_config().qmods_dir_template.clone();

    let app_version = mod_status::get_app_version_only(host)?;
    let mut mod_manager = ModManager::new(app_version, res_cache, &qmods_dir_template);
    mod_manager.load_mods(host)?;
    mod_manager.remove_mod(host, &id)?;

    Ok(Response::Mods {
        installed_mods: get_mod_models(mod_manager)?,
    })
}

pub fn get_mod_models(mut mod_manager: ModManager) -> Result<Vec<ModModel>> {
    mod_manager.check_mods_installed()?;

    Ok(mod_manager
        .get_mods()
        .map(|mod_info| {
            let mod_ref = (**mod_info).borrow();
            ModModel {
                id: mod_ref.manifest().id.clone(),
                name: mod_ref.manifest().name.clone(),
                version: mod_ref.manifest().version.clone(),
                game_version: mod_ref.manifest().package_version.clone(),
                description: mod_ref.manifest().description.clone(),
                is_enabled: mod_ref.installed(),
                is_core: mod_ref.is_core(),
            }
        })
        .collect())
}
