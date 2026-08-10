//! egui application: a session selector and a terminal view, toggled (never
//! shown together, per the mobile design).

use std::path::PathBuf;
use std::time::Duration;

use egui::{Color32, RichText};

use crate::config::{Config, Host};
use crate::input::key_event_to_bytes;
use crate::render::paint_terminal;
use crate::ssh::{FromSsh, SshConnection, ToSsh};

#[derive(PartialEq)]
enum View {
    Selector,
    Terminal,
}

/// Rough Android status-bar height (points) so top panels don't sit under the
/// system clock. Zero elsewhere. A proper safe-area inset is future work.
fn status_bar_inset() -> f32 {
    if cfg!(target_os = "android") {
        28.0
    } else {
        0.0
    }
}

/// The add/edit-host form, when open.
struct HostEditor {
    /// Index being edited, or None for a new host.
    index: Option<usize>,
    host: Host,
}

pub struct TmuxmuxApp {
    data_dir: PathBuf,
    config: Config,
    view: View,

    conn: Option<SshConnection>,
    /// Which configured host the live connection belongs to.
    connected_idx: Option<usize>,
    status: String,
    sessions: Vec<String>,
    /// True once we've received a session list for the current connection.
    listed: bool,

    // Terminal state.
    parser: vt100_ctt::Parser,
    acs: crate::acs::AcsFilter,
    attached: bool,
    attached_session: String,
    grid_cols: usize,
    grid_rows: usize,
    font_size: f32,

    editor: Option<HostEditor>,
    new_session: String,
}

impl TmuxmuxApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        data_dir: PathBuf,
        import_dir: Option<PathBuf>,
    ) -> Self {
        log::info!("TmuxmuxApp::new: data_dir={}", data_dir.display());
        cc.egui_ctx.style_mut(|s| s.visuals = egui::Visuals::dark());

        // Android eframe sometimes never issues the first RedrawRequested, so
        // update() never runs. Drive repaints from a background thread.
        let ctx = cc.egui_ctx.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(100));
            ctx.request_repaint();
        });

        let mut config = Config::load(&data_dir);
        let mut import_status = None;
        if let Some(dir) = import_dir {
            if let Some(msg) = config.import_from(&dir) {
                log::info!("import: {msg}");
                config.save(&data_dir);
                import_status = Some(msg);
            }
        }
        TmuxmuxApp {
            data_dir,
            config,
            view: View::Selector,
            conn: None,
            connected_idx: None,
            status: import_status.unwrap_or_else(|| "Select a host".into()),
            sessions: Vec::new(),
            listed: false,
            parser: vt100_ctt::Parser::new(24, 80, 0),
            acs: crate::acs::AcsFilter::new(),
            attached: false,
            attached_session: String::new(),
            grid_cols: 80,
            grid_rows: 24,
            font_size: 15.0,
            editor: None,
            new_session: String::new(),
        }
    }

    fn save_config(&self) {
        self.config.save(&self.data_dir);
    }

    fn connect_to(&mut self, idx: usize) {
        let host = match self.config.hosts.get(idx) {
            Some(h) => h.clone(),
            None => return,
        };
        self.disconnect();
        self.status = format!("Connecting to {}…", host.display_name());
        self.sessions.clear();
        self.listed = false;
        self.connected_idx = Some(idx);
        let conn = SshConnection::connect(host);
        conn.send(ToSsh::ListSessions);
        self.conn = Some(conn);
    }

    fn disconnect(&mut self) {
        if let Some(conn) = &self.conn {
            conn.send(ToSsh::Disconnect);
        }
        self.conn = None;
        self.connected_idx = None;
        self.attached = false;
        self.listed = false;
    }

    fn attach(&mut self, session: String) {
        let conn = match &self.conn {
            Some(c) => c,
            None => return,
        };
        // Fresh parser sized to the current grid.
        self.parser = vt100_ctt::Parser::new(self.grid_rows as u16, self.grid_cols as u16, 0);
        self.acs = crate::acs::AcsFilter::new();
        conn.send(ToSsh::Attach {
            session: session.clone(),
            rows: self.grid_rows as u16,
            cols: self.grid_cols as u16,
        });
        self.attached_session = session;
        self.attached = false; // set true on FromSsh::Attached
        self.view = View::Terminal;
    }

    fn detach_to_selector(&mut self) {
        if let Some(conn) = &self.conn {
            conn.send(ToSsh::Detach);
            conn.send(ToSsh::ListSessions);
        }
        self.attached = false;
        self.view = View::Selector;
    }

    /// Drain SSH events into app state.
    fn pump_ssh(&mut self) {
        let events = match &self.conn {
            Some(c) => c.poll(),
            None => return,
        };
        for ev in events {
            match ev {
                FromSsh::Status(s) => self.status = s,
                FromSsh::Error(e) => {
                    self.status = format!("⚠ {e}");
                    log::warn!("ssh: {e}");
                }
                FromSsh::SessionList(list) => {
                    self.sessions = list;
                    self.listed = true;
                }
                FromSsh::Attached => {
                    self.attached = true;
                    self.status = format!("Attached: {}", self.attached_session);
                }
                FromSsh::Data(bytes) => {
                    let mut filtered = Vec::new();
                    self.acs.feed(&bytes, &mut filtered);
                    self.parser.process(&filtered);
                }
                FromSsh::Detached => {
                    self.attached = false;
                    if self.view == View::Terminal {
                        self.view = View::Selector;
                    }
                }
                FromSsh::Closed => {
                    self.status = "Disconnected".into();
                    self.conn = None;
                    self.connected_idx = None;
                    self.attached = false;
                    self.view = View::Selector;
                }
            }
        }
    }

    // ---------- terminal input ----------

    fn handle_terminal_input(&mut self, ctx: &egui::Context) {
        let (events, modifiers) = ctx.input(|i| (i.events.clone(), i.modifiers));
        let app_cursor = self.parser.screen().application_cursor();
        let mut out: Vec<u8> = Vec::new();
        for event in &events {
            match event {
                egui::Event::Key { key, pressed: true, modifiers: mods, .. } => {
                    let bytes = key_event_to_bytes(*key, *mods, app_cursor);
                    out.extend_from_slice(&bytes);
                }
                egui::Event::Text(t) => {
                    if modifiers.alt && !modifiers.ctrl {
                        out.push(0x1b);
                    }
                    out.extend_from_slice(t.as_bytes());
                }
                // egui-winit turns Ctrl+C/X/V into these. Keep Ctrl+C as SIGINT.
                egui::Event::Copy => out.push(0x03),
                egui::Event::Cut => out.push(0x18),
                egui::Event::Paste(s) => {
                    let normalized = s.replace("\r\n", "\r").replace('\n', "\r");
                    if self.parser.screen().bracketed_paste() {
                        out.extend_from_slice(b"\x1b[200~");
                        out.extend_from_slice(normalized.as_bytes());
                        out.extend_from_slice(b"\x1b[201~");
                    } else {
                        out.extend_from_slice(normalized.as_bytes());
                    }
                }
                _ => {}
            }
        }
        if !out.is_empty() {
            if let Some(conn) = &self.conn {
                conn.send(ToSsh::Input(out));
            }
        }
    }

    // ---------- views ----------

    fn ui_selector(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("sel_top").show(ctx, |ui| {
            ui.add_space(status_bar_inset());
            ui.horizontal(|ui| {
                ui.heading("tmuxmux");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("➕ Host").clicked() {
                        self.editor = Some(HostEditor {
                            index: None,
                            host: Host::default(),
                        });
                    }
                });
            });
        });

        egui::TopBottomPanel::bottom("sel_status").show(ctx, |ui| {
            ui.label(RichText::new(&self.status).small().color(Color32::GRAY));
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                // Snapshot host list to avoid borrow conflicts with actions.
                let hosts: Vec<(usize, String)> = self
                    .config
                    .hosts
                    .iter()
                    .enumerate()
                    .map(|(i, h)| (i, h.display_name()))
                    .collect();

                if hosts.is_empty() {
                    ui.add_space(20.0);
                    ui.label("No hosts yet. Tap ➕ Host to add one.");
                }

                let mut connect_req: Option<usize> = None;
                let mut edit_req: Option<usize> = None;
                let mut attach_req: Option<String> = None;
                let mut new_req: Option<String> = None;

                for (i, name) in hosts {
                    let connected = self.connected_idx == Some(i);
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let dot = if connected { "🟢" } else { "⚪" };
                        if ui
                            .button(RichText::new(format!("{dot} {name}")).size(17.0))
                            .clicked()
                        {
                            connect_req = Some(i);
                        }
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if ui.small_button("✎").clicked() {
                                    edit_req = Some(i);
                                }
                            },
                        );
                    });

                    if connected {
                        ui.indent("sessions", |ui| {
                            if !self.listed {
                                ui.label(RichText::new("listing sessions…").italics().small());
                            } else if self.sessions.is_empty() {
                                ui.label(RichText::new("no sessions").italics().small());
                            }
                            for s in &self.sessions {
                                if ui
                                    .button(RichText::new(format!("  ▸ {s}")).size(16.0))
                                    .clicked()
                                {
                                    attach_req = Some(s.clone());
                                }
                            }
                            ui.horizontal(|ui| {
                                ui.label("new:");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.new_session)
                                        .desired_width(120.0)
                                        .hint_text("name"),
                                );
                                if ui.button("＋ create").clicked()
                                    && !self.new_session.trim().is_empty()
                                {
                                    new_req = Some(self.new_session.trim().to_string());
                                }
                            });
                        });
                    }
                }

                if let Some(i) = connect_req {
                    if self.connected_idx == Some(i) {
                        // Tapping the connected host refreshes its list.
                        if let Some(c) = &self.conn {
                            self.listed = false;
                            c.send(ToSsh::ListSessions);
                        }
                    } else {
                        self.connect_to(i);
                    }
                }
                if let Some(i) = edit_req {
                    if let Some(h) = self.config.hosts.get(i) {
                        self.editor = Some(HostEditor {
                            index: Some(i),
                            host: h.clone(),
                        });
                    }
                }
                if let Some(s) = new_req {
                    self.new_session.clear();
                    self.attach(s);
                }
                if let Some(s) = attach_req {
                    self.attach(s);
                }
            });
        });

        self.ui_editor(ctx);
    }

    fn ui_editor(&mut self, ctx: &egui::Context) {
        let mut editor = match self.editor.take() {
            Some(e) => e,
            None => return,
        };
        let mut open = true;
        let mut action: Option<EditorAction> = None;

        egui::Window::new(if editor.index.is_some() {
            "Edit host"
        } else {
            "Add host"
        })
        .collapsible(false)
        .resizable(true)
        .default_width(360.0)
        .open(&mut open)
        .show(ctx, |ui| {
            egui::Grid::new("host_form")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Label");
                    ui.text_edit_singleline(&mut editor.host.label);
                    ui.end_row();
                    ui.label("Host");
                    ui.text_edit_singleline(&mut editor.host.host);
                    ui.end_row();
                    ui.label("Port");
                    let mut port = editor.host.port.to_string();
                    if ui.text_edit_singleline(&mut port).changed() {
                        if let Ok(p) = port.parse() {
                            editor.host.port = p;
                        }
                    }
                    ui.end_row();
                    ui.label("Username");
                    ui.text_edit_singleline(&mut editor.host.username);
                    ui.end_row();
                    ui.label("Password");
                    ui.add(egui::TextEdit::singleline(&mut editor.host.password).password(true));
                    ui.end_row();
                    ui.label("Private key");
                    ui.add(
                        egui::TextEdit::multiline(&mut editor.host.private_key)
                            .desired_rows(3)
                            .hint_text("optional; overrides password"),
                    );
                    ui.end_row();
                    ui.label("Key passphrase");
                    ui.add(
                        egui::TextEdit::singleline(&mut editor.host.key_passphrase).password(true),
                    );
                    ui.end_row();
                });

            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    action = Some(EditorAction::Save);
                }
                if editor.index.is_some() && ui.button("🗑 Delete").clicked() {
                    action = Some(EditorAction::Delete);
                }
                if ui.button("Cancel").clicked() {
                    action = Some(EditorAction::Cancel);
                }
            });
        });

        match action {
            Some(EditorAction::Save) => {
                match editor.index {
                    Some(i) if i < self.config.hosts.len() => {
                        self.config.hosts[i] = editor.host.clone();
                    }
                    _ => self.config.hosts.push(editor.host.clone()),
                }
                self.save_config();
            }
            Some(EditorAction::Delete) => {
                if let Some(i) = editor.index {
                    if i < self.config.hosts.len() {
                        self.config.hosts.remove(i);
                        self.save_config();
                    }
                }
            }
            Some(EditorAction::Cancel) => {}
            None => {
                if open {
                    // Window still open — keep editing next frame.
                    self.editor = Some(editor);
                }
            }
        }
    }

    fn ui_terminal(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("term_top").show(ctx, |ui| {
            ui.add_space(status_bar_inset());
            ui.horizontal(|ui| {
                if ui.button("≡ Sessions").clicked() {
                    self.detach_to_selector();
                }
                ui.separator();
                let label = if self.attached {
                    format!("● {}", self.attached_session)
                } else {
                    format!("… {}", self.attached_session)
                };
                ui.label(label);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("A+").clicked() {
                        self.font_size = (self.font_size + 1.0).min(32.0);
                    }
                    if ui.small_button("A−").clicked() {
                        self.font_size = (self.font_size - 1.0).max(8.0);
                    }
                });
            });
        });

        // Capture keyboard before painting so this frame reflects it.
        self.handle_terminal_input(ctx);

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::BLACK))
            .show(ctx, |ui| {
                let rect = ui.available_rect_before_wrap();
                let grid = paint_terminal(ui, rect, self.parser.screen(), self.attached, self.font_size);
                // Resize the remote PTY + local parser if the grid changed.
                if grid.cols != self.grid_cols || grid.rows != self.grid_rows {
                    self.grid_cols = grid.cols;
                    self.grid_rows = grid.rows;
                    self.parser
                        .screen_mut()
                        .set_size(grid.rows as u16, grid.cols as u16);
                    if self.attached {
                        if let Some(conn) = &self.conn {
                            conn.send(ToSsh::Resize {
                                rows: grid.rows as u16,
                                cols: grid.cols as u16,
                            });
                        }
                    }
                }
            });
    }
}

enum EditorAction {
    Save,
    Delete,
    Cancel,
}

#[allow(deprecated)]
impl eframe::App for TmuxmuxApp {
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.pump_ssh();

        match self.view {
            View::Selector => self.ui_selector(ctx),
            View::Terminal => self.ui_terminal(ctx),
        }

        // Steady repaint while connected so PTY output and status flow in
        // without needing an input event to wake egui.
        let active = self.conn.is_some();
        ctx.request_repaint_after(Duration::from_millis(if active { 16 } else { 250 }));
    }
}

impl Drop for TmuxmuxApp {
    fn drop(&mut self) {
        self.disconnect();
    }
}
