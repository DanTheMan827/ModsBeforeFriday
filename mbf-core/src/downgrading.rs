use std::collections::{HashMap, VecDeque};
use std::io::{BufWriter, Write};

use anyhow::{anyhow, Context, Result};
use log::info;

use crate::host::Host;
use crate::resources::{self, Diff, DiffIndex, ResCache, VersionDiffs};
use mbf_zip::ZIP_CRC;

pub fn get_all_accessible_versions<H: Host>(
    host: &mut H,
    res_cache: &mut ResCache,
    from_version: &str,
) -> Result<HashMap<String, Vec<VersionDiffs>>> {
    let diff_index_edges = get_diff_index_graph(host, res_cache).context("Loading diff index")?;

    let mut predecessor_map: HashMap<String, Vec<VersionDiffs>> = HashMap::new();
    predecessor_map.insert(from_version.to_owned(), Vec::new());

    let mut queue = VecDeque::new();
    queue.push_back(from_version.to_string());

    while let Some(curr_ver) = queue.pop_front() {
        if let Some(edges) = diff_index_edges.get(&curr_ver) {
            for diff in edges {
                if !predecessor_map.contains_key(&diff.to_version) {
                    let mut path = predecessor_map.get(&curr_ver).unwrap().clone();
                    path.push(diff.clone());
                    predecessor_map.insert(diff.to_version.clone(), path);
                    queue.push_back(diff.to_version.clone());
                }
            }
        }
    }

    predecessor_map.remove(from_version);
    Ok(predecessor_map)
}

fn get_diff_index_graph<H: Host>(
    host: &mut H,
    res_cache: &mut ResCache,
) -> Result<HashMap<String, Vec<VersionDiffs>>> {
    let diff_index: DiffIndex = resources::get_diff_index(host, res_cache)
        .context("Fetching downgrading information")?;

    let mut edges: HashMap<String, Vec<VersionDiffs>> = HashMap::new();
    for diff in diff_index {
        edges.entry(diff.from_version.clone()).or_default().push(diff);
    }

    Ok(edges)
}

pub fn get_and_apply_diff_sequence<H: Host>(
    host: &mut H,
    from_version: &str,
    to_version: &str,
    temp_path: &str,
    temp_apk_path: &str,
    obb_backup_paths: Vec<String>,
    res_cache: &mut ResCache,
) -> Result<Vec<String>> {
    info!("Working out diff sequence for {from_version} --> {to_version}");
    let diff_sequences = get_all_accessible_versions(host, res_cache, from_version)
        .context("Determining diff sequence")?;
    let diffs = diff_sequences
        .get(to_version)
        .ok_or(anyhow!("No diff sequence found for version"))?;

    apply_diff_sequence(host, diffs, temp_path, temp_apk_path, obb_backup_paths)
        .context("Downgrading")
}

pub fn apply_diff_sequence<H: Host>(
    host: &mut H,
    diffs: &[VersionDiffs],
    temp_path: &str,
    temp_apk_path: &str,
    mut obb_backup_paths: Vec<String>,
) -> Result<Vec<String>> {
    info!("DOWNGRADING BEAT SABER: This may take a LONG time");
    for (i, diff) in diffs.iter().enumerate() {
        info!(
            "Applying diffs set {}/{} ({} --> {})",
            i + 1,
            diffs.len(),
            diff.from_version,
            diff.to_version
        );
        obb_backup_paths =
            apply_version_diff(host, diff, temp_path, temp_apk_path, obb_backup_paths)
                .context("Applying diff")?;
    }
    Ok(obb_backup_paths)
}

fn apply_version_diff<H: Host>(
    host: &mut H,
    diffs: &VersionDiffs,
    temp_path: &str,
    temp_apk_path: &str,
    obb_backup_paths: Vec<String>,
) -> Result<Vec<String>> {
    let diffs_path = format!("{}/diffs", temp_path);
    host.create_dir_all(&diffs_path).context("Creating diffs directory")?;
    info!("Downloading diffs");
    download_diffs(host, &diffs_path, diffs).context("Downloading diffs")?;

    info!("Downgrading APK");
    apply_diff(host, temp_apk_path, temp_apk_path, &diffs.apk_diff, &diffs_path)
        .context("Applying diff to APK")?;

    let mut dest_obb_paths = Vec::new();
    for obb_diff in &diffs.obb_diffs {
        let existing_obb = obb_backup_paths
            .iter()
            .find(|p| {
                std::path::Path::new(p)
                    .file_name()
                    .map(|n| n.to_string_lossy() == obb_diff.file_name.as_str())
                    .unwrap_or(false)
            })
            .ok_or(anyhow!("No obb file {} found", obb_diff.file_name))?;

        let obbs_folder = std::path::Path::new(existing_obb)
            .parent()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let dest_obb = format!("{}/{}", obbs_folder, obb_diff.output_file_name);

        apply_diff(host, existing_obb, &dest_obb, obb_diff, &diffs_path)
            .context("Applying diff to OBB")?;
        host.remove_file(existing_obb).context("Deleting old OBB")?;
        dest_obb_paths.push(dest_obb);
    }

    host.remove_dir_all(&diffs_path)?;
    Ok(dest_obb_paths)
}

fn apply_diff<H: Host>(
    host: &mut H,
    from_path: &str,
    to_path: &str,
    diff: &Diff,
    diffs_path: &str,
) -> Result<()> {
    let diff_path = format!("{}/{}", diffs_path, diff.diff_name);
    let diff_content = host.read_file(&diff_path).context("Reading diff file")?;
    let patch = qbsdiff::Bspatch::new(&diff_content).context("Diff file was invalid")?;

    let file_content = host.read_file(from_path).context("Reading original file")?;

    info!("Verifying installation is unmodified");
    let before_crc = ZIP_CRC.checksum(&file_content);
    if before_crc != diff.file_crc {
        return Err(anyhow!(
            "File CRC {} did not match expected value of {}. Your installation is corrupted.",
            before_crc,
            diff.file_crc
        ));
    }

    info!("Applying patch (This step may take a few minutes)");
    let mut output = Vec::with_capacity(diff.output_size);
    {
        let mut writer = BufWriter::new(std::io::Cursor::new(&mut output));
        patch.apply(&file_content, &mut writer)?;
        writer.flush()?;
    }

    host.write_file(to_path, &output)?;
    Ok(())
}

fn download_diffs<H: Host>(
    host: &mut H,
    to_path: &str,
    version_diffs: &VersionDiffs,
) -> Result<()> {
    for diff in &version_diffs.obb_diffs {
        info!("Downloading diff for OBB {}", diff.file_name);
        download_diff_retry(host, diff, to_path)?;
    }
    info!("Downloading diff for APK");
    download_diff_retry(host, &version_diffs.apk_diff, to_path)?;
    Ok(())
}

fn download_diff_retry<H: Host>(host: &mut H, diff: &Diff, to_dir: &str) -> Result<()> {
    let url = resources::get_diff_url(diff);
    let output_path = format!("{}/{}", to_dir, diff.diff_name);
    host.http_get_file(&url, &output_path).context("Downloading diff file")?;
    Ok(())
}
