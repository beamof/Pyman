//! Verifies the history save/load round-trip: entries written by `save()`
//! are recovered verbatim by `load()`, including the `autostart` flag that
//! drives next-launch behavior. Runs against a throwaway temp file via the
//! `PYMAN_HISTORY_FILE` override so it never touches the user's real config.

use pyman::history::{self, HistoryEntry};
use std::path::PathBuf;
use std::sync::Mutex;

// `history` must be re-exported from the crate root for a test (integration
// tests only see the public crate API). We reference it via the crate; if the
// crate doesn't re-export it, this test fails to compile — which is itself a
// useful signal that the module isn't wired in.

// All three tests below share the PYMAN_HISTORY_FILE env var, which is global
// state — so they would race if run in parallel. This mutex serializes them.
// (Once taken it's held for the test's whole body via the guard binding.)
static HISTORY_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn save_then_load_round_trips_entries_and_autostart() {
    let _guard = HISTORY_LOCK.lock().unwrap();
    let tmp = temp_path("roundtrip.json");
    std::env::set_var("PYMAN_HISTORY_FILE", &tmp);

    let entries = vec![
        HistoryEntry {
            name: "hello".into(),
            script: PathBuf::from("C:/scripts/hello.py"),
            args: vec!["a".into(), "b".into()],
            autostart: true,
        },
        HistoryEntry {
            name: "loop".into(),
            script: PathBuf::from("/home/u/loop.py"),
            args: vec![],
            autostart: false,
        },
    ];

    history::save(&entries);

    let loaded = history::load();
    assert_eq!(loaded.len(), 2, "expected 2 entries round-tripped");
    assert_eq!(loaded[0].name, "hello");
    assert_eq!(loaded[0].script, PathBuf::from("C:/scripts/hello.py"));
    assert_eq!(loaded[0].args, vec!["a".to_string(), "b".to_string()]);
    assert!(loaded[0].autostart, "autostart=true must survive round-trip");
    assert_eq!(loaded[1].name, "loop");
    assert!(!loaded[1].autostart, "autostart=false must survive round-trip");

    let _ = std::fs::remove_file(&tmp);
    std::env::remove_var("PYMAN_HISTORY_FILE");
}

#[test]
fn load_missing_file_returns_empty() {
    let _guard = HISTORY_LOCK.lock().unwrap();
    let tmp = temp_path("does_not_exist.json");
    // Make sure the file is absent.
    let _ = std::fs::remove_file(&tmp);
    std::env::set_var("PYMAN_HISTORY_FILE", &tmp);

    let loaded = history::load();
    assert!(loaded.is_empty(), "missing history file should yield empty list");

    std::env::remove_var("PYMAN_HISTORY_FILE");
}

#[test]
fn load_corrupt_file_returns_empty() {
    let _guard = HISTORY_LOCK.lock().unwrap();
    let tmp = temp_path("corrupt.json");
    std::fs::write(&tmp, b"{ this is not valid json ").unwrap();
    std::env::set_var("PYMAN_HISTORY_FILE", &tmp);

    let loaded = history::load();
    assert!(loaded.is_empty(), "corrupt history should not panic, just be empty");

    let _ = std::fs::remove_file(&tmp);
    std::env::remove_var("PYMAN_HISTORY_FILE");
}

/// Unique temp path per-test to avoid races between parallel tests sharing the
/// same override.
fn temp_path(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir();
    dir.join(format!("pyman_hist_test_{n}_{name}"))
}
