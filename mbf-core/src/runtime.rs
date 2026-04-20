use std::collections::HashMap;

use anyhow::Result;

use crate::models::{
    request::RequestEnum,
    response::Response,
};

/// Host operations that execute external effects for each request variant.
pub trait Host {
    fn get_mod_status(&mut self, override_core_mod_url: Option<String>) -> Result<Response>;
    fn patch(
        &mut self,
        downgrade_to: Option<String>,
        remodding: bool,
        manifest_mod: String,
        allow_no_core_mods: bool,
        device_pre_v51: bool,
        override_core_mod_url: Option<String>,
        vr_splash_path: Option<String>,
    ) -> Result<Response>;
    fn get_downgraded_manifest(&mut self, version: String) -> Result<Response>;
    fn remove_mod(&mut self, id: String) -> Result<Response>;
    fn set_mods_enabled(&mut self, statuses: HashMap<String, bool>) -> Result<Response>;
    fn import(&mut self, from_path: String) -> Result<Response>;
    fn import_url(&mut self, from_url: String) -> Result<Response>;
    fn fix_player_data(&mut self) -> Result<Response>;
    fn quick_fix(
        &mut self,
        override_core_mod_url: Option<String>,
        wipe_existing_mods: bool,
    ) -> Result<Response>;
}

/// Pure request routing from protocol request variants to host operations.
pub fn handle_request<H: Host>(host: &mut H, request: RequestEnum) -> Result<Response> {
    match request {
        RequestEnum::GetModStatus {
            override_core_mod_url,
        } => host.get_mod_status(override_core_mod_url),
        RequestEnum::Patch {
            downgrade_to,
            remodding,
            manifest_mod,
            allow_no_core_mods,
            device_pre_v51,
            override_core_mod_url,
            vr_splash_path,
        } => host.patch(
            downgrade_to,
            remodding,
            manifest_mod,
            allow_no_core_mods,
            device_pre_v51,
            override_core_mod_url,
            vr_splash_path,
        ),
        RequestEnum::GetDowngradedManifest { version } => host.get_downgraded_manifest(version),
        RequestEnum::RemoveMod { id } => host.remove_mod(id),
        RequestEnum::SetModsEnabled { statuses } => host.set_mods_enabled(statuses),
        RequestEnum::Import { from_path } => host.import(from_path),
        RequestEnum::ImportUrl { from_url } => host.import_url(from_url),
        RequestEnum::FixPlayerData => host.fix_player_data(),
        RequestEnum::QuickFix {
            override_core_mod_url,
            wipe_existing_mods,
        } => host.quick_fix(override_core_mod_url, wipe_existing_mods),
    }
}
