//! Verifies the new "CLI mode" (empty script path) naming/derive logic:
//! when the script path is empty, the entry is named after the python command
//! line (e.g. `-m http.server` → `python -m http.server`) so two CLI entries
//! with different args stay distinguishable in the list. These are pure-logic
//! tests — no Python or worker is involved.

use pyman::history::HistoryEntry;
use pyman::supervisor::TaskConfig;
use std::path::PathBuf;

#[test]
fn from_input_names_cli_entry_from_args() {
    let cfg = TaskConfig {
        script: PathBuf::new(),
        args: vec!["-m".into(), "http.server".into()],
    };
    let entry = HistoryEntry::from_input(None, cfg, false);
    assert_eq!(entry.name, "python -m http.server");
    // The empty path marker round-trips into history (and reloads as CLI mode).
    assert!(entry.script.as_os_str().is_empty());
}

#[test]
fn from_input_explicit_name_overrides_cli_default() {
    let cfg = TaskConfig {
        script: PathBuf::new(),
        args: vec!["-c".into(), "print(1)".into()],
    };
    let entry = HistoryEntry::from_input(Some("my one-liner"), cfg, false);
    assert_eq!(entry.name, "my one-liner");
}

#[test]
fn from_input_script_mode_still_uses_file_name() {
    // Regression guard: the existing script-mode behavior is unchanged.
    let cfg = TaskConfig {
        script: PathBuf::from("C:/scripts/hello.py"),
        args: vec![],
    };
    let entry = HistoryEntry::from_input(None, cfg, false);
    assert_eq!(entry.name, "hello.py");
}
