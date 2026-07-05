//! Verifies the interactive stdin pipeline end-to-end: `ScriptTask::write_stdin`
//! delivers user input to the child's stdin, and a script that reads stdin
//! (`examples/echo.py`) echoes it back through the captured log. Also checks the
//! `close_stdin` path (EOF) and the `Stream::Input` echo tag.

use pyman::supervisor::{ScriptTask, Stream, TaskConfig, TaskState};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Locate `examples/echo.py` next to the repo root, regardless of where the
/// test binary runs from (CARGO_MANIFEST_DIR points at `crates/pyman`).
fn echo_script() -> PathBuf {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .unwrap()
        .to_path_buf();
    let p = repo_root.join("examples").join("echo.py");
    assert!(p.exists(), "examples/echo.py must exist for this test");
    p
}

/// Drain the task until `pred(lines)` is satisfied or we time out. Returns the
/// snapshot of lines collected at the moment `pred` first returned true (so the
/// caller can assert on contents), or the last snapshot on timeout.
fn wait_until<F>(task: &mut ScriptTask, timeout: Duration, mut pred: F) -> Vec<String>
where
    F: FnMut(&[String]) -> bool,
{
    let start = Instant::now();
    loop {
        task.poll();
        let lines: Vec<String> = {
            let buf = task.log.lock().unwrap();
            buf.lines.iter().map(|l| l.text.clone()).collect()
        };
        if pred(&lines) {
            return lines;
        }
        if start.elapsed() >= timeout {
            return lines;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
}

#[test]
fn write_stdin_delivers_line_to_echo_script() {
    let mut task = ScriptTask::spawn(
        1,
        TaskConfig {
            script: echo_script(),
            args: vec![],
        },
    )
    .expect("spawn echo.py");

    // Sanity: the task started (didn't fail to spawn).
    assert!(
        matches!(task.state, TaskState::Running),
        "echo.py should start running"
    );

    // Wait for the "ready" banner so we know stdin is being read.
    let ready = wait_until(&mut task, Duration::from_secs(10), |lines| {
        lines.iter().any(|l| l.contains("echo.py ready"))
    });
    assert!(
        ready.iter().any(|l| l.contains("echo.py ready")),
        "echo.py should print a ready banner, got: {ready:?}"
    );

    // Send a line and expect it echoed back.
    task.write_stdin("hello world").expect("write succeeds");
    let echoed = wait_until(&mut task, Duration::from_secs(10), |lines| {
        lines.iter().any(|l| l.contains("echo: hello world"))
    });
    assert!(
        echoed.iter().any(|l| l.contains("echo: hello world")),
        "script should echo the line we sent, got: {echoed:?}"
    );

    // The echoed input should also be tagged Stream::Input in the log buffer.
    {
        let buf = task.log.lock().unwrap();
        let has_input_echo = buf
            .lines
            .iter()
            .any(|l| l.stream == Stream::Input && l.text == "hello world");
        assert!(
            has_input_echo,
            "write_stdin should echo the line as Stream::Input"
        );
    }

    // Closing stdin sends EOF → the script prints "done" and exits cleanly.
    task.close_stdin();
    let done = wait_until(&mut task, Duration::from_secs(10), |lines| {
        lines.iter().any(|l| l.contains("echo.py done"))
    });
    assert!(
        done.iter().any(|l| l.contains("echo.py done")),
        "script should print done on EOF, got: {done:?}"
    );

    // After EOF the task should finish cleanly. Poll a bit more in case the
    // exit hadn't propagated yet when wait_until returned.
    let start = Instant::now();
    while task.state == TaskState::Running && start.elapsed() < Duration::from_secs(5) {
        task.poll();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(task.state, TaskState::Finished, "script should exit 0 on EOF");
}

#[test]
fn stdin_open_reflects_state() {
    let mut task = ScriptTask::spawn(
        2,
        TaskConfig {
            script: echo_script(),
            args: vec![],
        },
    )
    .expect("spawn echo.py");

    // A freshly spawned running task has an open stdin.
    assert!(task.stdin_open(), "stdin should be open right after spawn");

    // After close_stdin, stdin_open becomes false even though the task itself
    // is still (briefly) running.
    task.close_stdin();
    assert!(!task.stdin_open(), "stdin_open false after close_stdin");

    // Drain to completion so we don't leave a dangling child.
    let start = Instant::now();
    while task.state == TaskState::Running && start.elapsed() < Duration::from_secs(5) {
        task.poll();
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn write_stdin_after_close_returns_err() {
    let mut task = ScriptTask::spawn(
        3,
        TaskConfig {
            script: echo_script(),
            args: vec![],
        },
    )
    .expect("spawn echo.py");

    task.close_stdin();
    // Writing after the pipe is closed should fail, not panic.
    let res = task.write_stdin("late");
    assert!(res.is_err(), "write_stdin after close should error");

    // Stop the task to clean up the child.
    task.stop();
}
