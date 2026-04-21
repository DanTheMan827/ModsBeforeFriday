use std::collections::HashMap;

use anyhow::{Context, Result};
use log::info;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use semver::Version;

use crate::host::Host;

// ---------- Data models ----------

pub type DiffIndex = Vec<VersionDiffs>;

#[derive(Deserialize, Serialize, Clone)]
pub struct CoreMod {
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "version")]
    pub version: Version,
    #[serde(rename = "downloadLink")]
    pub download_url: String,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct VersionedCoreMods {
    pub mods: Vec<CoreMod>,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct VersionDiffs {
    pub from_version: String,
    pub to_version: String,
    pub apk_diff: Diff,
    pub obb_diffs: Vec<Diff>,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Diff {
    pub diff_name: String,
    pub file_name: String,
    pub file_crc: u32,
    pub output_file_name: String,
    pub output_crc: u32,
    pub output_size: usize,
}

pub type ModRepo = HashMap<String, Vec<ModRepoMod>>;

#[derive(Clone, Deserialize)]
pub struct ModRepoMod {
    pub id: String,
    pub version: Version,
    pub download: String,
}

pub type CoreModIndex = HashMap<String, VersionedCoreMods>;

// ---------- JsonPullError ----------

pub enum JsonPullError {
    FetchError(anyhow::Error),
    ParseError(anyhow::Error),
}

impl From<JsonPullError> for anyhow::Error {
    fn from(e: JsonPullError) -> Self {
        match e {
            JsonPullError::FetchError(e) => e,
            JsonPullError::ParseError(e) => e,
        }
    }
}

// ---------- ResCache ----------

pub struct ResCache {
    cache_dir: String,
    memory_cache: HashMap<String, Vec<u8>>,
}

impl ResCache {
    pub fn new(cache_dir: String) -> Self {
        Self {
            cache_dir,
            memory_cache: HashMap::new(),
        }
    }

    pub fn get_json_cached<T: DeserializeOwned, H: Host>(
        &mut self,
        host: &mut H,
        url: &str,
        cache_file: &str,
    ) -> Result<T, JsonPullError> {
        if let Some(bytes) = self.memory_cache.get(cache_file) {
            return serde_json::from_slice(bytes).map_err(|e| JsonPullError::ParseError(e.into()));
        }

        let cache_path = format!("{}/{}", self.cache_dir, cache_file);

        if host.file_exists(&cache_path) {
            if let Ok(bytes) = host.read_file(&cache_path) {
                if let Ok(val) = serde_json::from_slice::<T>(&bytes) {
                    self.memory_cache.insert(cache_file.to_string(), bytes);
                    return Ok(val);
                }
            }
        }

        info!("Fetching {}", url);
        let bytes = host.http_get(url).map_err(JsonPullError::FetchError)?;
        let _ = host.write_file(&cache_path, &bytes);

        let result = serde_json::from_slice::<T>(&bytes)
            .map_err(|e| JsonPullError::ParseError(e.into()))?;
        self.memory_cache.insert(cache_file.to_string(), bytes);
        Ok(result)
    }
}

// ---------- Resource fetch functions ----------

const CORE_MODS_URL: &str =
    "https://raw.githubusercontent.com/QuestPackageManager/bs-coremods/main/core_mods.json";

pub fn fetch_core_mods<H: Host>(
    host: &mut H,
    res_cache: &mut ResCache,
    override_core_mod_url: Option<String>,
) -> Result<CoreModIndex, JsonPullError> {
    match override_core_mod_url {
        Some(url) => res_cache.get_json_cached(host, &url, "core_mods_override.json"),
        None => res_cache.get_json_cached(host, CORE_MODS_URL, "core_mods.json"),
    }
}

const UNITY_INDEX_URL: &str =
    "https://raw.githubusercontent.com/Lauriethefish/QuestUnstrippedUnity/main/index.json";
const UNITY_VER_FORMAT: &str =
    "https://raw.githubusercontent.com/Lauriethefish/QuestUnstrippedUnity/main/versions/{0}.so";

pub fn get_libunity_url<H: Host>(
    host: &mut H,
    res_cache: &mut ResCache,
    apk_id: &str,
    version: &str,
) -> Result<Option<String>> {
    let unity_index: HashMap<String, HashMap<String, String>> =
        res_cache.get_json_cached(host, UNITY_INDEX_URL, "libunity_index.json")?;

    let app_index = match unity_index.get(apk_id) {
        Some(idx) => idx,
        None => return Ok(None),
    };
    match app_index.get(version) {
        Some(unity_version) => Ok(Some(UNITY_VER_FORMAT.replace("{0}", unity_version))),
        None => Ok(None),
    }
}

const DIFF_INDEX_STEM: &str =
    "https://github.com/Lauriethefish/mbf-diffs/releases/download/1.0.0";

pub fn get_diff_index<H: Host>(
    host: &mut H,
    res_cache: &mut ResCache,
) -> Result<DiffIndex, JsonPullError> {
    res_cache.get_json_cached(
        host,
        &format!("{DIFF_INDEX_STEM}/index.json"),
        "diff_index.json",
    )
}

pub fn get_diff_url(diff: &Diff) -> String {
    format!("{DIFF_INDEX_STEM}/{}", diff.diff_name)
}

const MANIFEST_FORMAT: &str =
    "https://github.com/Lauriethefish/mbf-manifests/releases/download/1.0.0/{0}.xml";

pub fn get_manifest_axml<H: Host>(host: &mut H, version: &str) -> Result<Vec<u8>> {
    let manifest_url = MANIFEST_FORMAT.replace("{0}", version);
    info!("Fetching manifest for version {version}");
    host.http_get(&manifest_url)
        .context("Fetching manifest for BS ver")
}

const MOD_REPO_URL: &str = "https://mods.bsquest.xyz/mods.json";

pub fn get_mod_repo<H: Host>(host: &mut H, res_cache: &mut ResCache) -> Result<ModRepo> {
    Ok(res_cache.get_json_cached(host, MOD_REPO_URL, "mod_repo.json")?)
}
