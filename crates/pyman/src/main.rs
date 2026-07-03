//! pyman: a single binary that is both the GUI and the per-script worker.
//!
//! PyMan ships as **one** executable. Which role it takes is decided here, at
//! the very top of `main`, by looking at how it was invoked:
//!
//! * **Worker mode** — `pyman --worker <script> [args...]`, or the exe launched
//!   under the name `pyman-worker[.exe]`. The [`worker`] module embeds CPython
//!   via pyo3, runs one script, and exits. This is what the supervisor spawns.
//! * **GUI mode** (default) — everything else: the egui manager window.
//!
//! Collapsing the former two-binary workspace (a GUI exe + a `pyman-worker`
//! exe) into one binary makes release/distribution a single-file affair while
//! preserving true process isolation: each script still runs in its own
//! re-executed worker process, so a crashing script can never take down the UI.
//!
//! On Windows the GUI is linked against the "windows" subsystem (see
//! `build.rs`) so launching it does not pop up a console window — while
//! keeping a normal `main` so `--self-test` and worker output still work when
//! run from an existing terminal.
//!
//! All logic (app UI, history persistence, process supervision, the worker
//! runner) lives in the library part of this crate (`lib.rs`); this file is
//! the thin dispatch + entry point.

use pyman::{app, supervisor, worker};

fn main() -> eframe::Result {
    // --- Role dispatch -----------------------------------------------------
    // Worker mode: explicit `--worker` flag, OR the binary renamed to
    // `pyman-worker` (legacy invocation / convenience alias). The supervisor
    // always uses the `--worker` flag, but we keep the name-based path so the
    // old `pyman-worker script.py` form still works for manual debugging.
    let invoked_as_worker = std::env::args()
        .next()
        .as_deref()
        .map(|arg0| {
            let stem = std::path::Path::new(arg0)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            stem.eq_ignore_ascii_case("pyman-worker")
        })
        .unwrap_or(false);

    if invoked_as_worker || std::env::args().any(|a| a == "--worker") {
        std::process::exit(worker::run());
    }

    // --- GUI mode ----------------------------------------------------------
    // Headless self-test: spawn a worker via the real supervisor, drain its
    // log, and assert the pipeline works. Exits non-zero on failure. Useful in
    // CI / without a display.
    if std::env::args().any(|a| a == "--self-test") {
        match self_test() {
            Ok(()) => return Ok(()),
            Err(msg) => {
                eprintln!("{msg}");
                std::process::exit(1);
            }
        }
    }

    let opts = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([960.0, 640.0])
            .with_min_inner_size([640.0, 420.0])
            .with_title("PyMan")
            .with_icon(std::sync::Arc::new(pyman::icon::icon_data())),
        ..Default::default()
    };
    eframe::run_native(
        "pyman",
        opts,
        Box::new(|cc| {
            app::install_fonts(&cc.egui_ctx);
            Ok(Box::new(app::PymanApp::default()))
        }),
    )
}

/// Headless integration test of the spawn -> stream -> poll pipeline.
///
/// Uses `examples/hello.py` (next to the repo root). Prints the captured log
/// and returns Ok only if the worker produced the expected lines and finished
/// cleanly. Returns an error message string on failure.
fn self_test() -> Result<(), String> {
    use supervisor::{ScriptTask, TaskConfig, TaskState};
    use std::path::PathBuf;

    let script: PathBuf = ["examples", "hello.py"].iter().collect();
    if !script.exists() {
        return Err(format!(
            "self-test: cannot find {} (run from repo root)",
            script.display()
        ));
    }

    let mut task = ScriptTask::spawn(
        1,
        TaskConfig {
            script: script.clone(),
            args: vec!["selftest".into()],
        },
    )
    .map_err(|e| format!("spawn worker: {e}"))?;

    // Drain until the task is no longer running. Cap iterations as a safety
    // net so this can't hang forever on a broken worker.
    for _ in 0..2000 {
        task.poll();
        if task.state != TaskState::Running {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    let (lines, joined): (Vec<String>, String) = {
        let buf = task.log.lock().unwrap();
        let lines: Vec<String> = buf.lines.iter().map(|l| l.text.clone()).collect();
        let joined = lines.join("\n");
        (lines, joined)
    };

    println!(
        "=== self-test captured {} log lines (state={:?}, exit={:?}) ===",
        lines.len(),
        task.state,
        task.exit_code
    );
    for l in &lines {
        println!("  | {l}");
    }

    let ok_state = task.state == TaskState::Finished;
    let ok_greeting = joined.contains("hello from pyman-worker");
    let ok_argv = joined.contains("selftest");
    let ok_done = joined.contains("done");

    if ok_state && ok_greeting && ok_argv && ok_done {
        println!("self-test: PASS");
        Ok(())
    } else {
        Err(format!(
            "self-test: FAIL (state_ok={ok_state}, greeting={ok_greeting}, argv={ok_argv}, done={ok_done})"
        ))
    }
}
