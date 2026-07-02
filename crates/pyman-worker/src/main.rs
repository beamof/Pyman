//! pyman-worker: a leaf process that embeds CPython (via pyo3) and runs a
//! single Python script. Spawned by the `pyman` GUI process — one worker per
//! running script.
//!
//! Usage:
//!     pyman-worker <script_path> [arg1 arg2 ...]
//!
//! Contract with the GUI:
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
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let script = match args.next() {
        Some(s) => s,
        None => {
            eprintln!(r#"{{"kind":"error","message":"missing script path"}}"#);
            return ExitCode::from(2);
        }
    };
    let rest: Vec<String> = args.collect();

    // Hand the remaining args to Python as sys.argv (script path at index 0).
    let argv_json = serde_json::to_string(
        &std::iter::once(script.clone()).chain(rest.iter().cloned()).collect::<Vec<_>>(),
    )
    .unwrap_or_else(|_| "[]".to_string());
    let argv_setter = format!(
        "import sys\nsys.argv = {argv}\n",
        argv = argv_json
    );

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
        Ok(_) => ExitCode::SUCCESS,
        Err(err) => {
            // PyO3 prints unhandled exceptions to stderr in Python's usual
            // traceback format, which the GUI surfaces as part of the log.
            Python::with_gil(|py| err.print(py));
            ExitCode::from(1)
        }
    }
}
