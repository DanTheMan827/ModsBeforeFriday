use std::collections::HashMap;

use anyhow::Result;

use crate::{
    handlers,
    host::Host,
    models::{request::RequestEnum, response::Response},
};

/// Dispatches a [RequestEnum] to the appropriate handler, using the provided [Host] for all IO.
pub fn handle_request<H: Host>(host: &mut H, request: RequestEnum) -> Result<Response> {
    match request {
        RequestEnum::GetModStatus {
            override_core_mod_url,
        } => handlers::mod_status::handle_get_mod_status(host, override_core_mod_url),
        RequestEnum::Patch {
            downgrade_to,
            remodding,
            manifest_mod,
            allow_no_core_mods,
            device_pre_v51,
            override_core_mod_url,
            vr_splash_path,
        } => handlers::patching::handle_patch(
            host,
            downgrade_to,
            remodding,
            manifest_mod,
            device_pre_v51,
            allow_no_core_mods,
            override_core_mod_url,
            vr_splash_path,
        ),
        RequestEnum::GetDowngradedManifest { version } => {
            handlers::patching::handle_get_downgraded_manifest(host, version)
        }
        RequestEnum::RemoveMod { id } => handlers::mod_management::handle_remove_mod(host, id),
        RequestEnum::SetModsEnabled { statuses } => {
            handlers::mod_management::handle_set_mods_enabled(host, statuses)
        }
        RequestEnum::Import { from_path } => handlers::import::handle_import(host, from_path, None),
        RequestEnum::ImportUrl { from_url } => {
            handlers::import::handle_import_mod_url(host, from_url)
        }
        RequestEnum::FixPlayerData => handlers::utility::handle_fix_player_data(host),
        RequestEnum::QuickFix {
            override_core_mod_url,
            wipe_existing_mods,
        } => handlers::utility::handle_quick_fix(host, override_core_mod_url, wipe_existing_mods),
    }
}
