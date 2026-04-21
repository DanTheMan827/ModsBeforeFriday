//! Module for mod management within MBF.

mod manifest;
mod util;
mod loaded_mod;

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    io::{Cursor, Read, Seek},
    path::Path,
    rc::Rc,
};

use jsonschema::JSONSchema;
use log::{debug, error, info, warn};
pub use manifest::*;
pub use loaded_mod::Mod;

use anyhow::{anyhow, Context, Result};
use mbf_zip::ZipFile;
use semver::Version;

use crate::{
    host::Host,
    resources::{self, ModRepo, ModRepoMod, ResCache},
};

/// The JSON schema for the `mod.json` file within a qmod.
const QMOD_SCHEMA: &str = include_str!("../../../mbf-agent/src/mod_man/qmod_schema.json");
/// The maximum `_QPVersion` that MBF will accept in `mod.json`.
const MAX_SCHEMA_VERSION: Version = Version::new(1, 2, 0);

pub struct ModManager {
    mods: HashMap<String, Rc<RefCell<Mod>>>,
    schema: JSONSchema,
    qmods_dir: String,
    game_version: String,
    res_cache: ResCache,
    mod_repo: Option<ModRepo>,
}

impl ModManager {
    pub fn new(game_version: String, res_cache: ResCache, qmods_dir_template: &str) -> Self {
        Self {
            mods: HashMap::new(),
            schema: JSONSchema::options()
                .compile(
                    &serde_json::from_str::<serde_json::Value>(QMOD_SCHEMA)
                        .expect("QMOD schema should be valid JSON"),
                )
                .expect("QMOD schema should be a valid JSON schema"),
            qmods_dir: qmods_dir_template.replace('$', &game_version),
            game_version,
            res_cache,
            mod_repo: None,
        }
    }

    /// Removes ALL mod/early-mod and library files.
    pub fn wipe_all_mods<H: Host>(&mut self, host: &mut H) -> Result<()> {
        self.mods.clear();

        let config = host.get_config().clone();
        let to_remove = [
            config.old_qmods_dir.as_str(),
            config.late_mods_dir.as_str(),
            config.early_mods_dir.as_str(),
            config.libs_dir.as_str(),
            self.qmods_dir.as_str(),
        ];
        for path in to_remove {
            if host.file_exists(path) {
                host.remove_dir_all(path).context("Failed to delete mod folder")?;
            }
        }

        self.create_mods_dir(host)?;
        Ok(())
    }

    pub fn get_mods(&self) -> impl Iterator<Item = &Rc<RefCell<Mod>>> {
        self.mods.values()
    }

    pub fn get_mod(&self, id: &str) -> Option<&Rc<RefCell<Mod>>> {
        self.mods.get(id)
    }

    /// Loads the installed mods from the qmods directory.
    pub fn load_mods<H: Host>(&mut self, host: &mut H) -> Result<()> {
        self.create_mods_dir(host)?;
        self.mods.clear();

        let qmods_dir = self.qmods_dir.clone();
        for entry in host.list_dir(&qmods_dir).context("Reading qmods directory")? {
            if !entry.is_dir {
                continue;
            }

            let mod_path = entry.path.clone();
            match self.load_mod_from_directory(host, &mod_path) {
                Ok(loaded_mod) => {
                    if !self.mods.contains_key(&loaded_mod.manifest().id) {
                        self.mods.insert(
                            loaded_mod.manifest().id.clone(),
                            Rc::new(RefCell::new(loaded_mod)),
                        );
                    } else {
                        warn!(
                            "Mod at {mod_path} had ID {}, but a mod with this ID already existed",
                            loaded_mod.manifest().id
                        );
                    }
                }
                Err(err) => {
                    warn!("Failed to load mod from {mod_path}: {err}");
                    match host.remove_dir_all(&mod_path) {
                        Ok(_) => info!("Deleted invalid mod"),
                        Err(e) => warn!("Failed to delete invalid mod at {mod_path}: {e}"),
                    }
                }
            }
        }

        self.check_mods_installed()?;
        match self.load_old_qmods(host) {
            Ok(had_old_qmods) => {
                if had_old_qmods {
                    self.check_mods_installed()?;
                }
            }
            Err(err) => warn!("Failed to load legacy mods: {err}"),
        }

        Ok(())
    }

    /// Checks whether each loaded mod is installed.
    pub fn check_mods_installed(&mut self) -> Result<()> {
        debug!("Checking if mods are installed");
        for mod_rc in self.mods.values() {
            mod_rc.borrow_mut().installed = None;
        }

        let mod_ids: Vec<String> = self.mods.keys().cloned().collect();
        for mod_id in mod_ids {
            let mut checked_in_pass = HashSet::new();
            self.check_mod_installed(&mod_id, &mut checked_in_pass)
                .context("Checking if individual mod was installed")?;
        }

        Ok(())
    }

    /// Installs the mod with the given ID. Installs/upgrades dependencies if necessary.
    pub fn install_mod<H: Host>(&mut self, host: &mut H, id: &str) -> Result<()> {
        let mod_rc = self
            .mods
            .get(id)
            .ok_or(anyhow!("Could not install mod with ID {id} as it did not exist"))?
            .clone();

        let to_install = mod_rc.borrow();
        if to_install.installed() {
            return Ok(());
        }

        info!(
            "Installing {} v{}",
            to_install.manifest().id,
            to_install.manifest().version
        );

        let deps: Vec<_> = to_install.manifest().dependencies.clone();
        drop(to_install);

        for dep in &deps {
            match self.mods.get(&dep.id).cloned() {
                Some(existing_dep) => {
                    let dep_installed;
                    let dep_version_ok;
                    {
                        let dep_ref = existing_dep.borrow();
                        dep_installed = dep_ref.installed();
                        dep_version_ok = dep.version_range.matches(&dep_ref.manifest().version);
                    }

                    if !dep_version_ok {
                        info!(
                            "Dependency {} is out of date, updating",
                            dep.id
                        );
                        self.install_dependency(host, dep)?;
                    } else if !dep_installed && dep.required {
                        info!("Dependency {} was not installed, reinstalling", dep.id);
                        self.install_mod(host, &dep.id)?;
                    }
                }
                None => {
                    if dep.required {
                        info!("Dependency {} was not found: installing now", dep.id);
                        self.install_dependency(host, dep)?;
                    }
                }
            }
        }

        let config = host.get_config().clone();
        mod_rc
            .borrow_mut()
            .install_unchecked(host, &config)
            .context("Installing mod")?;
        Ok(())
    }

    /// Uninstalls the mod with the given ID. Uninstalls required dependants first.
    pub fn uninstall_mod<H: Host>(&self, host: &mut H, id: &str) -> Result<()> {
        let mod_rc = self
            .mods
            .get(id)
            .ok_or(anyhow!("Could not uninstall mod with ID {id} as it did not exist"))?
            .clone();
        {
            let to_remove = mod_rc.borrow();
            if !to_remove.installed() {
                return Ok(());
            }
            info!(
                "Uninstalling {} v{}",
                to_remove.manifest().id,
                to_remove.manifest().version
            );
        }

        let other_ids: Vec<String> = self.mods.keys().cloned().collect();
        for other_id in other_ids {
            if other_id == id {
                continue;
            }

            let should_uninstall = {
                let m = self.mods.get(&other_id).unwrap();
                let m_ref = m.borrow();
                m_ref.installed()
                    && m_ref
                        .manifest()
                        .dependencies
                        .iter()
                        .any(|dep| dep.id == id && dep.required)
            };

            if should_uninstall {
                info!("Uninstalling (required) dependent mod {}", other_id);
                self.uninstall_mod(host, &other_id)?;
            }
        }

        let retained_libs = loaded_mod::get_retained_lib_file_names(&self.mods, id);
        let config = host.get_config().clone();
        mod_rc
            .borrow_mut()
            .uninstall_unchecked(host, &config, retained_libs)
            .context("Uninstalling unchecked")?;
        Ok(())
    }

    /// Attempts to load a new QMOD from a stream.
    pub fn try_load_new_mod<H: Host>(
        &mut self,
        host: &mut H,
        mod_stream: impl Read + Seek,
    ) -> Result<String> {
        let mut zip = ZipFile::open(mod_stream).context("Mod was invalid ZIP archive")?;

        let json_data = zip
            .read_file("mod.json")
            .context("Mod had no mod.json manifest")?;
        let loaded_mod_manifest = self
            .load_manifest_from_slice(&json_data)
            .context("Parsing manifest")?;

        debug!(
            "Early load of new mod, ID {}, version: {}, author: {}",
            loaded_mod_manifest.id,
            loaded_mod_manifest.version,
            loaded_mod_manifest.author
        );

        let id = loaded_mod_manifest.id.clone();
        if let Err(msg) = self.check_dependency_compatibility(&id, &loaded_mod_manifest.version) {
            return Err(anyhow!(
                "Could not upgrade {} to v{}: {}",
                id,
                loaded_mod_manifest.version,
                msg
            ));
        }

        if let Some(existing_mod) = self.mods.get(&id).cloned() {
            info!("Removing existing version of mod");
            let retained = loaded_mod::get_retained_lib_file_names(&self.mods, &id);
            let config = host.get_config().clone();
            existing_mod
                .borrow_mut()
                .uninstall_unchecked(host, &config, retained)
                .context("Uninstalling existing mod")?;
        }
        self.remove_mod(host, &id)?;

        info!(
            "Extracting {} v{}",
            loaded_mod_manifest.id, loaded_mod_manifest.version
        );
        let extract_path = self.get_mod_extract_path(host, &loaded_mod_manifest);
        debug!("Extract path: {extract_path}");
        host.create_dir_all(&extract_path)
            .context("Creating extract directory")?;
        zip.extract_to_directory(&extract_path)
            .context("Extracting QMOD file")?;

        let config = host.get_config().clone();
        let loaded_mod = Mod::new(host, &config, loaded_mod_manifest, extract_path)
            .context("Creating Mod")?;
        self.mods
            .insert(id.clone(), Rc::new(RefCell::new(loaded_mod)));

        let mut checked_in_pass = HashSet::new();
        self.check_mod_installed(&id, &mut checked_in_pass)
            .context("Checking whether new mod was installed")?;
        Ok(id)
    }

    /// Uninstalls (if installed) and deletes the mod with the specified ID.
    pub fn remove_mod<H: Host>(&mut self, host: &mut H, id: &str) -> Result<()> {
        if self.mods.contains_key(id) {
            self.uninstall_mod(host, id)?;
            let to_remove = self.mods.remove(id).unwrap();
            let owned_mod = Rc::try_unwrap(to_remove)
                .expect("Should be only one reference")
                .into_inner();

            owned_mod
                .delete_unchecked(host)
                .context("Deleting mod files")?;
        }

        Ok(())
    }

    /// Sets a particular mod ID as being a core mod (transitively marks dependencies too).
    pub fn set_mod_core(&self, id: &str) {
        if let Some(mod_rc) = self.mods.get(id) {
            let mut mod_ref = match mod_rc.try_borrow_mut() {
                Ok(r) => r,
                Err(_) => {
                    warn!("Failed to set mod as core due to cyclical dependency: {id}");
                    return;
                }
            };

            mod_ref.is_core = true;

            let deps: Vec<_> = mod_ref
                .manifest()
                .dependencies
                .iter()
                .filter(|d| d.required)
                .map(|d| d.id.clone())
                .collect();
            drop(mod_ref);

            for dep_id in deps {
                self.set_mod_core(&dep_id);
            }
        }
    }

    fn load_old_qmods<H: Host>(&mut self, host: &mut H) -> Result<bool> {
        let old_qmods_dir = host.get_config().old_qmods_dir.clone();
        if !host.file_exists(&old_qmods_dir) {
            return Ok(false);
        }

        warn!("Migrating mods from legacy folder");
        let mut found_qmod = false;

        let entries = host
            .list_dir(&old_qmods_dir)
            .context("Reading old QMODs directory")?;

        for entry in entries {
            if entry.is_dir {
                continue;
            }
            let path = entry.path.clone();
            debug!("Migrating {path}");

            let mod_bytes = host.read_file(&path).context("Opening legacy mod")?;
            match self.try_load_new_mod(host, Cursor::new(mod_bytes)) {
                Ok(new_mod) => info!("Successfully migrated legacy mod {new_mod}"),
                Err(err) => warn!("Failed to migrate legacy mod at {path}: {err}"),
            }

            found_qmod = true;
            host.remove_file(&path).context("Deleting legacy mod")?;
        }

        let _ = host.remove_dir_all(&old_qmods_dir);

        Ok(found_qmod)
    }

    fn load_mod_from_directory<H: Host>(&self, host: &mut H, from: &str) -> Result<Mod> {
        let manifest_path = format!("{}/mod.json", from);
        if !host.file_exists(&manifest_path) {
            return Err(anyhow!("Mod at {from} had no mod.json manifest"));
        }

        let json_data = host
            .read_file(&manifest_path)
            .context("Opening manifest (mod.json) in mod folder")?;

        let manifest = self
            .load_manifest_from_slice(&json_data)
            .context("Parsing manifest as JSON")?;

        let config = host.get_config().clone();
        Mod::new(host, &config, manifest, from.to_string()).context("Creating Mod")
    }

    fn load_manifest_from_slice(&self, manifest_slice: &[u8]) -> Result<ModInfo> {
        let manifest_value = serde_json::from_slice::<serde_json::Value>(manifest_slice)?;

        match manifest_value.get("_QPVersion") {
            Some(serde_json::Value::String(schema_ver)) => {
                let sem_version = semver::Version::parse(schema_ver)
                    .context("Parsing specified QMOD schema version")?;

                if sem_version > MAX_SCHEMA_VERSION {
                    return Err(anyhow!(
                        "QMOD specified schema version {sem_version} which was newer than the maximum supported version {MAX_SCHEMA_VERSION}."
                    ));
                }
            }
            _ => {
                return Err(anyhow!(
                    "Could not load mod as its manifest did not specify a QMOD schema version"
                ))
            }
        }

        if let Err(errors) = self.schema.validate(&manifest_value) {
            let mut log_builder = String::new();
            for error in errors {
                log_builder.push_str(&format!("Validation error: {}\n", error));
                log_builder.push_str(&format!("Instance path: {}\n", error.instance_path));
            }
            return Err(anyhow!("QMOD schema validation failed: \n{log_builder}"));
        }

        Ok(serde_json::from_value(manifest_value)
            .expect("Failed to parse as QMOD manifest despite valid schema"))
    }

    fn get_retained_lib_files(&self, uninstalling_id: &str) -> HashSet<String> {
        loaded_mod::get_retained_lib_file_names(&self.mods, uninstalling_id)
    }

    fn install_dependency<H: Host>(&mut self, host: &mut H, dep: &ModDependency) -> Result<()> {
        let link = if let Some(dep_url) = self.try_get_dep_from_mod_repo(host, dep) {
            dep_url
        } else {
            match &dep.mod_link {
                Some(link) => link.clone(),
                None => {
                    return Err(anyhow!(
                        "Could not download dependency {}: no link given and could not find in mod repo",
                        dep.id
                    ))
                }
            }
        };

        info!("Downloading dependency from {}", link);
        let dependency_bytes = host.http_get(&link).context("Downloading dependency")?;

        self.try_load_new_mod(host, Cursor::new(dependency_bytes))?;
        self.install_mod(host, &dep.id)?;
        Ok(())
    }

    fn try_get_dep_from_mod_repo<H: Host>(
        &mut self,
        host: &mut H,
        dep: &ModDependency,
    ) -> Option<String> {
        let mut latest_dep: Option<ModRepoMod> = None;
        let mut update_with_latest = |repo_mod: &ModRepoMod| {
            if repo_mod.id == dep.id && dep.version_range.matches(&repo_mod.version) {
                match latest_dep.as_ref() {
                    Some(existing_latest) => {
                        if existing_latest.version > repo_mod.version {
                            return;
                        }
                    }
                    None => {}
                }
                latest_dep = Some(repo_mod.clone());
            }
        };

        let game_ver_clone = self.game_version.clone();

        match self.get_or_load_mod_repo(host) {
            Ok(mod_repo) => {
                if let Some(global_mods) = mod_repo.get("global") {
                    global_mods.iter().for_each(&mut update_with_latest);
                }
                if let Some(version_mods) = mod_repo.get(&game_ver_clone) {
                    version_mods.iter().for_each(&mut update_with_latest);
                }

                match latest_dep {
                    Some(dep) => {
                        info!(
                            "Found download URL for {} v{} in mod repo",
                            dep.id, dep.version
                        );
                        Some(dep.download)
                    }
                    None => {
                        debug!(
                            "Mod repo had no matching dependency for {} range {}",
                            dep.id, dep.version_range
                        );
                        None
                    }
                }
            }
            Err(err) => {
                warn!(
                    "Could not check for latest {} range {} from mod repo: {err}",
                    dep.id, dep.version_range
                );
                None
            }
        }
    }

    fn check_mod_installed(
        &self,
        id: &str,
        checked_in_pass: &mut HashSet<String>,
    ) -> Result<bool> {
        if !checked_in_pass.insert(id.to_string()) {
            return Err(anyhow!(
                "Recursive dependency detected. Mod with ID {id} depends on itself"
            ));
        }

        let mod_rc = self
            .mods
            .get(id)
            .ok_or(anyhow!("No mod with ID {id} found"))?;

        let mod_ref = mod_rc.borrow();
        let installed = self.check_mod_installed_internal(&*mod_ref, checked_in_pass)?;

        drop(mod_ref);
        mod_rc.borrow_mut().installed = Some(installed);

        Ok(installed)
    }

    fn check_mod_installed_internal(
        &self,
        mod_ref: &Mod,
        checked_in_path: &mut HashSet<String>,
    ) -> Result<bool> {
        if !mod_ref.files_exist() {
            return Ok(false);
        }

        for dependency in &mod_ref.manifest().dependencies {
            match self.get_mod(&dependency.id) {
                Some(dep_rc) => {
                    let dep_ref = dep_rc.borrow();

                    let dep_installed = match dep_ref.installed {
                        None => {
                            drop(dep_ref);
                            self.check_mod_installed(&dependency.id, checked_in_path)
                                .context("Checking if dependency was installed")?
                        }
                        Some(installed) => installed,
                    };

                    if !dep_installed {
                        return Ok(!dependency.required);
                    }

                    let dep_ver_ok = dependency
                        .version_range
                        .matches(&dep_rc.borrow().manifest().version);
                    if !dep_ver_ok {
                        return Ok(false);
                    }
                }
                None => return Ok(!dependency.required),
            }
        }

        Ok(true)
    }

    fn get_mod_extract_path<H: Host>(&self, host: &mut H, manifest: &ModInfo) -> String {
        let mut i = 1usize;
        loop {
            let mut folder_name = format!("{}_v{}", manifest.id, manifest.version);
            if i > 1 {
                warn!(
                    "When finding path to extract {} v{}, the folder name {folder_name} was already occupied",
                    manifest.id, manifest.version
                );
                folder_name.push('_');
                folder_name.push_str(&i.to_string());
            }

            let extract_path = format!("{}/{}", self.qmods_dir, folder_name);
            if !host.file_exists(&extract_path) {
                break extract_path;
            }

            i += 1;
        }
    }

    fn create_mods_dir<H: Host>(&self, host: &mut H) -> Result<()> {
        let qmods_dir = self.qmods_dir.clone();
        let config = host.get_config().clone();

        host.create_dir_all(&qmods_dir)?;
        host.create_dir_all(&config.late_mods_dir)?;
        host.create_dir_all(&config.early_mods_dir)?;
        host.create_dir_all(&config.libs_dir)?;

        if !host.file_exists(&config.moddata_nomedia) {
            host.write_file(&config.moddata_nomedia, &[])?;
        }

        Ok(())
    }

    fn check_dependency_compatibility(
        &self,
        dep_id: &str,
        new_version: &Version,
    ) -> Result<(), String> {
        let mut incompatibilities = String::new();
        let mut all_compatible = true;

        for (_, existing_mod) in &self.mods {
            let mod_ref = existing_mod.borrow();
            if !mod_ref.installed() {
                continue;
            }

            if let Some(existing_dep) = mod_ref
                .manifest()
                .dependencies
                .iter()
                .find(|d| d.id == dep_id)
            {
                if !existing_dep.version_range.matches(new_version) {
                    all_compatible = false;
                    let msg = format!(
                        "Mod {} depends on range {}",
                        mod_ref.manifest().id,
                        existing_dep.version_range
                    );
                    error!("Cannot upgrade {dep_id} to {new_version}: {msg}");
                    incompatibilities.push_str(&msg);
                    incompatibilities.push('\n');
                }
            }
        }

        if all_compatible {
            Ok(())
        } else {
            incompatibilities.pop();
            Err(incompatibilities)
        }
    }

    fn get_or_load_mod_repo<H: Host>(&mut self, host: &mut H) -> Result<&ModRepo> {
        if self.mod_repo.is_none() {
            let repo = resources::get_mod_repo(host, &mut self.res_cache)
                .context("Downloading mod repo")?;
            self.mod_repo = Some(repo);
        }

        Ok(self.mod_repo.as_ref().expect("Just loaded mod repo"))
    }
}
