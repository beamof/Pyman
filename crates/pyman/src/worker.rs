//! The script-execution "worker" role of the single `pyman` binary.
//!
//! PyMan ships as **one** binary that acts in two roles, dispatched in
//! `main.rs`:
//!
//! * **GUI** (default): the egui manager window.
//! * **Worker**: when invoked as `pyman --worker <script> [args...]` (or when
//!   the binary is launched under the name `pyman-worker`), it embeds CPython
//!   via pyo3 and runs a single Python script to completion, then exits.
//!
//! The supervisor spawns one worker process per running script by re-executing
//! its own executable with the `--worker` flag, so a release bundle is just a
//! single `pyman[.exe]`. The GUI process never initializes Python — pyo3's
//! `auto-initialize` only boots the interpreter on first GIL acquisition, which
//! happens exclusively here in worker mode.
//!
//! Contract with the GUI (unchanged from the old two-binary design):
//!   * stdout/stderr are captured by the GUI and shown as the script's log.
//!     Python's `print()` flows through these streams unchanged, so do NOT
//!     write anything else to stdout from Rust.
//!   * The worker's exit status tells the GUI whether the script succeeded,
//!     failed with a Python exception, or was killed.
//!   * On startup the worker prints one JSON status line to **stderr** so the
//!     GUI can confirm the worker is alive before Python output begins. The
//!     GUI's log viewer prefixes worker lines with `[worker]`.

use pyo3::prelude::*;
use pyo3::types::PyDict;
use std::ffi::CString;

/// Locate a usable CPython and make sure pyo3 can load it at runtime.
///
/// pyo3 0.23 with the default (non-abi3) feature resolves `python3.dll` /
/// `python3XX.dll` at **runtime** via `LoadLibrary` — there's no Python in the
/// exe's import table. Windows' DLL search walks the usual order (app dir,
/// system dirs, then `%PATH%`), so a `pyman.exe` copied to a machine without
/// Python 3.12 fails with "python312.dll not found" the first time the worker
/// touches Python.
///
/// This runs *before* any pyo3 call (see [`run`]) so we can influence that
/// search. Strategy:
///   1. If `PYO3_PYTHON` is set (pyo3's own override), trust it — pyo3 will
///      use that interpreter's directory directly. Nothing to prepend.
///   2. Otherwise scan `%PATH%` for `python.exe`, take its directory, and
///      **prepend** it to `PATH`. That puts the matching `python3.dll` /
///      `python3XX.dll` first in the DLL search. (We don't just read it once:
///      `set_var` mutates the process env that pyo3's loader inherits.)
///   3. If nothing is found, print a friendly Chinese message to stderr — the
///      GUI surfaces worker stderr as log lines — and return `false` so the
///      caller bails out cleanly instead of letting pyo3 panic on a missing
///      DLL.
///
/// Returns `true` if a Python was located (or `PYO3_PYTHON` is set), `false`
/// if no Python is reachable and the worker should abort.
///
/// Note: this is best-effort. pyo3 is bound to a specific CPython minor
/// version at link time (the one CI built against — currently 3.12), so the
/// found Python must be that same series. A 3.11/3.13 install won't satisfy
/// it even when on PATH; the error message names the expected version.
fn ensure_python_on_path() -> bool {
    // Case 1: explicit override. pyo3 reads this itself, so we're done.
    if std::env::var_os("PYO3_PYTHON").is_some() {
        return true;
    }

    // Case 2: walk PATH for a python.exe, remember the first dir that has one.
    // We don't verify the version here — the right 3.12 may live anywhere on
    // the user's PATH; prepending its directory is enough for the DLL search.
    let py_dir = std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path).find_map(|dir| {
            let candidate = dir.join(if cfg!(windows) { "python.exe" } else { "python3" });
            if candidate.is_file() {
                Some(dir)
            } else {
                None
            }
        })
    });

    if let Some(dir) = py_dir {
        // Prepend so Windows' DLL search hits this Python's python3XX.dll
        // before any stale/other version earlier on PATH. Keep the rest of
        // PATH intact so the script can still find its own tools.
        let new_path = match std::env::var_os("PATH") {
            Some(existing) => {
                let mut joined = std::path::PathBuf::from(&dir);
                joined.push(";"); // Windows PATH separator; harmless on Unix
                joined.push(&existing);
                joined.into_os_string()
            }
            None => dir.into_os_string(),
        };
        std::env::set_var("PATH", new_path);
        return true;
    }

    // Case 3: no Python reachable. Tell the user in Chinese (the GUI's log
    // viewer shows worker stderr verbatim), naming the expected version so
    // they know which one to install.
    eprintln!("{{\"kind\":\"error\",\"message\":\"未找到 Python 解释器。请安装 Python 3.12 并加入 PATH，或设置 PYO3_PYTHON 环境变量指向 python.exe。\"}}");
    false
}

/// Run a single script as a worker. Returns the process exit code:
/// `0` = script finished cleanly, `1` = Python raised, `2` = bad invocation.
///
/// The caller (`main`) is expected to feed this straight into
/// `std::process::exit`. We keep it as a plain `i32` (rather than diverging
/// internally) so the dispatch logic in `main.rs` stays readable and the
/// function remains unit-testable.
pub fn run() -> i32 {
    // Before touching pyo3, make sure CPython is discoverable on this machine.
    // On a clean install without Python 3.12, pyo3's deferred LoadLibrary
    // would otherwise panic the process with an opaque "python312.dll not
    // found" — this turns that into a friendly log message the GUI can show.
    if !ensure_python_on_path() {
        return 2;
    }

    // argv[0] is the exe path; drop it, then drop the leading `--worker`
    // dispatch flag if present. We reach worker mode via either `--worker` or
    // being launched as `pyman-worker`; in the latter case the flag is absent,
    // so there's nothing extra to strip. Collecting into a Vec sidesteps
    // `std::env::Args` not being `Clone`.
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--worker") {
        args.remove(0);
    }
    let (script, rest) = match args.split_first() {
        Some((script, rest)) => (script.clone(), rest.to_vec()),
        None => {
            eprintln!(r#"{{"kind":"error","message":"missing script path"}}"#);
            return 2;
        }
    };

    // Hand the remaining args to Python as sys.argv (script path at index 0).
    let argv_json = serde_json::to_string(
        &std::iter::once(script.clone())
            .chain(rest.iter().cloned())
            .collect::<Vec<_>>(),
    )
    .unwrap_or_else(|_| "[]".to_string());
    let argv_setter = format!("import sys\nsys.argv = {argv}\n", argv = argv_json);

    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    eprintln!(r#"{{"kind":"started","started_ms":{started}}}"#);

    let result = Python::with_gil(|py| -> PyResult<PyObject> {
        // Set up sys.argv so the script sees its CLI args.
        let argv_c = CString::new(argv_setter.as_str())
            .map_err(|_| PyErr::new::<pyo3::exceptions::PyValueError, _>("NUL in argv setup"))?;
        py.run(&argv_c, None, None)?;

        // Run the file as __main__ (exec scope), mirroring `python script.py`.
        let code = std::fs::read_to_string(&script).map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyOSError, _>(format!(
                "cannot read script {script}: {e}"
            ))
        })?;
        let code_c = CString::new(code.into_bytes())
            .map_err(|_| PyErr::new::<pyo3::exceptions::PyValueError, _>("NUL byte in script"))?;
        let locals = PyDict::new(py);
        py.run(&code_c, Some(&locals), Some(&locals))
            .map(|()| py.None())
    });

    match result {
        Ok(_) => 0,
        Err(err) => {
            // PyO3 prints unhandled exceptions to stderr in Python's usual
            // traceback format, which the GUI surfaces as part of the log.
            Python::with_gil(|py| err.print(py));
            1
        }
    }
}
