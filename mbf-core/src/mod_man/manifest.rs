//! Structures for the deserialization for the QMOD `mod.json` file.

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

/// Model for the `mod.json` manifest within a QMOD.
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
pub struct ModInfo {
    #[serde(rename(serialize = "_QPVersion", deserialize = "_QPVersion"))]
    pub schema_version: Version,
    pub name: String,
    pub id: String,
    pub modloader: Option<String>,
    pub author: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub porter: Option<String>,
    pub version: Version,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_library: Option<bool>,
    pub dependencies: Vec<ModDependency>,
    pub mod_files: Vec<String>,
    pub late_mod_files: Vec<String>,
    pub library_files: Vec<String>,
    pub file_copies: Vec<FileCopy>,
    pub copy_extensions: Vec<CopyExtension>,
}

impl Default for ModInfo {
    fn default() -> Self {
        Self {
            schema_version: Version::new(1, 2, 0),
            name: Default::default(),
            id: Default::default(),
            author: Default::default(),
            porter: Default::default(),
            version: semver::Version::new(0, 0, 0),
            package_id: Default::default(),
            package_version: Default::default(),
            description: Default::default(),
            cover_image: Default::default(),
            is_library: Default::default(),
            dependencies: Default::default(),
            mod_files: Default::default(),
            library_files: Default::default(),
            file_copies: Default::default(),
            copy_extensions: Default::default(),
            modloader: Some("Scotland2".into()),
            late_mod_files: Default::default(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModDependency {
    #[serde(rename = "version")]
    pub version_range: VersionReq,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "downloadIfMissing")]
    pub mod_link: Option<String>,
    #[serde(default = "true_default")]
    pub required: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct FileCopy {
    pub name: String,
    pub destination: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct CopyExtension {
    pub extension: String,
    pub destination: String,
}

fn true_default() -> bool {
    true
}
