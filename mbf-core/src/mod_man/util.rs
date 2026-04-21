//! Utilities for mod management.

use anyhow::{Context, Result};
use log::{debug, warn};
use std::path::Path;

use crate::host::Host;

/// Checks if all files with the specified file names exist within a directory.
pub(super) fn files_exist_in_dir<H: Host>(
    host: &mut H,
    dir_path: &str,
    file_paths: impl Iterator<Item = impl AsRef<str>>,
) -> bool {
    for name in file_paths {
        let file_name = Path::new(name.as_ref())
            .file_name()
            .expect("Mod file names should not be blank")
            .to_string_lossy()
            .to_string();
        if !host.file_exists(&format!("{}/{}", dir_path, file_name)) {
            return false;
        }
    }
    true
}

/// Copies stated mod/lib/early_mod files from a mod folder to the modloader folder.
pub(super) fn copy_files_from_mod_folder<H: Host>(
    host: &mut H,
    mod_folder: &str,
    files: &[impl AsRef<str>],
    modloader_folder: &str,
) -> Result<()> {
    for file in files {
        let file = file.as_ref();
        let file_location = format!("{}/{}", mod_folder, file);

        if !host.file_exists(&file_location) {
            warn!("Could not install file {file} as it wasn't found in the QMOD");
            continue;
        }

        let file_name = Path::new(file)
            .file_name()
            .context("Mod file should have a file name")?
            .to_string_lossy()
            .to_string();
        let copy_to = format!("{}/{}", modloader_folder, file_name);

        debug!("Copying {file_name} to {copy_to}");

        if host.file_exists(&copy_to) {
            host.remove_file(&copy_to).context("Removing existing mod file")?;
        }
        host.copy_file(&file_location, &copy_to)
            .context("Copying SO for mod")?;
    }

    Ok(())
}

/// Removes all files within a specified folder that have the same file name as one
/// of the files specified.
pub(super) fn remove_file_names_from_folder<H: Host>(
    host: &mut H,
    file_paths: impl Iterator<Item = impl AsRef<str>>,
    from: &str,
) -> Result<()> {
    for path in file_paths {
        if let Some(file_name) = Path::new(path.as_ref()).file_name() {
            let stored_path = format!("{}/{}", from, file_name.to_string_lossy());
            if host.file_exists(&stored_path) {
                debug!("Removing {}", file_name.to_string_lossy());
                host.remove_file(&stored_path)?;
            }
        }
    }

    Ok(())
}
