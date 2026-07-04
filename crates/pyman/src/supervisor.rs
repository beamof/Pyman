//! Process supervision for per-script worker child processes.
//!
//! One [`ScriptTask`] exists per running script. Each task owns a child
//! process — the same `pyman` binary re-executed in worker mode via the
//! `--worker` flag — and a background thread that reads its merged
//! stdout/stderr line-by-line into a ring buffer the UI can read.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// How many log lines we keep per script before dropping the oldest.
const LOG_CAP: usize = 10_000;

/// A single log line, tagged with the stream it came from.
#[derive(Clone, Debug)]
pub struct LogLine {
    pub ts_ms: u128,
    pub stream: Stream,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskState {
    /// Worker process is alive.
    Running,
    /// Worker exited cleanly (script returned 0 / no exception).
    Finished,
    /// Worker exited non-zero, or could not be spawned.
    Failed,
    /// User asked us to stop it; we're killing the process.
    Stopped,
}

/// Immutable configuration for a task. Captured at spawn time so editing the
/// form later doesn't mutate an in-flight run.
#[derive(Clone, Debug)]
pub struct TaskConfig {
    pub script: PathBuf,
    pub args: Vec<String>,
}

impl TaskConfig {
    /// CLI mode: the user left the script path empty and put Python's own
    /// command-line (e.g. `-m http.server`) in the args box. Instead of running
    /// a script file, the supervisor spawns `python <args>` directly. An empty
    /// `script` PathBuf is the marker.
    pub fn is_cli_mode(&self) -> bool {
        self.script.as_os_str().is_empty()
    }

    /// Human-readable one-liner for the task, used in log headers and the
    /// stopped-entry view. Script mode shows the path; CLI mode shows
    /// `python <args>` (e.g. `python -m http.server`), so the row stays
    /// recognizable without a file name.
    pub fn describe(&self) -> String {
        if self.is_cli_mode() {
            format!("python {}", self.args.join(" "))
        } else {
            self.script.display().to_string()
        }
    }
}

/// Shared log buffer. The reader thread writes; the UI thread reads.
pub type SharedLog = Arc<Mutex<LogBuffer>>;

pub struct LogBuffer {
    pub lines: VecDeque<LogLine>,
    /// Highest line index the UI has already shown — used to detect new lines.
    pub seen_up_to: usize,
}

impl LogBuffer {
    fn new() -> Self {
        Self {
            lines: VecDeque::with_capacity(1024),
            seen_up_to: 0,
        }
    }

    fn push(&mut self, stream: Stream, text: String) {
        if self.lines.len() >= LOG_CAP {
            self.lines.pop_front();
        }
        let ts_ms = now_ms();
        self.lines.push_back(LogLine {
            ts_ms,
            stream,
            text,
        });
    }

    /// Drop every buffered line. Used by the UI's "clear log" button — the
    /// reader threads keep running and will append fresh lines afterwards.
    fn clear(&mut self) {
        self.lines.clear();
        self.seen_up_to = 0;
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// One running script and everything we need to manage it.
pub struct ScriptTask {
    pub id: u64,
    pub config: TaskConfig,
    pub state: TaskState,
    pub started_ms: u128,
    pub ended_ms: Option<u128>,
    pub exit_code: Option<i32>,
    pub log: SharedLog,
    child: Option<Child>,
}

impl ScriptTask {
    /// Spawn `python` for `config`. Returns the task on success.
    ///
    /// Both run modes spawn the **real Python interpreter** as a child process
    /// — the GUI never embeds CPython, so it links no pyo3 and has no
    /// `python3.dll` load-time dependency (the GUI starts on machines without
    /// Python). The two modes differ only in how `config` becomes a command
    /// line; they share one supervisor pipeline (captured stdout/stderr,
    /// line-buffered log, exit-code polling):
    ///   * **Script mode** (default): `config.script` is a file path →
    ///     `python <script> <args>`. Running the file directly via the
    ///     interpreter is exactly what the old embedded worker did (read file,
    ///     set `sys.argv`, exec as `__main__`), so behavior is unchanged.
    ///   * **CLI mode** (`config.is_cli_mode()`): the script path is empty and
    ///     `args` holds Python's own command line (e.g. `-m http.server`) →
    ///     `python <args>`. This is the faithful way to run `python <args>`:
    ///     every flag (`-m` / `-c` / `-O` …) is handled by the real interpreter.
    pub fn spawn(id: u64, config: TaskConfig) -> std::io::Result<Self> {
        // Both modes need a real Python install to spawn. If none is reachable
        // we synthesize a Failed task with a friendly message rather than
        // letting `python` fail to resolve (or spawn nothing at all).
        let py_dir = match crate::worker::find_python_on_path() {
            Some(d) => d,
            None => {
                return Ok(failed_task(
                    id,
                    config,
                    crate::worker::NO_PYTHON_MSG.to_string(),
                ));
            }
        };
        let exe = match crate::worker::find_python_exe() {
            Some(p) => p,
            // find_python_on_path() succeeded above, so this is unreachable in
            // practice — but keep it defensive instead of an expect.
            None => return Ok(failed_task(id, config, "找不到 python 可执行文件".into())),
        };

        // CLI mode with no args would drop the user into the REPL (which never
        // exits and can't be meaningfully managed here), so reject it up front.
        if config.is_cli_mode() {
            if config.args.is_empty() {
                return Ok(failed_task(
                    id,
                    config,
                    "参数为空：脚本路径留空时，请把要传给 python 的参数填到『参数』里（例如 -m http.server）。".into(),
                ));
            }
            let mut cmd = Command::new(&exe);
            cmd.args(&config.args);
            return spawn_child(id, config, cmd, &py_dir, &exe);
        }

        // Script mode: `python <script> <args>`.
        let mut cmd = Command::new(&exe);
        cmd.arg(&config.script);
        cmd.args(&config.args);
        spawn_child(id, config, cmd, &py_dir, &exe)
    }

    /// Request termination. Sends a kill (not a graceful signal) because
    /// scripts may be stuck in C extensions; reliability beats politeness
    /// here. Idempotent.
    pub fn stop(&mut self) {
        if self.state != TaskState::Running {
            return;
        }
        self.state = TaskState::Stopped;
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
        }
    }

    /// Clear the buffered log lines for this task. The log is behind an
    /// `Arc<Mutex<...>>`, so this only needs `&self` — the UI can call it
    /// directly without going through a deferred action (which would conflict
    /// with the immutable borrow held while drawing the list).
    pub fn clear_log(&self) {
        self.log.lock().unwrap().clear();
    }

    /// Poll the child process: if it has exited, record exit code + state.
    pub fn poll(&mut self) {
        if self.state != TaskState::Running {
            return;
        }
        let Some(child) = self.child.as_mut() else {
            return;
        };
        match child.try_wait() {
            Ok(Some(status)) => {
                self.exit_code = status.code();
                self.ended_ms = Some(now_ms());
                // A user-initiated stop sets Stopped before the OS reports the
                // kill; otherwise classify by exit code.
                if self.state == TaskState::Running {
                    self.state = if status.success() {
                        TaskState::Finished
                    } else {
                        TaskState::Failed
                    };
                }
            }
            Ok(None) => {} // still running
            Err(_) => {
                self.state = TaskState::Failed;
                self.ended_ms = Some(now_ms());
            }
        }
    }
}

/// Synthesize a Failed task that carries a single stderr line explaining why
/// the spawn never produced a real child. Used for every "we couldn't run it"
/// path (no Python, no worker binary, bad args) so the user sees a friendly
/// message in the log instead of an opaque crash or silent nothing. Centralized
/// here to keep the spawn method readable and the field set consistent.
fn failed_task(id: u64, config: TaskConfig, message: String) -> ScriptTask {
    let log = Arc::new(Mutex::new(LogBuffer::new()));
    log.lock().unwrap().push(Stream::Stderr, message);
    ScriptTask {
        id,
        config,
        state: TaskState::Failed,
        started_ms: now_ms(),
        ended_ms: Some(now_ms()),
        exit_code: None,
        log,
        child: None,
    }
}

/// Finish configuring `cmd` (PATH, stdio, no-console window), spawn it, and
/// wire its piped stdout/stderr into reader threads. Shared by both run modes
/// so the log/state pipeline is identical for `python <script>` and
/// `python <args>`. `py_dir` is prepended to the child's PATH (so `import`s /
/// nested child processes inside the script resolve the same Python); `exe` is
/// only used to name the binary in a spawn-failure message.
fn spawn_child(
    id: u64,
    config: TaskConfig,
    mut cmd: Command,
    py_dir: &std::path::Path,
    exe: &std::path::Path,
) -> std::io::Result<ScriptTask> {
    // Prepend the Python directory to the child's PATH. `exe` is an absolute
    // path so the interpreter itself doesn't need PATH to start, but the
    // script (or its children) may `import` or spawn tools that expect Python
    // to be reachable — keeping its dir first mirrors what a user gets in a
    // terminal with Python on PATH.
    let sep = if cfg!(windows) { ";" } else { ":" };
    let new_path = match std::env::var_os("PATH") {
        Some(existing) => {
            let mut s = py_dir.as_os_str().to_owned();
            s.push(sep);
            s.push(&existing);
            s
        }
        None => py_dir.as_os_str().to_owned(),
    };
    cmd.env("PATH", new_path);

    // Merge stderr into stdout via a single piped handle so ordering is
    // preserved and the UI sees a single coherent stream. We tag each line with
    // its origin in the reader threads below.
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.stdin(Stdio::null());

    // On Windows, a console-subsystem child spawned from a GUI (no-console)
    // parent gets a brand-new console window allocated for it — that's the
    // black terminal that flashes up when a script runs. CREATE_NO_WINDOW
    // (0x08000000) tells the loader not to create any console for the child.
    // The piped stdout/stderr above still work: the child writes to the pipes,
    // not a console, so output still reaches our reader threads.
    #[cfg(target_os = "windows")]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return Ok(failed_task(
                id,
                config,
                format!("启动失败 {}: {e}", exe.display()),
            ));
        }
    };

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");
    let log = Arc::new(Mutex::new(LogBuffer::new()));

    // Two reader threads, one per stream, both feeding the same buffer.
    spawn_reader(stdout, Arc::clone(&log), Stream::Stdout);
    spawn_reader(stderr, Arc::clone(&log), Stream::Stderr);

    Ok(ScriptTask {
        id,
        config,
        state: TaskState::Running,
        started_ms: now_ms(),
        ended_ms: None,
        exit_code: None,
        log,
        child: Some(child),
    })
}

fn spawn_reader<R: std::io::Read + Send + 'static>(
    stream: R,
    log: SharedLog,
    tag: Stream,
) {
    std::thread::Builder::new()
        .name(match tag {
            Stream::Stdout => "pyman-stdout".into(),
            Stream::Stderr => "pyman-stderr".into(),
        })
        .spawn(move || {
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            loop {
                line.clear();
                let n = match reader.read_line(&mut line) {
                    Ok(n) => n,
                    Err(_) => break,
                };
                if n == 0 {
                    break;
                }
                // Strip the trailing newline; the UI adds its own.
                let trimmed = line.trim_end_matches(['\r', '\n']);
                log.lock().unwrap().push(tag, trimmed.to_string());
            }
        })
        .expect("spawn reader thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_mode_is_not_cli() {
        let cfg = TaskConfig {
            script: PathBuf::from("examples/hello.py"),
            args: vec!["x".into()],
        };
        assert!(!cfg.is_cli_mode());
        assert_eq!(cfg.describe(), "examples/hello.py");
    }

    #[test]
    fn empty_path_is_cli_mode() {
        // Empty PathBuf is the CLI-mode marker: an empty path means "no script,
        // run python with the args directly".
        let cfg = TaskConfig {
            script: PathBuf::new(),
            args: vec!["-m".into(), "http.server".into()],
        };
        assert!(cfg.is_cli_mode());
        // describe() folds args into `python <args>` so the row is recognizable
        // without a file name.
        assert_eq!(cfg.describe(), "python -m http.server");
    }

    #[test]
    fn cli_mode_with_no_args_describes_plain_python() {
        // Edge case: no args yet. describe() still returns "python " (with a
        // trailing space) — acceptable, and add_entry/spawn reject this before
        // it ever runs.
        let cfg = TaskConfig {
            script: PathBuf::new(),
            args: vec![],
        };
        assert!(cfg.is_cli_mode());
        assert_eq!(cfg.describe(), "python ");
    }
}
