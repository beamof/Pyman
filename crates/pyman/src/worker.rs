//! Locate a usable Python install for the supervisor to spawn.
//!
//! Every script (and every `python <args>` in CLI mode) runs as an ordinary
//! child process spawned by the supervisor. What lives here is the *discovery*
//! logic the supervisor uses to resolve the interpreter before spawning:
//! [`find_python_on_path`] returns a real install *directory* (used to prepend
//! Python to the child's PATH so `import`s and nested child processes resolve
//! it), and [`find_python_exe`] returns the interpreter *binary* to actually
//! launch.
//!
//! Filtering out the Windows Store App Execution Alias stub is important: the
//! stub is a zero-byte reparse point that pops the Store install UI instead of
//! running the script, so picking it would silently do nothing.

/// Find a real Python install directory.
///
/// "Real" means the directory contains both `python.exe` AND `python3.dll`.
/// This dual check filters out the Windows Store App Execution Alias stubs
/// (`%LOCALAPPDATA%\Microsoft\WindowsApps\python.exe`), which are zero-byte
/// reparse points with no `python3.dll` beside them and would otherwise win
/// the "first python.exe on PATH" race — and launch the Store instead of the
/// interpreter.
///
/// Returns the directory, or `None` if no usable Python is found. The
/// supervisor surfaces that as a friendly error to the user.
pub fn find_python_on_path() -> Option<std::path::PathBuf> {
    // PYMAN_PYTHON override: the user pointed to a specific interpreter. Its
    // directory is where python3.dll lives.
    if let Some(exe) = std::env::var_os("PYMAN_PYTHON") {
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

/// Resolve the interpreter executable itself, or `None` if no usable Python is
/// reachable. Used by the supervisor's CLI mode (script path empty ⇒ run
/// `python <args>` directly), which needs the interpreter binary — not just its
/// directory — to spawn. Falls back to the same discovery rules as
/// [`find_python_on_path`] so the worker (PATH injection) and the CLI child
/// (direct spawn) agree on *which* Python is used.
pub fn find_python_exe() -> Option<std::path::PathBuf> {
    let dir = find_python_on_path()?;
    // On Windows the binary is unambiguously `python.exe`. On Unix a real dir
    // may have only `python3` or only `python` (see `is_real_python_dir`), so we
    // prefer `python3` and fall back to whichever is actually present.
    if cfg!(windows) {
        Some(dir.join("python.exe"))
    } else {
        ["python3", "python"]
            .iter()
            .map(|n| dir.join(n))
            .find(|p| p.is_file())
            .or(Some(dir.join("python3")))
    }
}

/// Friendly Chinese message shown when no usable Python is reachable. Exposed
/// so the supervisor can log it once (rather than the worker failing silently
/// with exit 127, which would look like an opaque crash).
pub const NO_PYTHON_MSG: &str =
    "未找到可用的 Python 解释器。请安装 Python 3.8 或更高版本（安装时勾选 \"Add Python to PATH\"），或设置 PYMAN_PYTHON 环境变量指向 python.exe 的完整路径。已扫描 PATH 但未找到同时含 python.exe 与 python3.dll 的目录。";

/// A directory is a "real" Python install dir if it has both the interpreter
/// binary and (on Windows) its shared library. Checking both is what filters
/// out the Windows Store stub.
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
