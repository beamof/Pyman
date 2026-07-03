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
    /// Parse the args box. Quotes are not supported; keep it simple. Empty
    /// fields are dropped.
    fn parse_args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
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
        if path.is_empty() {
            self.flash = Some("请先填写脚本路径".into());
            return;
        }
        let p = std::path::PathBuf::from(&path);
        if !p.exists() {
            self.flash = Some(format!("脚本文件不存在: {path}"));
            return;
        }
        let config = TaskConfig {
            script: p,
            args: Self::parse_args(&self.args_input),
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
                        .on_hover_text("Python 脚本的完整路径，例如 C:/scripts/foo.py");
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
                        .on_hover_text("空格分隔的参数，会传给脚本的 sys.argv");
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
                    Some(e) if e.task.is_some() => draw_log(ui, e.task.as_ref().unwrap()),
                    Some(e) => {
                        ui.label(format!("#{}  {}", e.id, e.name));
                        ui.label(format!("路径: {}", e.config.script.display()));
                        if !e.config.args.is_empty() {
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
fn draw_log(ui: &mut egui::Ui, task: &ScriptTask) {
    let header = format!(
        "#{}  {}  {}",
        task.id,
        task.config.script.display(),
        match task.state {
            TaskState::Running => "运行中…".to_string(),
            TaskState::Finished => format!("完成 (exit={})", task.exit_code.unwrap_or(0)),
            TaskState::Failed => format!("失败 (exit={})", task.exit_code.unwrap_or(-1)),
            TaskState::Stopped => "已停止".to_string(),
        }
    );
    ui.label(egui::RichText::new(&header).strong());
    if !task.config.args.is_empty() {
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

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
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
                        // are still tellable apart at a glance.
                        let body_color = match stream {
                            Stream::Stdout => egui::Color32::BLACK,
                            Stream::Stderr => egui::Color32::from_rgb(170, 0, 0),
                        };
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(text)
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
}
