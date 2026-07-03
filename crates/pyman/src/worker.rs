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

/// Run a single script as a worker. Returns the process exit code:
/// `0` = script finished cleanly, `1` = Python raised, `2` = bad invocation.
///
/// The caller (`main`) is expected to feed this straight into
/// `std::process::exit`. We keep it as a plain `i32` (rather than diverging
/// internally) so the dispatch logic in `main.rs` stays readable and the
/// function remains unit-testable.
pub fn run() -> i32 {
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
