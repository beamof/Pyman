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
    /// Form: autostart checkbox.
    autostart_input: bool,
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
            autostart_input: false,
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
        let autostart = self.autostart_input;
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
                self.flash = Some(format!("已启动 #{}: {}", id, config.script.display()));
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
        let (name, config) = match self.entries.iter().find(|e| e.id == id) {
            Some(e) => (e.name.clone(), e.config.clone()),
            None => return,
        };
        match ScriptTask::spawn(id, config) {
            Ok(task) => {
                self.flash = Some(format!("已启动: {name}"));
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
                e.task.as_mut().map(|t| t.stop());
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
                    let badge = entry_state_badge(e);
                    let label = format!("#{} {} {}", e.id, e.name, badge);
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
                    if e.is_running() {
                        if ui.small_button("⏹ 停止").clicked() {
                            action = Some(TaskAction::Stop(e.id));
                        }
                    } else {
                        if ui.small_button("▶ 运行").clicked() {
                            action = Some(TaskAction::Run(e.id));
                        }
                        if ui.small_button("✕ 移除").clicked() {
                            action = Some(TaskAction::Remove(e.id));
                        }
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
                    ui.checkbox(&mut self.autostart_input, "下次启动自动运行")
                        .on_hover_text("勾选后，这条记录会在下次启动 PyMan 时自动运行");
                    if ui.button("清空表单").clicked() {
                        self.script_input.clear();
                        self.args_input.clear();
                        self.autostart_input = false;
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
    ui.separator();

    // Snapshot the log under lock, render outside the lock.
    let (lines, total) = {
        let buf = task.log.lock().unwrap();
        let total = buf.lines.len();
        let snapshot: Vec<(Stream, String)> =
            buf.lines.iter().map(|l| (l.stream, l.text.clone())).collect();
        (snapshot, total)
    };

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if lines.is_empty() {
                ui.label("(还没有输出)");
                return;
            }
            egui::Grid::new(format!("log_{}", task.id))
                .striped(true)
                .num_columns(2)
                .show(ui, |ui| {
                    for (stream, text) in &lines {
                        let color = match stream {
                            Stream::Stdout => egui::Color32::from_gray(220),
                            Stream::Stderr => egui::Color32::from_rgb(255, 170, 170),
                        };
                        ui.label(
                            egui::RichText::new(match stream {
                                Stream::Stdout => "out",
                                Stream::Stderr => "err",
                            })
                            .small()
                            .color(egui::Color32::DARK_GRAY),
                        );
                        ui.label(
                            egui::RichText::new(text)
                                .color(color)
                                .family(egui::FontFamily::Monospace),
                        );
                        ui.end_row();
                    }
                });
        });
    ui.label(format!("{total} 行"));
}

/// Badge for an entry's current state, accounting for stopped (no task) entries.
fn entry_state_badge(e: &Entry) -> &'static str {
    match e.task.as_ref().map(|t| t.state) {
        Some(TaskState::Running) => "[运行中]",
        Some(TaskState::Finished) => "[完成]",
        Some(TaskState::Failed) => "[失败]",
        Some(TaskState::Stopped) => "[已停止]",
        None => "[未运行]",
    }
}

enum TaskAction {
    Run(u64),
    Stop(u64),
    Remove(u64),
    ToggleAutostart(u64),
}
