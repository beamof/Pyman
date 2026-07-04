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
    /// Spawn a worker for `config`. Returns the task on success.
    ///
    /// The worker is the **separate** `pyman-worker` binary (the only place
    /// pyo3 is linked), baked into this GUI exe at build time and extracted to
    /// the user data dir at runtime (see [`crate::embed`]). So a release
    /// bundle is still a single `pyman[.exe]` to download, while keeping
    /// `python3.dll` out of the GUI's import table so the GUI starts even on
    /// machines without Python.
    pub fn spawn(id: u64, config: TaskConfig) -> std::io::Result<Self> {
        // The worker links python3.dll as a hard import, so the loader must
        // find python3.dll at worker startup (before the worker's main runs).
        // We locate a real Python install directory NOW, in the GUI process,
        // and inject it into the child's PATH so the loader resolves
        // python3.dll from there. If we can't find one, we don't even spawn
        // the worker — it would exit 127 immediately, which would look like an
        // opaque crash. Instead we synthesize a Failed task with a friendly
        // message in the log.
        let py_dir = crate::worker::find_python_on_path();
        if py_dir.is_none() {
            let log = Arc::new(Mutex::new(LogBuffer::new()));
            log.lock().unwrap().push(
                Stream::Stderr,
                crate::worker::NO_PYTHON_MSG.to_string(),
            );
            return Ok(ScriptTask {
                id,
                config,
                state: TaskState::Failed,
                started_ms: now_ms(),
                ended_ms: Some(now_ms()),
                exit_code: None,
                log,
                child: None,
            });
        }

        // Make sure the worker binary is on disk (extracted from our embedded
        // copy on first run / when stale). This, not find_python_on_path, is
        // the failure that shows up if the data dir isn't writable.
        let worker = match crate::embed::ensure_worker() {
            Ok(p) => p,
            Err(msg) => {
                let log = Arc::new(Mutex::new(LogBuffer::new()));
                log.lock().unwrap().push(Stream::Stderr, msg);
                return Ok(ScriptTask {
                    id,
                    config,
                    state: TaskState::Failed,
                    started_ms: now_ms(),
                    ended_ms: Some(now_ms()),
                    exit_code: None,
                    log,
                    child: None,
                });
            }
        };

        let mut cmd = Command::new(&worker);
        // The worker tolerates a leading --worker flag (it strips it if
        // present), so pass it for clarity / parity with direct invocation.
        cmd.arg("--worker");
        cmd.arg(&config.script);
        cmd.args(&config.args);

        // Prepend the Python directory to the child's PATH so the loader finds
        // python3.dll at startup. We build the PATH from the current env so the
        // worker still sees the rest of the user's tools.
        if let Some(dir) = &py_dir {
            let sep = if cfg!(windows) { ";" } else { ":" };
            let new_path = match std::env::var_os("PATH") {
                Some(existing) => {
                    let mut s = dir.as_os_str().to_owned();
                    s.push(sep);
                    s.push(&existing);
                    s
                }
                None => dir.as_os_str().to_owned(),
            };
            cmd.env("PATH", new_path);
        }

        // Merge stderr into stdout via a single piped handle so ordering is
        // preserved and the UI sees a single coherent stream. We tag each
        // line with its origin below.
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.stdin(Stdio::null());

        // On Windows, a console-subsystem child spawned from a GUI (no-console)
        // parent gets a brand-new console window allocated for it — that's the
        // black terminal that flashes up when a script runs. CREATE_NO_WINDOW
        // (= CREATE_NO_WINDOW flag 0x08000000) tells the loader not to create
        // any console for the child. The piped stdout/stderr above still work:
        // the child writes to the pipes, not a console. This keeps the worker
        // a normal console binary (so running it directly in a terminal still
        // shows output) while suppressing the window when launched by the GUI.
        #[cfg(target_os = "windows")]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let log = Arc::new(Mutex::new(LogBuffer::new()));
                log.lock().unwrap().push(
                    Stream::Stderr,
                    format!("failed to spawn worker {:?}: {e}", worker.display()),
                );
                return Ok(ScriptTask {
                    id,
                    config,
                    state: TaskState::Failed,
                    started_ms: now_ms(),
                    ended_ms: Some(now_ms()),
                    exit_code: None,
                    log,
                    child: None,
                });
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
