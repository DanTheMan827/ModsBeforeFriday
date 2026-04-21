use std::{fs, io::Read, process::Command};

use anyhow::{Context, Result};
use mbf_core::host::{CommandOutput, CoreConfig, DirEntry, Host};

use crate::parameters::PARAMETERS;

pub struct AgentHost {
    config: CoreConfig,
}

impl AgentHost {
    pub fn new() -> Self {
        let p = &*PARAMETERS;
        Self {
            config: CoreConfig {
                apk_id: p.apk_id.clone(),
                qmods_dir_template: p.qmods.clone(),
                old_qmods_dir: p.old_qmods.clone(),
                moddata_nomedia: p.moddata_nomedia.clone(),
                modloader_dir: p.modloader_dir.clone(),
                late_mods_dir: p.late_mods.clone(),
                early_mods_dir: p.early_mods.clone(),
                libs_dir: p.libs.clone(),
                player_data_path: p.player_data.clone(),
                player_data_bak_path: p.player_data_bak.clone(),
                obb_dir: p.obb_dir.clone(),
                datakeeper_player_data: p.datakeeper_player_data.clone(),
                aux_data_backup: p.aux_data_backup.clone(),
                custom_levels_dir: p.custom_levels.clone(),
                mbf_downloads_dir: p.mbf_downloads.clone(),
                temp_dir: p.temp.clone(),
                res_cache_dir: p.res_cache.clone(),
                legacy_dirs: p.legacy_dirs.to_vec(),
            },
        }
    }
}

impl Host for AgentHost {
    fn get_config(&self) -> &CoreConfig {
        &self.config
    }

    fn read_file(&mut self, path: &str) -> Result<Vec<u8>> {
        fs::read(path).with_context(|| format!("Reading {path}"))
    }

    fn write_file(&mut self, path: &str, data: &[u8]) -> Result<()> {
        fs::write(path, data).with_context(|| format!("Writing {path}"))
    }

    fn file_exists(&mut self, path: &str) -> bool {
        std::path::Path::new(path).exists()
    }

    fn remove_file(&mut self, path: &str) -> Result<()> {
        fs::remove_file(path).with_context(|| format!("Removing {path}"))
    }

    fn create_dir_all(&mut self, path: &str) -> Result<()> {
        fs::create_dir_all(path).with_context(|| format!("Creating dir {path}"))
    }

    fn remove_dir_all(&mut self, path: &str) -> Result<()> {
        fs::remove_dir_all(path).with_context(|| format!("Removing dir {path}"))
    }

    fn copy_file(&mut self, from: &str, to: &str) -> Result<()> {
        fs::copy(from, to).with_context(|| format!("Copying {from} to {to}"))?;
        Ok(())
    }

    fn list_dir(&mut self, path: &str) -> Result<Vec<DirEntry>> {
        let entries = fs::read_dir(path).with_context(|| format!("Reading dir {path}"))?;
        let mut result = Vec::new();
        for entry in entries {
            let entry = entry.context("Reading directory entry")?;
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            result.push(DirEntry {
                path: entry.path().to_string_lossy().to_string(),
                is_dir,
            });
        }
        Ok(result)
    }

    fn http_get(&mut self, url: &str) -> Result<Vec<u8>> {
        let agent = mbf_res_man::default_agent::get_agent();
        let response = agent
            .get(url)
            .call()
            .with_context(|| format!("HTTP GET {url}"))?;
        let mut bytes = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut bytes)
            .with_context(|| format!("Reading response body from {url}"))?;
        Ok(bytes)
    }

    fn http_get_file(&mut self, url: &str, dest_path: &str) -> Result<Option<String>> {
        crate::downloads::download_file_with_attempts(crate::get_dl_cfg(), dest_path, url)
    }

    fn run_command(&mut self, cmd: &str, args: &[&str]) -> Result<CommandOutput> {
        let output = Command::new(cmd)
            .args(args)
            .output()
            .with_context(|| format!("Running command: {cmd}"))?;
        Ok(CommandOutput {
            stdout: output.stdout,
            stderr: output.stderr,
            exit_code: output.status.code(),
        })
    }
}
