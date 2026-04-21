//! The structure for each mod loaded by MBF.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::Path;

use super::{util, ModInfo};
use crate::host::{CoreConfig, Host};
use anyhow::{Context, Result};
use log::{debug, warn};

/// Represents a mod (in QMOD format).
#[derive(Debug)]
pub struct Mod {
    pub(super) manifest: ModInfo,
    /// Whether all mod files exist in their expected destinations.
    pub(super) files_exist: bool,
    /// Whether or not the mod is installed (all dependencies also installed).
    /// None until `check_mods_installed` is called.
    pub(super) installed: Option<bool>,
    /// The folder that this mod was loaded from.
    pub(super) loaded_from: String,
    pub(super) is_core: bool,
}

impl Mod {
    pub fn installed(&self) -> bool {
        self.installed
            .expect("Mod install status should have been checked before use")
    }

    pub fn manifest(&self) -> &ModInfo {
        &self.manifest
    }

    pub fn is_core(&self) -> bool {
        self.is_core
    }

    pub(super) fn new<H: Host>(
        host: &mut H,
        config: &CoreConfig,
        manifest: ModInfo,
        loaded_from: String,
    ) -> Result<Self> {
        let files_exist = Self::check_if_files_copied(host, config, &manifest);
        Ok(Self {
            loaded_from,
            files_exist,
            manifest,
            installed: None,
            is_core: false,
        })
    }

    pub(super) fn files_exist(&self) -> bool {
        self.files_exist
    }

    /// Installs this mod by copying all necessary files to the modloader folders.
    pub(super) fn install_unchecked<H: Host>(
        &mut self,
        host: &mut H,
        config: &CoreConfig,
    ) -> Result<()> {
        util::copy_files_from_mod_folder(
            host,
            &self.loaded_from,
            &self.manifest().mod_files,
            &config.early_mods_dir,
        )?;
        util::copy_files_from_mod_folder(
            host,
            &self.loaded_from,
            &self.manifest().library_files,
            &config.libs_dir,
        )?;
        util::copy_files_from_mod_folder(
            host,
            &self.loaded_from,
            &self.manifest().late_mod_files,
            &config.late_mods_dir,
        )?;

        self.copy_file_copies(host).context("Copying auxiliary files")?;

        self.installed = Some(true);
        self.files_exist = true;

        Ok(())
    }

    /// Uninstalls this mod by deleting all copied binary files and file copies.
    pub(super) fn uninstall_unchecked<H: Host>(
        &mut self,
        host: &mut H,
        config: &CoreConfig,
        retained_libs: HashSet<String>,
    ) -> Result<()> {
        util::remove_file_names_from_folder(
            host,
            self.manifest().mod_files.iter(),
            &config.early_mods_dir,
        )?;
        util::remove_file_names_from_folder(
            host,
            self.manifest().late_mod_files.iter(),
            &config.late_mods_dir,
        )?;
        util::remove_file_names_from_folder(
            host,
            self.manifest()
                .library_files
                .iter()
                .filter(|lib_file| !retained_libs.contains(lib_file.as_str())),
            &config.libs_dir,
        )?;

        for copy in &self.manifest().file_copies {
            if host.file_exists(&copy.destination) {
                debug!("Removing file copy at destination {}", copy.destination);
                host.remove_file(&copy.destination)
                    .context("Deleting copied file")?;
            }
        }

        self.installed = Some(false);
        self.files_exist = false;

        Ok(())
    }

    /// Deletes the mod directory without checking install status first.
    pub(super) fn delete_unchecked<H: Host>(self, host: &mut H) -> Result<()> {
        host.remove_dir_all(&self.loaded_from)
            .context("Deleting mod extract directory")?;
        Ok(())
    }

    fn copy_file_copies<H: Host>(&self, host: &mut H) -> Result<()> {
        for file_copy in &self.manifest().file_copies {
            let file_path_in_mod = format!("{}/{}", self.loaded_from, file_copy.name);
            if !host.file_exists(&file_path_in_mod) {
                warn!(
                    "Could not install file copy {} as it did not exist in the QMOD",
                    file_copy.name
                );
                continue;
            }

            if let Some(parent) = Path::new(&file_copy.destination).parent() {
                host.create_dir_all(&parent.to_string_lossy())
                    .context("Creating destination directory for file copy")?;
            }

            if host.file_exists(&file_copy.destination) {
                host.remove_file(&file_copy.destination)
                    .context("Removing existing copied file")?;
            }

            debug!(
                "Installing file copy {} to {}",
                file_path_in_mod, file_copy.destination
            );
            host.copy_file(&file_path_in_mod, &file_copy.destination)
                .context("Copying stated file copy to destination")?;
        }

        Ok(())
    }

    fn check_if_files_copied<H: Host>(
        host: &mut H,
        config: &CoreConfig,
        manifest: &ModInfo,
    ) -> bool {
        util::files_exist_in_dir(host, &config.early_mods_dir, manifest.mod_files.iter())
            && util::files_exist_in_dir(
                host,
                &config.late_mods_dir,
                manifest.late_mod_files.iter(),
            )
            && util::files_exist_in_dir(host, &config.libs_dir, manifest.library_files.iter())
            && {
                let destinations: Vec<String> = manifest
                    .file_copies
                    .iter()
                    .map(|c| c.destination.clone())
                    .collect();
                destinations.iter().all(|dest| host.file_exists(dest))
            }
    }
}

/// Returns the file names (without directory) of all library files in `manifest` as OsStr-compatible strings.
pub(super) fn lib_file_names(manifest: &ModInfo) -> HashSet<String> {
    manifest
        .library_files
        .iter()
        .filter_map(|lib_path| {
            Path::new(lib_path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
        })
        .collect()
}

/// Returns a set of library file *names* (not full paths) that are retained by installed mods
/// other than the one being uninstalled.
pub(super) fn get_retained_lib_file_names(
    mods: &std::collections::HashMap<String, std::rc::Rc<std::cell::RefCell<Mod>>>,
    uninstalling_id: &str,
) -> HashSet<String> {
    let mut retained: HashSet<String> = HashSet::new();
    for (id, m) in mods.iter() {
        if id == uninstalling_id {
            continue;
        }
        let m_ref = m.borrow();
        if !m_ref.installed() {
            continue;
        }
        for lib_path in m_ref.manifest().library_files.iter() {
            if let Some(file_name) = Path::new(lib_path).file_name() {
                // Check that the lib file name does not match with OsStr comparison
                if !OsStr::new(lib_path).is_empty() {
                    retained.insert(file_name.to_string_lossy().to_string());
                }
            }
        }
    }
    retained
}
