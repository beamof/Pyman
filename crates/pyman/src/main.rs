//! pyman: the GUI entry point.
//!
//! PyMan is a single-binary download but *two* crates under the hood. This
//! binary is the egui manager window only; it does NOT link pyo3, so its
//! loader never demands `python3.dll` and the GUI starts even on machines
//! without Python installed. The script-execution worker is a separate
//! `pyman-worker` binary (the sole place CPython is linked), embedded into
//! this exe at build time (`build.rs` + `embed`) and spawned one-process-per-
//! script by the supervisor — so a crashing script still can't take down the
//! UI, and distribution stays a single downloaded file.
//!
//! On Windows the GUI is linked against the "windows" subsystem (see
//! `build.rs`) so launching it does not pop up a console window — while
//! keeping a normal `main` so `--self-test` output works when run from an
//! existing terminal.
//!
//! All logic (app UI, history persistence, process supervision, worker
//! discovery, embedded-worker extraction) lives in the library part of this
//! crate (`lib.rs`); this file is the thin entry point.

use pyman::{app, supervisor};

fn main() -> eframe::Result {
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
