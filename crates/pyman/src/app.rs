//! egui front-end for pyman.
//!
//! The app owns a single list of [`Entry`]s. Each entry is a remembered
//! script (persisted to disk by the `history` module) plus an optional running
//! [`ScriptTask`]. An entry with `task = None` is a loaded-but-stopped history
//! item; running it spawns a worker and fills in `task`. This keeps the "what
//! scripts exist" list and the "what's running now" view as one structure.

use crate::history::{self, HistoryEntry};
use crate::supervisor::{ScriptTask, Stream, TaskConfig, TaskState};
use eframe::egui;

/// Install CJK fallback fonts so the Chinese UI text renders (egui's bundled
/// fonts are Latin-only). Delegates to the `font` module; declared here so the
/// app-creation closure in `main.rs` can call `app::install_fonts`.
pub fn install_fonts(ctx: &egui::Context) {
    crate::font::install(ctx);
}

/// One row in the UI: a remembered script and, if currently running, its task.
struct Entry {
    id: u64,
    /// User-facing label (defaults to the script file name).
    name: String,
    autostart: bool,
    config: TaskConfig,
    /// None => not running (loaded history item); Some => live worker task.
    task: Option<ScriptTask>,
}

impl Entry {
    fn is_running(&self) -> bool {
        matches!(self.task.as_ref().map(|t| t.state), Some(TaskState::Running))
    }

    /// Derive the persisted form of this entry. We always persist, even for
    /// finished tasks, so the history survives across launches.
    fn to_history(&self) -> HistoryEntry {
        HistoryEntry {
            name: self.name.clone(),
            script: self.config.script.clone(),
            args: self.config.args.clone(),
            autostart: self.autostart,
        }
    }
}

/// Top-level UI state.
pub struct PymanApp {
    entries: Vec<Entry>,
    next_id: u64,
    /// Form: script path text field.
    script_input: String,
    /// Form: args text field (space-separated, like a shell).
    args_input: String,
    /// Which entry's log is currently selected in the viewer.
    selected: Option<u64>,
    /// Most recent user-facing message (e.g. "added", "invalid path").
    flash: Option<String>,
    /// Logo texture, lazily uploaded on first frame (needs a Context).
    logo: Option<egui::TextureHandle>,
}

impl Default for PymanApp {
    fn default() -> Self {
        // Load saved history. autostart entries are spawned; the rest are
        // loaded as stopped entries the user can re-run.
        let mut next_id: u64 = 1;
        let mut entries: Vec<Entry> = Vec::new();
        for h in history::load() {
            let id = next_id;
            next_id += 1;
            let task = if h.autostart {
                // Autostart: spawn immediately. If the spawn fails we still
                // keep the entry (stopped) so it isn't silently dropped.
                match ScriptTask::spawn(id, TaskConfig {
                    script: h.script.clone(),
                    args: h.args.clone(),
                }) {
                    Ok(t) => Some(t),
                    Err(e) => {
                        eprintln!("[pyman] autostart failed for {}: {e}", h.script.display());
                        None
                    }
                }
            } else {
                None
            };
            entries.push(Entry {
                id,
                name: h.name,
                autostart: h.autostart,
                config: TaskConfig {
                    script: h.script,
                    args: h.args,
                },
                task,
            });
        }

        Self {
            entries,
            next_id,
            script_input: String::new(),
            args_input: String::new(),
            // Select the first entry, if any, so the log viewer isn't empty.
            selected: None,
            flash: None,
            logo: None,
        }
    }
}

impl PymanApp {
    /// Parse the args box. Tokens are whitespace-separated; a token may be
    /// wrapped in double quotes to keep its internal spaces (so `-c "print(1)"`
    /// yields two args, not three). No escaping — a bare backslash or quote is
    /// literal. Empty input yields no args. This is just enough shell flavor for
    /// the CLI mode (`python <args>`), which needs grouped arguments like
    /// `-c "..."` to work; plain space-separated values behave exactly as
    /// before.
    fn parse_args(s: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut in_quotes = false;
        for ch in s.chars() {
            match ch {
                '"' => in_quotes = !in_quotes,
                c if c.is_whitespace() => {
                    if in_quotes {
                        cur.push(c);
                    } else if !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                    }
                }
                c => cur.push(c),
            }
        }
        if !cur.is_empty() {
            out.push(cur);
        }
        out
    }

    /// Persist the current entries to disk. Centralized so every mutation
    /// site just calls this once after changing state.
    fn persist(&self) {
        history::save(
            &self
                .entries
                .iter()
                .map(Entry::to_history)
                .collect::<Vec<_>>(),
        );
    }

    fn add_entry(&mut self) {
        let path = self.script_input.trim().to_string();
        let args = Self::parse_args(&self.args_input);

        // Two add modes:
        //   * Script mode (default): a real script path is given.
        //   * CLI mode: the path is left empty and `args` holds Python's own
        //     command line (e.g. `-m http.server`). We then run `python <args>`
        //     instead of a script file (see `supervisor::ScriptTask::spawn`).
        let config = if path.is_empty() {
            if args.is_empty() {
                self.flash = Some(
                    "脚本路径为空时，请在『参数』里填写要传给 python 的参数（例如 -m http.server）。".into(),
                );
                return;
            }
            TaskConfig {
                // Empty PathBuf is the CLI-mode marker (see TaskConfig::is_cli_mode).
                script: std::path::PathBuf::new(),
                args,
            }
        } else {
            let p = std::path::PathBuf::from(&path);
            if !p.exists() {
                self.flash = Some(format!("脚本文件不存在: {path}"));
                return;
            }
            TaskConfig { script: p, args }
        };
        // The "下次启动自动运行" form option was removed — newly added scripts
        // default to not autostarting. Users can still toggle autostart per
        // entry in the list ("自启✓" chip).
        let autostart = false;
        // No name field in the UI — derive the label from the script's file
        // name so the list reads e.g. "hello.py" instead of a full path.
        let name =
            HistoryEntry::from_input(None, config.clone(), autostart).name;

        let id = self.next_id;
        self.next_id += 1;
        // A freshly added script runs immediately regardless of autostart:
        // autostart only governs *next launch* behavior.
        match ScriptTask::spawn(id, config.clone()) {
            Ok(task) => {
                // Select the entry so its log is shown in the viewer.
                self.selected = Some(id);
                self.entries.push(Entry {
                    id,
                    name,
                    autostart,
                    config,
                    task: Some(task),
                });
            }
            Err(e) => self.flash = Some(format!("启动失败: {e}")),
        }
        self.persist();
    }

    /// Start (or restart) a stopped entry's worker.
    fn run_entry(&mut self, id: u64) {
        // Move the config out temporarily to avoid borrowing self during spawn.
        let config = match self.entries.iter().find(|e| e.id == id) {
            Some(e) => e.config.clone(),
            None => return,
        };
        match ScriptTask::spawn(id, config) {
            Ok(task) => {
                // Select the entry so its log is shown in the viewer — without
                // this the viewer keeps showing whatever was previously
                // selected, so the just-started script's output is hidden.
                self.selected = Some(id);
                if let Some(e) = self.entries.iter_mut().find(|e| e.id == id) {
                    e.task = Some(task);
                }
            }
            Err(e) => self.flash = Some(format!("启动失败: {e}")),
        }
        self.persist();
    }

    fn stop_entry(&mut self, id: u64) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.id == id) {
            if let Some(t) = e.task.as_mut() {
                t.stop();
            }
        }
        // Stopping doesn't change persistence (entry stays, autostart intact).
    }

    fn remove_entry(&mut self, id: u64) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.id == id) {
            if e.is_running() {
                if let Some(t) = e.task.as_mut() {
                    t.stop();
                }
                self.flash = Some("请先停止后再移除".into());
                return;
            }
        }
        self.entries.retain(|e| e.id != id);
        if self.selected == Some(id) {
            self.selected = self.entries.first().map(|e| e.id);
        }
        self.persist();
    }

    /// Toggle autostart on an entry and persist.
    fn toggle_autostart(&mut self, id: u64) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.id == id) {
            e.autostart = !e.autostart;
        }
        self.persist();
    }

    /// Send a line of user input to the running task's stdin. Empty input is
    /// dropped (no-op), so pressing Enter on an empty box just clears the field
    /// without sending a stray newline. Write errors (stdin already closed)
    /// surface as a flash so the user knows their input didn't go through.
    fn send_stdin(&mut self, id: u64, text: String) {
        if text.is_empty() {
            return;
        }
        if let Some(e) = self.entries.iter_mut().find(|e| e.id == id) {
            if let Some(t) = e.task.as_mut() {
                if t.write_stdin(&text).is_err() {
                    self.flash =
                        Some(format!("#{id} 的输入发送失败：脚本可能已退出或已关闭 stdin"));
                }
            }
        }
    }
}

impl eframe::App for PymanApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Poll every running child and re-request repaint while anything is
        // running, so logs stream and states update without manual refresh.
        if self.poll_tasks() {
            ctx.request_repaint();
        }

        // Actions collected from UI buttons, applied after panels are drawn so
        // we don't mutate `self.entries` while iterating it.
        let mut action: Option<TaskAction> = None;

        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                // Logo: lazily upload the texture on the first frame, then
                // cache the handle on self for reuse.
                let logo = self
                    .logo
                    .get_or_insert_with(|| crate::icon::logo_texture(ctx));
                ui.add(
                    egui::Image::from_texture(egui::load::SizedTexture {
                        id: logo.id(),
                        size: egui::vec2(28.0, 28.0),
                    }),
                );
                ui.heading("PyMan — Python 脚本管理器");
                ui.separator();
                ui.label(format!("脚本: {}  运行中: {}", self.entries.len(), self.running_count()));
            });
        });

        // Flash message bar.
        if let Some(msg) = self.flash.clone() {
            egui::TopBottomPanel::top("flash").show(ctx, |ui| {
                ui.colored_label(egui::Color32::LIGHT_BLUE, &msg);
            });
        }

        egui::SidePanel::left("tasks").resizable(true).show(ctx, |ui| {
            ui.heading("脚本列表");
            ui.separator();
            if self.entries.is_empty() {
                ui.label("(暂无，在右侧添加脚本)");
            }
            for e in &self.entries {
                ui.horizontal(|ui| {
                    let is_sel = self.selected == Some(e.id);
                    // State is shown as a colored chip below (green=running,
                    // red=stopped), so the row label just carries id + name.
                    let label = format!("#{} {}", e.id, e.name);
                    if ui.selectable_label(is_sel, &label).clicked() {
                        self.selected = Some(e.id);
                    }
                    // Autostart toggle button. When ON we give it a solid
                    // green fill AND explicitly set the text to white so the
                    // label stays readable on both light and dark themes —
                    // egui otherwise keeps the theme's default text color,
                    // which is near-black and vanishes on the green fill on
                    // light themes.
                    let on = e.autostart;
                    let auto_label = if on { "自启✓" } else { "自启" };
                    let btn = if on {
                        egui::Button::new(
                            egui::RichText::new(auto_label)
                                .color(egui::Color32::WHITE)
                                .small(),
                        )
                        .fill(egui::Color32::from_rgb(46, 125, 50)) // green-800
                        .stroke(egui::Stroke::NONE)
                    } else {
                        egui::Button::new(auto_label).small()
                    };
                    let auto_btn = ui.add(btn);
                    let clicked = auto_btn.clicked();
                    auto_btn.on_hover_text("点击切换：下次启动 PyMan 时是否自动运行该脚本");
                    if clicked {
                        action = Some(TaskAction::ToggleAutostart(e.id));
                    }
                    // Status chip: a colored, clickable button whose fill
                    // signals the task state (green=running, red=not running)
                    // and whose click toggles it. We keep the short state word
                    // as the label so the chip conveys more than two states
                    // (完成/失败/已停止/未运行 all map to red but read
                    // distinctly). The "⏹"/"▶" glyphs are CJK-covered so they
                    // render fine; text is forced white for contrast on both
                    // the green and red fills (see the autostart button above
                    // for why explicit white matters on light themes).
                    let (chip_fill, chip_label, hover, chip_action) =
                        if e.is_running() {
                            (
                                egui::Color32::from_rgb(46, 125, 50), // green-800
                                "⏹ 运行中",
                                "运行中 — 点击停止",
                                TaskAction::Stop(e.id),
                            )
                        } else {
                            (
                                egui::Color32::from_rgb(198, 40, 40), // red-800
                                match entry_state_badge(e) {
                                    "完成" => "▶ 已完成",
                                    "失败" => "▶ 已失败",
                                    "已停止" => "▶ 已停止",
                                    _ => "▶ 未运行",
                                },
                                "未运行 — 点击运行",
                                TaskAction::Run(e.id),
                            )
                        };
                    let chip = ui.add(
                        egui::Button::new(
                            egui::RichText::new(chip_label)
                                .color(egui::Color32::WHITE)
                                .small(),
                        )
                        .fill(chip_fill)
                        .stroke(egui::Stroke::NONE),
                    );
                    // `on_hover_text` consumes the Response, so capture the
                    // click first.
                    let clicked = chip.clicked();
                    chip.on_hover_text(hover);
                    if clicked {
                        action = Some(chip_action);
                    }
                    // `&&` short-circuits, so the button only renders (and is
                    // hittable) when the task isn't running — matching the old
                    // nested-if behavior while satisfying clippy::collapsible_if.
                    if !e.is_running() && ui.small_button("✕ 移除").clicked() {
                        action = Some(TaskAction::Remove(e.id));
                    }
                });
            }
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            // Add-script form at the top.
            ui.group(|ui| {
                ui.set_width(ui.available_width());
                ui.label("添加脚本");
                ui.horizontal(|ui| {
                    ui.label("脚本路径:");
                    ui.text_edit_singleline(&mut self.script_input)
                        .on_hover_text("Python 脚本的完整路径，例如 C:/scripts/foo.py；留空则把『参数』作为 python 的命令行参数运行（例如 -m http.server）");
                    // Native OS file-open dialog. Synchronous (rfd::FileDialog
                    // blocks this thread until the user picks or cancels); that's
                    // fine for a desktop tool — the brief UI freeze during the
                    // modal is the expected behavior of a native dialog, and it
                    // keeps the wiring trivial (no async runtime to thread
                    // through egui's immediate-mode update loop). On cancel the
                    // field is left untouched.
                    if ui.button("浏览…").clicked() {
                        let mut dlg = rfd::FileDialog::new()
                            .set_title("选择 Python 脚本")
                            .add_filter(
                                "Python 脚本 (*.py)",
                                &["py", "pyw"],
                            )
                            // Always-on fallback so non-.py scripts (or any
                            // file the user insists on) remain selectable.
                            .add_filter("所有文件", &["*"]);
                        // Start in the directory of the current value if it
                        // points somewhere real, else let the OS pick (recent/
                        // documents) — nicer than always landing in C:\.
                        if let Some(parent) = std::path::Path::new(&self.script_input)
                            .parent()
                            .filter(|p| !p.as_os_str().is_empty() && p.is_dir())
                        {
                            dlg = dlg.set_directory(parent);
                        }
                        if let Some(picked) = dlg.pick_file() {
                            self.script_input = picked.display().to_string();
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("参数:");
                    ui.text_edit_singleline(&mut self.args_input)
                        .on_hover_text("空格分隔。脚本模式：传给脚本的 sys.argv；脚本路径留空时：作为 python 的命令行参数（支持 -m 模块名、-c \"代码\" 等，双引号可分组）");
                });
                ui.horizontal(|ui| {
                    if ui.button("▶ 添加并启动").clicked() {
                        self.flash = None;
                        self.add_entry();
                    }
                    if ui.button("清空表单").clicked() {
                        self.script_input.clear();
                        self.args_input.clear();
                    }
                });
            });

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);

            // Log viewer for the selected entry.
            ui.heading("日志");
            if let Some(id) = self.selected {
                match self.entries.iter().find(|e| e.id == id) {
                    Some(e) if e.task.is_some() => draw_log(ui, e.task.as_ref().unwrap(), &mut action),
                    Some(e) => {
                        ui.label(format!("#{}  {}", e.id, e.name));
                        // CLI mode (empty path) folds its args into describe()
                        // as `python <args>`, so a single "运行: ..." line is
                        // enough; script mode shows the path then the args row.
                        ui.label(format!("运行: {}", e.config.describe()));
                        if !e.config.is_cli_mode() && !e.config.args.is_empty() {
                            ui.label(format!("参数: {}", e.config.args.join(" ")));
                        }
                        ui.separator();
                        ui.label("(未运行，点击左侧 ▶ 运行 开始)");
                    }
                    None => {
                        ui.label("(该脚本已被移除)");
                    }
                }
            } else {
                ui.label("(从左侧选择一个脚本查看日志)");
            }
        });

        if let Some(a) = action {
            match a {
                TaskAction::Run(id) => self.run_entry(id),
                TaskAction::Stop(id) => self.stop_entry(id),
                TaskAction::Remove(id) => self.remove_entry(id),
                TaskAction::ToggleAutostart(id) => self.toggle_autostart(id),
                TaskAction::SendStdin(id, text) => self.send_stdin(id, text),
            }
        }
    }
}

impl PymanApp {
    fn running_count(&self) -> usize {
        self.entries.iter().filter(|e| e.is_running()).count()
    }

    /// Poll every running task; return true if any are still running (so the
    /// UI should keep repainting). Finished tasks are *kept* (their entry and
    /// log remain visible) — only the worker child is done.
    fn poll_tasks(&mut self) -> bool {
        let mut any_running = false;
        for e in &mut self.entries {
            if let Some(t) = e.task.as_mut() {
                t.poll();
                if t.state == TaskState::Running {
                    any_running = true;
                }
            }
        }
        any_running
    }
}

/// Render the log panel for a single task. Free-standing so it can borrow a
/// `&ScriptTask` without aliasing the rest of the app state.
///
/// Each row shows three columns: a local wall-clock timestamp (`HH:MM:SS.mmm`,
/// captured when the line was read), the origin (`out`/`err`), and the line
/// text in pure black (stdout) / dark-red (stderr) for readability on both
/// light and dark themes. The text cell is a selectable label, so the user can
/// drag-select lines and copy them with Ctrl+C.
fn draw_log(ui: &mut egui::Ui, task: &ScriptTask, action: &mut Option<TaskAction>) {
    let header = format!(
        "#{}  {}  {}",
        task.id,
        task.config.describe(),
        match task.state {
            TaskState::Running => "运行中…".to_string(),
            TaskState::Finished => format!("完成 (exit={})", task.exit_code.unwrap_or(0)),
            TaskState::Failed => format!("失败 (exit={})", task.exit_code.unwrap_or(-1)),
            TaskState::Stopped => "已停止".to_string(),
        }
    );
    ui.label(egui::RichText::new(&header).strong());
    // CLI mode's describe() already shows `python <args>`, so only print the
    // args row in script mode (where it's separate from the path header).
    if !task.config.is_cli_mode() && !task.config.args.is_empty() {
        ui.label(format!("参数: {}", task.config.args.join(" ")));
    }

    // Snapshot the log under lock, render outside the lock. We carry ts_ms too
    // so each rendered row can show the wall-clock time the line arrived.
    let (lines, total) = {
        let buf = task.log.lock().unwrap();
        let total = buf.lines.len();
        let snapshot: Vec<(u128, Stream, String)> = buf
            .lines
            .iter()
            .map(|l| (l.ts_ms, l.stream, l.text.clone()))
            .collect();
        (snapshot, total)
    };

    // Toolbar: copy-all button. egui's per-label selection only works within a
    // single line, which is useless for multi-line logs — so we offer a
    // one-click "copy everything" that writes the whole buffer (with
    // timestamps) to the system clipboard via ctx.copy_text. The "已复制 ✓"
    // feedback is transient UI state keyed per task.
    let copied_key = egui::Id::new(("log_copied_at", task.id));
    let cleared_key = egui::Id::new(("log_cleared_at", task.id));
    ui.horizontal(|ui| {
        let tooltip = "把当前脚本的全部日志（含时间戳）复制到剪贴板";
        if ui.button("📋 复制全部").on_hover_text(tooltip).clicked() {
            let blob = lines
                .iter()
                .map(|(ts, _stream, text)| format!("{} {}", format_ts(*ts), text))
                .collect::<Vec<_>>()
                .join("\n");
            ui.ctx().copy_text(blob);
            ui.data_mut(|d| d.insert_temp(copied_key, std::time::Instant::now()));
        }
        // Show the confirmation for ~2s after the click. Instant::now() as the
        // fallback keeps the closure total (no panic on missing key).
        let just_copied = ui
            .data(|d| {
                d.get_temp::<std::time::Instant>(copied_key)
                    .map(|t| t.elapsed().as_secs() < 2)
            })
            .unwrap_or(false);
        if just_copied {
            // Plain text — no checkmark glyph: U+2713 isn't covered by egui's
            // bundled Latin font nor the CJK fallback, so it renders as tofu.
            // The green color already signals success.
            ui.colored_label(egui::Color32::from_rgb(46, 125, 50), "已复制");
        }
        // Clear the in-memory log buffer for this task. New output keeps
        // accumulating (the reader threads are untouched), so this only wipes
        // the displayed history. Mirror the copy button's transient ✓ feedback.
        if ui
            .button("🗑 清空日志")
            .on_hover_text("清空当前脚本的日志显示（脚本仍会继续输出新内容）")
            .clicked()
        {
            task.clear_log();
            ui.data_mut(|d| d.insert_temp(cleared_key, std::time::Instant::now()));
        }
        let just_cleared = ui
            .data(|d| {
                d.get_temp::<std::time::Instant>(cleared_key)
                    .map(|t| t.elapsed().as_secs() < 2)
            })
            .unwrap_or(false);
        if just_cleared {
            ui.colored_label(egui::Color32::from_rgb(46, 125, 50), "已清空");
        }
        ui.label(format!("{total} 行"));
    });
    ui.separator();

    // Cap the log scroll area's height so it does NOT greedily consume the
    // entire central panel and push the stdin input box below the visible
    // region. We reserve room for the input row drawn afterwards: a separator
    // + a ~18px text field + the 发送/关闭 stdin buttons ≈ 40px, plus a small
    // margin. Without this cap the input box would be forever scrolled out of
    // view (the original bug: ScrollArea defaults to max_height = infinity).
    let input_row_height = 44.0;
    let log_max_height = (ui.available_height() - input_row_height).max(80.0);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .max_height(log_max_height)
        .stick_to_bottom(true)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if lines.is_empty() {
                ui.label("(还没有输出)");
                return;
            }
            // Two columns: 时间 | 输出内容. The output column uses
            // Label::selectable(true) so the user can drag-select and Ctrl+C
            // copy log text (egui's default `ui.label` selection is often
            // visually subtle / theme-dependent; making it explicit also keeps
            // it working if the global style turns selectable labels off). The
            // stream origin (stdout/stderr) is conveyed only by color now —
            // black for stdout, dark-red for stderr.
            egui::Grid::new(format!("log_{}", task.id))
                .striped(true)
                .num_columns(2)
                .show(ui, |ui| {
                    for (ts_ms, stream, text) in &lines {
                        // Timestamp: local wall-clock HH:MM:SS.mmm. Compact and
                        // dim so it stays a secondary annotation.
                        ui.label(
                            egui::RichText::new(format_ts(*ts_ms))
                                .small()
                                .color(egui::Color32::DARK_GRAY)
                                .family(egui::FontFamily::Monospace),
                        );
                        // Body text: pure black for readability (the old
                        // near-white stdout color vanished on light themes).
                        // stderr keeps a distinct dark-red so the two streams
                        // are still tellable apart at a glance; user input is a
                        // muted blue so the conversation's "I typed this" lines
                        // read as echoes, not script output.
                        let body_color = match stream {
                            Stream::Stdout => egui::Color32::BLACK,
                            Stream::Stderr => egui::Color32::from_rgb(170, 0, 0),
                            Stream::Input => egui::Color32::from_rgb(0, 80, 160),
                        };
                        // Prefix echoed user input with a chevron so it reads
                        // as "you typed" rather than blending into the script's
                        // own stdout. Pure stdout/stderr render as-is.
                        let display = if *stream == Stream::Input {
                            format!("» {text}")
                        } else {
                            text.clone()
                        };
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(display)
                                    .color(body_color)
                                    .family(egui::FontFamily::Monospace),
                            )
                            .selectable(true)
                            // Let long lines wrap inside the cell instead of
                            // forcing the grid wider than the panel.
                            .wrap(),
                        );
                        ui.end_row();
                    }
                });
        });

    // Input box: send lines to the script's stdin. Only meaningful while the
    // task is running and we still hold an open stdin pipe; once the child is
    // gone or stdin closed, the box is disabled with an explanatory hint.
    draw_stdin_input(ui, task, action);
}

/// Render the per-task stdin input row. Each task keeps its own [`InputState`]
/// in egui's memory (keyed by task id) so switching between tasks doesn't wipe
/// half-typed input or its history. Enter (or the 发送 button) sends the line;
/// Up/Down arrows walk the input history, shell-style.
fn draw_stdin_input(ui: &mut egui::Ui, task: &ScriptTask, action: &mut Option<TaskAction>) {
    ui.separator();
    let open = task.stdin_open();
    let hint = if open {
        "在此输入并按回车发送给脚本的 stdin（↑/↓ 选择历史输入）"
    } else {
        match task.state {
            TaskState::Running => "stdin 已关闭，无法再发送输入",
            _ => "脚本未运行，无法发送输入",
        }
    };

    // Input state (draft + history + navigation cursor) is per-task so each
    // task has its own input line and history that survives switching
    // selection.
    let state_key = egui::Id::new(("stdin_input_state", task.id));
    let mut input: InputState = ui
        .data(|d| d.get_temp::<InputState>(state_key))
        .unwrap_or_default();

    ui.horizontal(|ui| {
        ui.label("输入:");
        // Reserve room for the 发送 button + spacing (~80px) so the text field
        // takes the rest without pushing it off the row. egui's immediate-mode
        // layout has no look-ahead, so we subtract a constant; the floor keeps
        // it sane on very narrow UIs.
        let field_width = (ui.available_width() - 80.0).max(120.0);
        let resp = ui.add(
            egui::TextEdit::singleline(&mut input.draft)
                .desired_width(field_width)
                .hint_text(hint)
                .interactive(open),
        );

        // Arrow-key history navigation, shell/readline style. Only consume the
        // keys while the field is focused so Up/Down keep working elsewhere
        // (e.g. inside the log selection). The `InputState` methods below are
        // pure and unit-tested; this branch only feeds them the key events.
        if open && resp.has_focus() {
            let ctx = resp.ctx.clone();
            let up = ctx.input(|i| i.key_pressed(egui::Key::ArrowUp));
            let down = ctx.input(|i| i.key_pressed(egui::Key::ArrowDown));
            if up {
                input.prev_history();
            }
            if down {
                input.next_history();
            }
        }

        // Enter (when focused) sends the line; the 发送 button is the mouse
        // equivalent. `&&` short-circuits so the button is only drawn when the
        // input is actually open — when closed we render the disabled field
        // above with its hint and no button.
        let enter_pressed = resp.lost_focus()
            && resp.ctx.input(|i| i.key_pressed(egui::Key::Enter));
        if open
            && (enter_pressed || ui.button("发送").clicked())
            && !input.draft.trim().is_empty()
        {
            let text = std::mem::take(&mut input.draft);
            // Commit to history BEFORE dispatching the action, so the line is
            // recorded even if the send later fails (the user still typed it).
            input.push_history(text.clone());
            *action = Some(TaskAction::SendStdin(task.id, text));
            // Reclaim focus so the user can keep typing the next line.
            resp.request_focus();
        }
    });

    // Persist the (possibly edited / navigated) input state back to per-task
    // memory.
    ui.data_mut(|d| d.insert_temp(state_key, input));
}

/// Per-task input state: the current draft plus a shell-style history of
/// previously sent lines and a cursor into it.
///
/// Navigation mirrors a terminal:
///   * `prev_history` (↑) walks toward older entries; the first press saves the
///     current draft so ↓ all the way back (or past the newest entry) restores
///     it rather than clobbering it with the latest history line.
///   * `next_history` (↓) walks toward newer entries; pressing ↓ past the
///     newest entry returns to the saved draft.
///   * Editing a recalled line then pressing ↑/↓ again discards the edits
///     (standard shell behavior — we don't try to merge edits back).
///
/// The navigation logic is split into pure methods so it can be unit-tested
/// without an egui context.
#[derive(Clone, Default)]
struct InputState {
    /// Current text in the input box.
    draft: String,
    /// Previously sent lines, oldest first. Bounded to avoid unbounded growth
    /// from long-running interactive sessions.
    history: Vec<String>,
    /// Position in the history navigation, or `None` when not navigating
    /// (i.e. the user is typing a fresh line). An index of `i` means the draft
    /// is currently showing `history[i]`.
    history_pos: Option<usize>,
    /// The draft as it was before the user first pressed ↑, restored when they
    /// navigate back below the newest entry.
    saved_draft: String,
}

impl InputState {
    /// Cap on how many sent lines we keep. Match the log buffer's order of
    /// magnitude (10k lines) — plenty for an interactive session, bounded so a
    /// runaway script can't grow memory forever.
    const HISTORY_CAP: usize = 10_000;

    /// Record a sent line. Resets navigation so the next ↑ starts from the
    /// newest entry. Dedupes a repeat of the immediately previous line (common
    /// when re-sending the same command) but keeps other duplicates in order,
    /// matching typical shell history.
    fn push_history(&mut self, line: String) {
        if self.history.last().map(String::as_str) != Some(line.as_str()) {
            self.history.push(line);
            if self.history.len() > Self::HISTORY_CAP {
                // Drop the oldest to stay bounded; navigation indices stay
                // valid because we always reset history_pos below.
                self.history.remove(0);
            }
        }
        self.history_pos = None;
        self.saved_draft.clear();
    }

    /// ↑: move to the previous (older) history entry. On the first press, save
    /// the current draft so ↓ past the newest entry restores it. No-op when the
    /// history is empty or we're already at the oldest entry.
    fn prev_history(&mut self) {
        if self.history.is_empty() {
            return;
        }
        match self.history_pos {
            // First press: save draft, jump to newest.
            None => {
                self.saved_draft = self.draft.clone();
                let pos = self.history.len() - 1;
                self.draft = self.history[pos].clone();
                self.history_pos = Some(pos);
            }
            // Already navigating: move older if possible. Stays put at the
            // oldest entry (so repeated ↑ doesn't wrap unexpectedly).
            Some(0) => {}
            Some(pos) => {
                let pos = pos - 1;
                self.draft = self.history[pos].clone();
                self.history_pos = Some(pos);
            }
        }
    }

    /// ↓: move to the next (newer) history entry. Pressing ↓ past the newest
    /// entry returns to the saved draft and ends navigation. No-op when not
    /// navigating.
    fn next_history(&mut self) {
        let Some(pos) = self.history_pos else {
            return;
        };
        if pos + 1 >= self.history.len() {
            // Past the newest entry: restore the pre-navigation draft.
            self.draft = self.saved_draft.clone();
            self.history_pos = None;
            self.saved_draft.clear();
        } else {
            let pos = pos + 1;
            self.draft = self.history[pos].clone();
            self.history_pos = Some(pos);
        }
    }
}

/// Format a unix-epoch-millis timestamp as local `HH:MM:SS.mmm`.
///
/// The log buffer stores UTC milliseconds (see `supervisor::now_ms`); for a
/// script-manager UI, local time is what a user expects to read, so we convert
/// via chrono's local zone. Falls back to a plain millis counter if the system
/// clock / zone is unavailable so rendering never panics.
fn format_ts(ts_ms: u128) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_millis_opt(ts_ms as i64).single() {
        Some(t) => t.format("%H:%M:%S%.3f").to_string(),
        None => format!("+{ts_ms}ms"),
    }
}

/// Short state word for an entry, accounting for stopped (no task) entries.
/// Used to label the status chip; the brackets/brackets-style "[运行中]" form
/// is gone now that the chip itself carries the visual weight.
fn entry_state_badge(e: &Entry) -> &'static str {
    match e.task.as_ref().map(|t| t.state) {
        Some(TaskState::Running) => "运行中",
        Some(TaskState::Finished) => "完成",
        Some(TaskState::Failed) => "失败",
        Some(TaskState::Stopped) => "已停止",
        None => "未运行",
    }
}

enum TaskAction {
    Run(u64),
    Stop(u64),
    Remove(u64),
    ToggleAutostart(u64),
    /// Send the contents of the per-task input box to the task's stdin.
    /// Carries (id, text). Empty input is dropped by the action site.
    SendStdin(u64, String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_plain_whitespace() {
        // Backwards compat: bare space-separated tokens, empty fields dropped.
        assert_eq!(
            PymanApp::parse_args("  -m  http.server  "),
            vec!["-m".to_string(), "http.server".to_string()]
        );
        assert!(PymanApp::parse_args("   ").is_empty());
        assert!(PymanApp::parse_args("").is_empty());
    }

    #[test]
    fn parse_args_quotes_group_internal_spaces() {
        // CLI mode needs grouped args like `-c "print(1 + 2)"` to stay one arg.
        assert_eq!(
            PymanApp::parse_args(r#"-c "print(1 + 2)""#),
            vec!["-c".to_string(), "print(1 + 2)".to_string()]
        );
        // A quote in the middle doesn't start a group; only a `"` toggles.
        assert_eq!(
            PymanApp::parse_args(r#"say "a b" tail"#),
            vec!["say".to_string(), "a b".to_string(), "tail".to_string()]
        );
    }

    #[test]
    fn parse_args_unclosed_quote_keeps_rest() {
        // A stray unclosed quote just gathers the remainder — lenient, no panic.
        assert_eq!(
            PymanApp::parse_args(r#"-c "print(1)""#),
            vec!["-c".to_string(), "print(1)".to_string()]
        );
    }

    #[test]
    fn input_state_history_dedupes_consecutive_repeats() {
        let mut s = InputState::default();
        s.push_history("foo".into());
        s.push_history("foo".into()); // repeat of last → dropped
        s.push_history("bar".into());
        assert_eq!(s.history, vec!["foo".to_string(), "bar".to_string()]);
        // A non-consecutive repeat (foo after bar) is still kept in order.
        s.push_history("foo".into());
        assert_eq!(
            s.history,
            vec!["foo".to_string(), "bar".to_string(), "foo".to_string()]
        );
    }

    #[test]
    fn input_state_arrow_up_then_down_round_trips() {
        let mut s = InputState::default();
        s.push_history("a".into());
        s.push_history("b".into());
        s.push_history("c".into());
        assert_eq!(s.history.len(), 3);

        // Start from a fresh draft, press ↑: draft becomes newest ("c").
        s.draft = "typed".into();
        s.prev_history();
        assert_eq!(s.draft, "c");
        // ↑ again → "b", again → "a", then stays at oldest.
        s.prev_history();
        assert_eq!(s.draft, "b");
        s.prev_history();
        assert_eq!(s.draft, "a");
        s.prev_history(); // already oldest, no wrap
        assert_eq!(s.draft, "a");

        // ↓ walks back toward newest, then restores the pre-navigation draft.
        s.next_history();
        assert_eq!(s.draft, "b");
        s.next_history();
        assert_eq!(s.draft, "c");
        s.next_history(); // past newest → saved draft restored
        assert_eq!(s.draft, "typed");
        assert!(s.history_pos.is_none());
        // ↓ again now that we're back to typing is a no-op.
        s.next_history();
        assert_eq!(s.draft, "typed");
    }

    #[test]
    fn input_state_first_up_saves_then_down_restores() {
        // The pre-navigation draft must come back when ↓ past the newest entry,
        // even with a one-element history.
        let mut s = InputState::default();
        s.push_history("only".into());
        s.draft = "half-typed".into();

        s.prev_history();
        assert_eq!(s.draft, "only");
        s.next_history(); // past the single entry → restore
        assert_eq!(s.draft, "half-typed");
    }

    #[test]
    fn input_state_empty_history_up_is_noop() {
        let mut s = InputState::default();
        s.draft = "x".into();
        s.prev_history(); // nothing to navigate to
        assert_eq!(s.draft, "x");
        assert!(s.history_pos.is_none());
    }

    #[test]
    fn input_state_push_resets_navigation() {
        // After sending a new line mid-navigation, ↑ should start from the new
        // newest entry, not from wherever the cursor was.
        let mut s = InputState::default();
        s.push_history("a".into());
        s.push_history("b".into());
        s.prev_history(); // now showing "b", cursor at index 1

        s.push_history("c".into());
        assert!(s.history_pos.is_none());
        s.prev_history(); // first ↑ after send → newest = "c"
        assert_eq!(s.draft, "c");
    }
}
