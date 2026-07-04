//! Extract the bundled worker binary to disk so the supervisor can spawn it.
//!
//! PyMan ships as a single `pyman[.exe]` download, but the script-execution
//! worker (the part that links CPython) is a *separate* binary — keeping
//! `python3.dll` out of the GUI's import table so the GUI starts on machines
//! without Python (see `build.rs` for the why). At build time the compiled
//! `pyman-worker[.exe]` is `include_bytes!`'d into the GUI binary as
//! [`WORKER_BYTES`]; at runtime this module writes those bytes to the user's
//! per-app data directory once (cached by a content stamp) and hands the path
//! back to the supervisor to spawn.
//!
//! Why a data dir and not next to the exe: the install dir may be read-only
//! (Program Files, or the quarantine folder an unzip tool drops things into),
//! and we don't want writes there. The data dir (`%APPDATA%\..\Local\pyman` on
//! Windows via `dirs::data_dir`) is per-user, writable, and conventional.

use std::fs;
use std::io::Write;
use std::path::PathBuf;

/// The worker binary baked into this GUI exe at build time by `build.rs`.
/// Sourcing it via `include_bytes!` (rather than reading a file at runtime)
/// keeps distribution a single self-contained download.
pub const WORKER_BYTES: &[u8] = include_bytes!(env!("PYMAN_WORKER_BIN"));

/// A short, stable fingerprint of the embedded worker, used as the on-disk
/// cache key. We key on length + the first/last 32 bytes — enough to detect a
/// different build (which differs throughout) without hashing all ~5 MB every
/// startup. Collisions across real builds are astronomically unlikely; a
/// mismatch just triggers a re-extract (correctness, not safety).
fn stamp_of(bytes: &[u8]) -> String {
    let n = bytes.len();
    let head: Vec<u8> = bytes.iter().take(32).copied().collect();
    let tail: Vec<u8> = bytes.iter().rev().take(32).copied().collect();
    format!("{n:x}-{head:x?}-{tail:x?}")
}

/// The directory we extract the worker into: `<data_local_dir>/pyman/`.
///
/// We use the *local* data dir (not `data_dir`): on Windows that's
/// `%LOCALAPPDATA%` (non-roaming), so the ~200 KB worker binary isn't needlessly
/// synced across machines at logon. `dirs::data_dir()` would put it in Roaming.
fn worker_dir() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("pyman"))
}

/// The full path of the extracted worker executable.
fn worker_path() -> Option<PathBuf> {
    worker_dir().map(|d| d.join(format!("pyman-worker{}", std::env::consts::EXE_SUFFIX)))
}

/// The path to a tiny file recording the stamp of the bytes we last wrote, so
/// repeated launches skip the rewrite when the embedded worker hasn't changed.
fn stamp_path() -> Option<PathBuf> {
    worker_dir().map(|d| d.join("pyman-worker.stamp"))
}

/// Ensure the worker binary is on disk and up to date, returning its path.
///
/// Idempotent: if the extracted file's recorded stamp matches the embedded
/// bytes' stamp, this is a no-op (avoids re-writing ~5 MB — and the Defender
/// rescans that trigger — on every launch). Otherwise writes the new bytes
/// atomically (temp file + rename) and updates the stamp.
///
/// Returns an error string (rather than `io::Error`) so the supervisor can
/// surface it directly as a friendly log line.
pub fn ensure_worker() -> Result<PathBuf, String> {
    let dir = worker_dir().ok_or_else(|| {
        "无法定位用户数据目录（%APPDATA%/XDG_DATA_HOME），无处放置 worker。".to_string()
    })?;
    let path = worker_path().ok_or_else(|| "无法计算 worker 路径。".to_string())?;
    let stamp_file = stamp_path().ok_or_else(|| "无法计算 stamp 路径。".to_string())?;

    let want = stamp_of(WORKER_BYTES);

    // Fast path: already extracted and unchanged.
    let up_to_date = path.is_file()
        && fs::read_to_string(&stamp_file).map(|s| s.trim() == want).unwrap_or(false);
    if up_to_date {
        return Ok(path);
    }

    // Slow path: (re)create the dir and write the worker atomically.
    fs::create_dir_all(&dir)
        .map_err(|e| format!("无法创建目录 {}: {e}", dir.display()))?;

    let mut tmp = path.clone();
    let pid = std::process::id();
    tmp.set_extension(format!("tmp.{pid}{}", std::env::consts::EXE_SUFFIX));
    {
        let mut f = fs::File::create(&tmp)
            .map_err(|e| format!("无法创建临时文件 {}: {e}", tmp.display()))?;
        f.write_all(WORKER_BYTES)
            .map_err(|e| format!("写入 worker 失败 {}: {e}", tmp.display()))?;
        f.sync_all().ok(); // best-effort; rename is the real durability gate
    }

    // rename over the destination. On Windows this replaces an existing file
    // atomically only if both are on the same volume (they are — same dir).
    fs::rename(&tmp, &path)
        .map_err(|e| format!("无法把 worker 移到位 {}: {e}", path.display()))?;

    // Record the stamp last, so a crash mid-write leaves a stale stamp and we
    // re-extract next time rather than trusting a half-written binary.
    if let Err(e) = fs::write(&stamp_file, &want) {
        // Non-fatal: worst case we re-extract next launch.
        eprintln!("embed: failed to write stamp {}: {e}", stamp_file.display());
    }

    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamp_is_stable_for_same_bytes() {
        assert_eq!(stamp_of(WORKER_BYTES), stamp_of(WORKER_BYTES));
    }

    #[test]
    fn stamp_changes_when_bytes_differ() {
        let a = stamp_of(WORKER_BYTES);
        // Same length, different first byte → different stamp.
        let mut mutated = WORKER_BYTES.to_vec();
        mutated[0] ^= 0xff;
        let b = stamp_of(&mutated);
        assert_ne!(a, b);
    }

    #[test]
    fn worker_bytes_look_like_a_pe_image() {
        // Sanity: the embedded bytes should start with the "MZ" DOS header.
        assert!(WORKER_BYTES.len() > 64, "worker bytes suspiciously small");
        assert_eq!(&WORKER_BYTES[0..2], b"MZ", "embedded worker is not a PE/EXE");
    }
}
