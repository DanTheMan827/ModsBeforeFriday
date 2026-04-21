use std::io::Cursor;

use anyhow::{anyhow, Context, Result};
use log::{debug, info, warn};
use mbf_zip::ZipFile;

use crate::{
    host::Host,
    mod_man::ModManager,
    models::response::{self, ImportResultType, Response},
    resources::ResCache,
};

pub fn handle_import_mod_url<H: Host>(host: &mut H, from_url: String) -> Result<Response> {
    let config = host.get_config().clone();
    host.create_dir_all(&config.mbf_downloads_dir)?;
    let download_path = format!("{}/import_from_url", config.mbf_downloads_dir);

    info!("Downloading {}", from_url);
    let filename = host
        .http_get_file(&from_url, &download_path)
        .context("Downloading mod from URL")?;

    handle_import(host, download_path, filename)
}

pub fn handle_import<H: Host>(
    host: &mut H,
    from_path: String,
    override_filename: Option<String>,
) -> Result<Response> {
    let res_cache = ResCache::new(host.get_config().res_cache_dir.clone());
    let qmods_dir_template = host.get_config().qmods_dir_template.clone();
    let mut mod_manager = ModManager::new(
        super::mod_status::get_app_version_only(host)?,
        res_cache,
        &qmods_dir_template,
    );
    mod_manager.load_mods(host)?;

    let filename = match override_filename {
        Some(f) => f,
        None => std::path::Path::new(&from_path)
            .file_name()
            .ok_or_else(|| anyhow!("No filename in {from_path}"))?
            .to_string_lossy()
            .to_string(),
    };

    info!("Attempting to import from {filename}");

    let file_ext = filename
        .split('.')
        .rev()
        .next()
        .ok_or_else(|| anyhow!("No file extension in filename {filename}"))?
        .to_string()
        .to_lowercase();

    let import_result = if file_ext == "qmod" {
        handle_import_qmod(host, mod_manager, &from_path)
    } else if file_ext == "zip" {
        attempt_song_import(host, &from_path)
    } else if file_ext == "dll" {
        // PC mod file — delete it and report back.
        host.remove_file(&from_path)
            .context("Removing temporary upload file")?;
        Ok(response::ImportResultType::NonQuestModDetected)
    } else {
        attempt_file_copy(host, &from_path, file_ext, mod_manager)
    };

    match import_result {
        Ok(result) => Ok(Response::ImportResult {
            result,
            used_filename: filename,
        }),
        Err(err) => {
            match host.remove_file(&from_path) {
                Ok(_) => {}
                Err(e) => warn!("Failed to remove temporary file: {e}"),
            }
            Err(err)
        }
    }
}

fn handle_import_qmod<H: Host>(
    host: &mut H,
    mut mod_manager: ModManager,
    from_path: &str,
) -> Result<ImportResultType> {
    debug!("Loading {from_path} as a QMOD");
    let bytes = host.read_file(from_path)?;
    let id = mod_manager
        .try_load_new_mod(host, Cursor::new(bytes))
        .context("Loading QMOD")?;
    host.remove_file(from_path)?;

    let installed_mods = super::mod_management::get_mod_models(mod_manager)?;
    Ok(ImportResultType::ImportedMod {
        imported_id: id,
        installed_mods,
    })
}

fn attempt_file_copy<H: Host>(
    host: &mut H,
    from_path: &str,
    file_ext: String,
    mod_manager: ModManager,
) -> Result<ImportResultType> {
    for m in mod_manager.get_mods() {
        let mod_ref = (**m).borrow();
        if let Some(copy_ext) = mod_ref
            .manifest()
            .copy_extensions
            .iter()
            .find(|ext| ext.extension.eq_ignore_ascii_case(&file_ext))
        {
            info!("Copying to {}", copy_ext.destination);
            host.create_dir_all(&copy_ext.destination)
                .context("Creating destination folder for file copy")?;

            let file_name = std::path::Path::new(from_path)
                .file_name()
                .expect("Must have file name")
                .to_string_lossy()
                .to_string();
            let dest_path = format!("{}/{}", copy_ext.destination, file_name);

            host.copy_file(from_path, &dest_path)
                .context("Copying mod file copy extension")?;
            host.remove_file(from_path)?;

            return Ok(ImportResultType::ImportedFileCopy {
                copied_to: dest_path,
                mod_id: mod_ref.manifest().id.to_string(),
            });
        }
    }

    Err(anyhow!(
        "File extension `.{}` was not recognised by any mod",
        file_ext
    ))
}

fn attempt_song_import<H: Host>(host: &mut H, from_path: &str) -> Result<ImportResultType> {
    let bytes = host.read_file(from_path)?;
    let mut zip = ZipFile::open(Cursor::new(bytes)).context("Song was invalid ZIP file")?;

    if zip.contains_file("info.dat") || zip.contains_file("Info.dat") {
        let file_stem = std::path::Path::new(from_path)
            .file_stem()
            .expect("Must have file stem")
            .to_string_lossy()
            .to_string();

        let custom_levels_dir = host.get_config().custom_levels_dir.clone();
        let extract_path = format!("{}/{}", custom_levels_dir, file_stem);

        if host.file_exists(&extract_path) {
            host.remove_dir_all(&extract_path)
                .context("Deleting existing song")?;
        }
        host.create_dir_all(&extract_path)?;

        let entry_names: Vec<String> = zip.iter_entry_names().map(|s| s.to_string()).collect();
        for entry_name in entry_names {
            let entry_data = zip.read_file(&entry_name)?;
            let dest = format!("{}/{}", extract_path, entry_name);
            if let Some(parent) = std::path::Path::new(&dest).parent() {
                let parent_str = parent.to_string_lossy().to_string();
                if !parent_str.is_empty() {
                    host.create_dir_all(&parent_str)?;
                }
            }
            host.write_file(&dest, &entry_data)?;
        }

        host.remove_file(from_path)?;
        Ok(ImportResultType::ImportedSong)
    } else {
        Err(anyhow!(
            "ZIP file was not a song; unclear how to import it"
        ))
    }
}
