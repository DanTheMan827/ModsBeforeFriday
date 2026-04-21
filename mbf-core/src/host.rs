use anyhow::Result;

/// An entry returned by `Host::list_dir`.
#[derive(Debug)]
pub struct DirEntry {
    /// Full path of the entry.
    pub path: String,
    pub is_dir: bool,
}

/// The output of a command run via `Host::run_command`.
#[derive(Debug)]
pub struct CommandOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: Option<i32>,
}

/// Platform-specific paths and settings provided to `mbf-core` at runtime.
#[derive(Clone, Default)]
pub struct CoreConfig {
    pub apk_id: String,
    /// Template for the QMODs directory; `$` is replaced with the game version.
    pub qmods_dir_template: String,
    pub old_qmods_dir: String,
    pub moddata_nomedia: String,
    pub modloader_dir: String,
    pub late_mods_dir: String,
    pub early_mods_dir: String,
    pub libs_dir: String,
    pub player_data_path: String,
    pub player_data_bak_path: String,
    pub obb_dir: String,
    pub datakeeper_player_data: String,
    pub aux_data_backup: String,
    pub custom_levels_dir: String,
    pub mbf_downloads_dir: String,
    pub temp_dir: String,
    pub res_cache_dir: String,
    pub legacy_dirs: Vec<String>,
}

/// The sole interface through which `mbf-core` accesses external effects.
pub trait Host {
    fn get_config(&self) -> &CoreConfig;

    // --- Filesystem ---
    fn read_file(&mut self, path: &str) -> Result<Vec<u8>>;
    fn write_file(&mut self, path: &str, data: &[u8]) -> Result<()>;
    fn file_exists(&mut self, path: &str) -> bool;
    fn remove_file(&mut self, path: &str) -> Result<()>;
    fn create_dir_all(&mut self, path: &str) -> Result<()>;
    fn remove_dir_all(&mut self, path: &str) -> Result<()>;
    fn copy_file(&mut self, from: &str, to: &str) -> Result<()>;
    fn list_dir(&mut self, path: &str) -> Result<Vec<DirEntry>>;

    // --- Networking ---
    fn http_get(&mut self, url: &str) -> Result<Vec<u8>>;
    /// Downloads `url` to `dest_path`, returning the filename from Content-Disposition if present.
    fn http_get_file(&mut self, url: &str, dest_path: &str) -> Result<Option<String>>;

    // --- Process execution ---
    fn run_command(&mut self, cmd: &str, args: &[&str]) -> Result<CommandOutput>;
}
