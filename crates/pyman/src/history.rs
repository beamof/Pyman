//! Persistence for the script list: every added script is remembered to disk
//! so the next launch reloads it. Each entry carries an `autostart` flag the
//! user can toggle per script — entries with `autostart = true` are re-run on
//! startup, the rest are loaded into the list but left stopped.
//!
//! Storage: a single JSON file (`pyman_history.json`) under the OS config dir:
//!   * Windows: %APPDATA%\pyman\pyman_history.json
//!   * macOS:   ~/Library/Application Support/pyman/pyman_history.json
//!   * Linux:   $XDG_CONFIG_HOME/pyman/pyman_history.json (~/.config/...)
//!
//! Writes are best-effort: a failure to persist only logs a warning and never
//! breaks the running app. Reads on a missing/corrupt file yield an empty list.

use crate::supervisor::TaskConfig;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const FILE_NAME: &str = "pyman_history.json";

/// One persisted script. `autostart` controls whether it is re-spawned on the
/// next app launch. Everything else mirrors the in-memory task config.
#[derive(Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// Arbitrary display name (defaults to the file name on add). Lets the
    /// user label two runs of the same script differently.
    pub name: String,
    pub script: PathBuf,
    pub args: Vec<String>,
    /// If true, this script is started automatically when the app launches.
    pub autostart: bool,
}

impl HistoryEntry {
    /// Build an entry from form input. The name defaults to something short and
    /// recognizable in the list: the script's file name in script mode, or
    /// `python <args>` in CLI mode (empty path) — e.g. `-m http.server`
    /// becomes `python -m http.server`, so two CLI entries with different args
    /// stay distinguishable. A caller-provided non-empty `name` always wins.
    pub fn from_input(name: Option<&str>, config: TaskConfig, autostart: bool) -> Self {
        let name = name
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                if config.is_cli_mode() {
                    config.describe()
                } else {
                    config
                        .script
                        .file_name()
                        .and_then(|s| s.to_str())
                        .map(str::to_owned)
                        .unwrap_or_else(|| config.script.display().to_string())
                }
            });
        Self {
            name,
            script: config.script,
            args: config.args,
            autostart,
        }
    }
}

/// Where the history file lives. Returns None if the OS config dir can't be
/// resolved (very rare; we then just disable persistence).
///
/// Override: if `PYMAN_HISTORY_FILE` is set, use that exact path instead. This
/// is intended for tests so they don't touch the user's real config dir.
fn history_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("PYMAN_HISTORY_FILE") {
        return Some(PathBuf::from(p));
    }
    let base = dirs::config_dir()?;
    Some(base.join("pyman").join(FILE_NAME))
}

/// Load the saved entries. On any error (missing file, IO error, corrupt
/// JSON) we return an empty list and log — never propagate, so a bad history
/// can't prevent the app from starting.
pub fn load() -> Vec<HistoryEntry> {
    let Some(path) = history_path() else {
        return Vec::new();
    };
    match std::fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice::<Vec<HistoryEntry>>(&bytes) {
            Ok(entries) => entries,
            Err(e) => {
                eprintln!("[pyman] history file corrupt, ignoring: {e}");
                Vec::new()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            eprintln!("[pyman] history read failed: {e}");
            Vec::new()
        }
    }
}

/// Save the given entries atomically: write to a temp file in the same dir,
/// then rename over the target. Avoids a half-written file if we crash
/// mid-write. Best-effort — logs on failure.
pub fn save(entries: &[HistoryEntry]) {
    let Some(target) = history_path() else {
        return;
    };
    let Some(dir) = target.parent() else {
        return;
    };

    if let Err(e) = std::fs::create_dir_all(dir) {
        eprintln!("[pyman] cannot create config dir {}: {e}", dir.display());
        return;
    }

    let bytes = match serde_json::to_vec_pretty(entries) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("[pyman] history serialize failed: {e}");
            return;
        }
    };

    let tmp = dir.join(format!("{FILE_NAME}.tmp"));
    if let Err(e) = std::fs::write(&tmp, &bytes) {
        eprintln!("[pyman] history temp write failed: {e}");
        return;
    }
    // rename is atomic on the same filesystem; the temp lives in the same dir.
    if let Err(e) = persistent_rename(&tmp, &target) {
        eprintln!("[pyman] history rename failed: {e}");
        let _ = std::fs::remove_file(&tmp);
    }
}

/// rename() across directories can fail on Windows; since tmp and target share
/// a dir here a plain rename is fine, but wrap it so we can swap strategies
/// later if needed.
fn persistent_rename(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::rename(from, to)
}
