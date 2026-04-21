use std::path::{Path, PathBuf};

use crate::{
    downloads,
    mod_man::ModManager,
    models::response::{self, ImportResultType, Response}, parameters::PARAMETERS
};
use anyhow::{anyhow, Context, Result};
use log::{debug, info, warn};
use mbf_zip::ZipFile;

pub(super) fn handle_import_mod_url(from_url: String) -> Result<Response> {
    crate::hal().create_dir_all(Path::new(&PARAMETERS.mbf_downloads))?;
    let download_path = Path::new(&PARAMETERS.mbf_downloads).join("import_from_url");

    info!("Downloading {}", from_url);
    let filename: Option<String> =
        downloads::download_file_with_attempts(&crate::get_dl_cfg(), &download_path, &from_url)?;

    handle_import(&download_path, filename)
}

pub(super) fn handle_import(
    from_path: impl AsRef<Path> + std::fmt::Debug,
    override_filename: Option<String>,
) -> Result<Response> {
    let res_cache = crate::load_res_cache()?;
    let mut mod_manager = ModManager::new(super::get_app_version_only()?, &res_cache);
    mod_manager.load_mods()?;

    let filename = match override_filename {
        Some(filename) => filename,
        None => from_path
            .as_ref()
            .file_name()
            .ok_or(anyhow!("No filename in {from_path:?}"))?
            .to_string_lossy()
            .to_string(),
    };

    let path = from_path.as_ref().to_owned();
    info!("Attempting to import from {filename}");

    let file_ext = filename
        .split('.')
        .rev()
        .next()
        .ok_or(anyhow!("No file extension in filename {filename}"))?
        .to_string()
        .to_lowercase();

    let import_result = if file_ext == "qmod" {
        handle_import_qmod(mod_manager, path.clone())
    } else if file_ext == "zip" {
        attempt_song_import(path.clone())
    } else if file_ext == "dll" {
        crate::hal().remove_file(&path).context("Removing temporary upload file")?;
        Ok(response::ImportResultType::NonQuestModDetected)
    } else {
        attempt_file_copy(path.clone(), file_ext, mod_manager)
    };

    match import_result {
        Ok(result) => Ok(Response::ImportResult {
            result,
            used_filename: filename,
        }),
        Err(err) => {
            match crate::hal().remove_file(&path) {
                Ok(_) => {}
                Err(err) => warn!("Failed to remove temporary file: {err}"),
            }
            Err(err)
        }
    }
}

fn handle_import_qmod(mut mod_manager: ModManager, from_path: PathBuf) -> Result<ImportResultType> {
    debug!("Loading {from_path:?} as a QMOD");
    let file = crate::hal().open_file_rw(&from_path)?;
    let id = mod_manager.try_load_new_mod(file)?;
    crate::hal().remove_file(&from_path)?;

    let installed_mods = super::mod_management::get_mod_models(mod_manager)?;

    Ok(ImportResultType::ImportedMod {
        imported_id: id,
        installed_mods,
    })
}

fn attempt_file_copy(
    from_path: PathBuf,
    file_ext: String,
    mod_manager: ModManager,
) -> Result<ImportResultType> {
    for m in mod_manager.get_mods() {
        let mod_ref = (**m).borrow();
        match mod_ref
            .manifest()
            .copy_extensions
            .iter()
            .filter(|ext| ext.extension.eq_ignore_ascii_case(&file_ext))
            .next()
        {
            Some(copy_ext) => {
                info!("Copying to {}", copy_ext.destination);
                let dest_folder = Path::new(&copy_ext.destination);
                crate::hal().create_dir_all(dest_folder)
                    .context("Creating destination folder for file copy")?;
                let dest_path = dest_folder.join(from_path.file_name().unwrap());

                crate::hal().copy_file(&from_path, &dest_path).context("Copying mod file copy extension")?;
                crate::hal().remove_file(&from_path)?;

                return Ok(ImportResultType::ImportedFileCopy {
                    copied_to: dest_path.to_string_lossy().to_string(),
                    mod_id: mod_ref.manifest().id.to_string(),
                });
            }
            None => {}
        }
    }

    Err(anyhow!(
        "File extension `.{}` was not recognised by any mod",
        file_ext
    ))
}

fn attempt_song_import(from_path: PathBuf) -> Result<ImportResultType> {
    let song_handle = crate::hal().open_file_rw(&from_path)?;
    let mut zip = ZipFile::open(song_handle).context("Song was invalid ZIP file")?;

    if zip.contains_file("info.dat") || zip.contains_file("Info.dat") {
        let extract_path = Path::new(&PARAMETERS.custom_levels)
            .join(from_path.file_stem().expect("Must have file stem"));

        if crate::hal().path_exists(&extract_path) {
            crate::hal().remove_dir_all(&extract_path).context("Deleting existing song")?;
        }

        crate::hal().create_dir_all(&extract_path)?;
        let entry_names = zip
            .iter_entry_names()
            .map(|s| s.to_string())
            .collect::<Vec<String>>();
        for entry_name in entry_names {
            zip.extract_file_to(&entry_name, extract_path.join(&entry_name))?;
        }

        drop(zip);
        crate::hal().remove_file(&from_path)?;
        Ok(ImportResultType::ImportedSong)
    } else {
        Err(anyhow!(
            "ZIP file was not a song; Unclear know how to import it"
        ))
    }
}
