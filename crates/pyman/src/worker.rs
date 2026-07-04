//! Locate a usable CPython install for the worker child process.
//!
//! The GUI crate no longer links pyo3 (the CPython embedding lives in the
//! separate `pyman-worker` crate / binary). What stays here is the *discovery*
//! logic the supervisor uses to find a real Python install directory before
//! spawning a worker: the worker links `python3.dll` as a hard load-time
//! import, so the Windows loader must find `python3.dll` at worker **process
//! startup** — before `main` even runs. Any PATH fixup done *inside* the
//! worker is too late; the process exits 127 before its code executes.
//!
//! So the supervisor calls [`find_python_on_path`] *before* spawning the
//! worker and injects the returned directory into the child's `PATH`
//! (see `supervisor::ScriptTask::spawn`), letting the loader resolve
//! `python3.dll` from there on startup.

/// Find a real CPython install directory the worker can use.
///
/// "Real" means the directory contains both `python.exe` AND `python3.dll`.
/// This dual check filters out the Windows Store App Execution Alias stubs
/// (`%LOCALAPPDATA%\Microsoft\WindowsApps\python.exe`), which are zero-byte
/// reparse points with no `python3.dll` beside them and would otherwise win
/// the "first python.exe on PATH" race.
///
/// Returns the directory, or `None` if no usable Python is found. The
/// supervisor surfaces that as a friendly error to the user.
pub fn find_python_on_path() -> Option<std::path::PathBuf> {
    // PYO3_PYTHON override: the user pointed pyo3 at a specific interpreter.
    // Its directory is where python3.dll lives.
    if let Some(exe) = std::env::var_os("PYO3_PYTHON") {
        if let Some(parent) = std::path::Path::new(&exe).parent() {
            if is_real_python_dir(parent) {
                return Some(parent.to_path_buf());
            }
        }
    }
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path).find_map(|dir| {
            if is_real_python_dir(&dir) {
                Some(dir)
            } else {
                None
            }
        })
    })
}

/// Friendly Chinese message shown when no usable Python is reachable. Exposed
/// so the supervisor can log it once (rather than the worker failing silently
/// with exit 127, which would look like an opaque crash).
pub const NO_PYTHON_MSG: &str =
    "未找到可用的 Python 解释器。请安装 Python 3.8 或更高版本（安装时勾选 \"Add Python to PATH\"），或设置 PYO3_PYTHON 环境变量指向 python.exe 的完整路径。已扫描 PATH 但未找到同时含 python.exe 与 python3.dll 的目录。";

/// A directory is a "real" CPython install dir if it has both the interpreter
/// binary and the abi3 entry shared object. Checking both is what filters out
/// the Windows Store stub.
#[cfg(windows)]
fn is_real_python_dir(dir: &std::path::Path) -> bool {
    dir.join("python.exe").is_file() && dir.join("python3.dll").is_file()
}

#[cfg(not(windows))]
fn is_real_python_dir(dir: &std::path::Path) -> bool {
    // On Unix the shared object name varies (libpython3.so / .so.1.0 / dylib);
    // requiring the interpreter binary is enough — Unix dynamic loaders honor
    // LD_LIBRARY_PATH and the install usually sets that up correctly.
    dir.join("python3").is_file() || dir.join("python").is_file()
}
