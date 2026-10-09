//! The desktop window: sign-in, the room list, and the open room.
//!
//! Wide windows show the room list in a sidebar beside the open room.
//! Narrow ones show one at a time, with a back button.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use chrono::{Datelike, Local, TimeZone};
use eframe::egui::{
    self, Align, Align2, Color32, FontData, FontDefinitions, FontFamily, FontId, Id, Key, Layout,
    Margin, Modifiers, Rect, RichText, Sense, Shape, Stroke, TextEdit, Ui, pos2,
    text::{LayoutJob, TextFormat, TextWrapping},
    vec2,
};

use crate::backend::{Backend, Command, Event, SyncStatus};
use crate::matrix::RoomRow;
use crate::timeline::{BodyKind, MessageRow, SendState, TimelineRow};

/// Ngwa's accent: a warm orange.
const ACCENT: Color32 = Color32::from_rgb(232, 116, 59);
const ROOM_ROW_HEIGHT: f32 = 62.0;
/// Below this width, show the room list and the room one at a time.
const NARROW: f32 = 720.0;
/// Messages from the same person within this gap share one header.
const GROUP_GAP_MS: i64 = 5 * 60_000;
/// Avatar and name colours, picked by a hash of the room or user ID so
/// each keeps its colour between runs.
const PEOPLE_COLORS: [Color32; 8] = [
    Color32::from_rgb(76, 110, 245),
    Color32::from_rgb(18, 150, 120),
    Color32::from_rgb(196, 72, 120),
    Color32::from_rgb(214, 140, 30),
    Color32::from_rgb(120, 86, 220),
    Color32::from_rgb(36, 140, 190),
    Color32::from_rgb(200, 80, 60),
    Color32::from_rgb(90, 140, 60),
];

/// Open the window and run until it is closed.
pub fn run(demo: bool, started: Instant) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("ngwa-sync")
        .build()?;
    let handle = runtime.handle().clone();

    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Ngwa")
            .with_app_id("chat.ngwa.Ngwa")
            .with_inner_size([1080.0, 720.0])
            .with_min_inner_size([360.0, 480.0])
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon/ngwa-256.png"))
                    .expect("the bundled icon is a valid PNG"),
            ),
        ..Default::default()
    };
    prefer_x11_on_wsl(&mut options);

    let result = eframe::run_native(
        "Ngwa",
        options,
        Box::new(move |cc| {
            setup_fonts(&cc.egui_ctx);
            setup_style(&cc.egui_ctx);
            let backend = Backend::start(&handle, cc.egui_ctx.clone(), demo);
            Ok(Box::new(App::new(backend, started)))
        }),
    );

    // The window is gone; give the engine a moment to stop syncing cleanly.
    runtime.shutdown_timeout(Duration::from_secs(2));
    result.map_err(|e| anyhow!("could not open the window: {e}"))
}

/// WSL's Wayland support draws Linux-style title bars and leaves a ghost
/// outline of the window's old size after maximising. Its X11 support
/// gives a normal Windows title bar instead, so use that there.
fn prefer_x11_on_wsl(options: &mut eframe::NativeOptions) {
    #[cfg(target_os = "linux")]
    {
        let on_wsl = std::env::var_os("WSL_DISTRO_NAME").is_some()
            || std::fs::read_to_string("/proc/sys/kernel/osrelease")
                .is_ok_and(|release| release.to_lowercase().contains("microsoft"));
        let wanted = std::env::var_os("NGWA_WAYLAND").is_none();
        if on_wsl && wanted && std::env::var_os("DISPLAY").is_some() {
            options.event_loop_builder = Some(Box::new(|builder| {
                use winit::platform::x11::EventLoopBuilderExtX11;
                builder.with_x11();
            }));
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = options;
}

// ---- Look and feel -------------------------------------------------------

fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "inter".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/Inter-Regular.otf"
        ))),
    );
    fonts.font_data.insert(
        "inter-semibold".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/Inter-SemiBold.otf"
        ))),
    );
    // Inter first; egui's own fonts stay behind it for emoji and symbols.
    let fallbacks = fonts.families[&FontFamily::Proportional].clone();
    fonts
        .families
        .get_mut(&FontFamily::Proportional)
        .expect("egui always defines a proportional family")
        .insert(0, "inter".into());
    let mut semibold = vec!["inter-semibold".to_owned()];
    semibold.extend(fallbacks);
    fonts
        .families
        .insert(FontFamily::Name("semibold".into()), semibold);
    ctx.set_fonts(fonts);
}

fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

fn setup_style(ctx: &egui::Context) {
    use egui::TextStyle::{Body, Button, Heading, Monospace, Small};
    ctx.all_styles_mut(|style| {
        style.text_styles.insert(Body, FontId::proportional(14.5));
        style.text_styles.insert(Button, FontId::proportional(14.0));
        style
            .text_styles
            .insert(Heading, FontId::proportional(22.0));
        style.text_styles.insert(Small, FontId::proportional(12.0));
        style.text_styles.insert(Monospace, FontId::monospace(13.0));
        style.spacing.item_spacing = vec2(8.0, 6.0);
        style.spacing.button_padding = vec2(12.0, 6.0);
        style.spacing.scroll.floating = true;
        style.visuals.selection.bg_fill = ACCENT.gamma_multiply(0.45);
        style.visuals.hyperlink_color = ACCENT;
        for widget in [
            &mut style.visuals.widgets.inactive,
            &mut style.visuals.widgets.hovered,
            &mut style.visuals.widgets.active,
            &mut style.visuals.widgets.open,
            &mut style.visuals.widgets.noninteractive,
        ] {
            widget.corner_radius = 8.into();
        }
        style.visuals.window_corner_radius = 12.into();
    });
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        let v = &mut style.visuals;
        v.panel_fill = Color32::from_rgb(31, 32, 36);
        v.window_fill = Color32::from_rgb(38, 39, 44);
        v.extreme_bg_color = Color32::from_rgb(42, 43, 49);
        v.faint_bg_color = Color32::from_rgb(36, 37, 42);
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(48, 49, 55));
        v.widgets.inactive.weak_bg_fill = Color32::from_rgb(46, 47, 53);
        v.widgets.inactive.bg_fill = Color32::from_rgb(46, 47, 53);
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(54, 55, 62);
        v.override_text_color = None;
    });
    ctx.style_mut_of(egui::Theme::Light, |style| {
        let v = &mut style.visuals;
        v.panel_fill = Color32::WHITE;
        v.window_fill = Color32::WHITE;
        v.extreme_bg_color = Color32::from_rgb(240, 240, 243);
        v.faint_bg_color = Color32::from_rgb(246, 246, 248);
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(228, 228, 232));
    });
}

/// Colours that depend on the light or dark theme.
struct Palette {
    sidebar: Color32,
    main: Color32,
    raised: Color32,
    hover: Color32,
    text: Color32,
    strong: Color32,
    weak: Color32,
}

fn palette(ui: &Ui) -> Palette {
    if ui.visuals().dark_mode {
        Palette {
            sidebar: Color32::from_rgb(24, 25, 28),
            main: Color32::from_rgb(31, 32, 36),
            raised: Color32::from_rgb(42, 43, 49),
            hover: Color32::from_rgb(40, 41, 47),
            text: Color32::from_rgb(214, 215, 220),
            strong: Color32::from_rgb(242, 242, 245),
            weak: Color32::from_rgb(142, 143, 152),
        }
    } else {
        Palette {
            sidebar: Color32::from_rgb(244, 244, 246),
            main: Color32::WHITE,
            raised: Color32::from_rgb(238, 238, 242),
            hover: Color32::from_rgb(233, 233, 238),
            text: Color32::from_rgb(40, 40, 46),
            strong: Color32::from_rgb(16, 16, 20),
            weak: Color32::from_rgb(110, 111, 120),
        }
    }
}

// ---- State ---------------------------------------------------------------

#[derive(PartialEq)]
enum Phase {
    /// Checking for a saved session. Usually too brief to notice.
    Starting,
    SignedOut,
    SigningIn,
    SignedIn,
}

#[derive(Default)]
struct SignInForm {
    id: String,
    homeserver: String,
    password: String,
    error: Option<String>,
    focused: bool,
}

/// The room on screen.
struct RoomView {
    id: String,
    rows: Vec<TimelineRow>,
    /// The first batch of messages has arrived.
    loaded: bool,
    history_loading: bool,
    reached_start: bool,
    /// Row count when older messages were last requested, so a request
    /// that brings nothing new isn't repeated in a loop.
    requested_at_len: Option<usize>,
    draft: String,
    reply_to: Option<Reply>,
    focus_composer: bool,
    /// Older messages were just added above: keep the view where it was.
    anchor_height: Option<f32>,
    scroll_to: Option<f32>,
    content_height: f32,
    was_invite: bool,
    /// False where only admins may post, such as server notice rooms.
    can_send: bool,
}

#[derive(Clone)]
struct Reply {
    event_id: String,
    sender_name: String,
    body: String,
}

impl RoomView {
    fn new(id: String, is_invite: bool) -> Self {
        Self {
            id,
            rows: Vec::new(),
            loaded: false,
            history_loading: false,
            reached_start: false,
            requested_at_len: None,
            draft: String::new(),
            reply_to: None,
            focus_composer: true,
            anchor_height: None,
            scroll_to: None,
            content_height: 0.0,
            was_invite: is_invite,
            can_send: true,
        }
    }
}

/// What the user did in the timeline this frame.
#[derive(Default)]
struct TimelineActions {
    reply: Option<Reply>,
    retry: Option<String>,
    delete: Option<String>,
}

struct App {
    backend: Backend,
    phase: Phase,
    form: SignInForm,

    user_id: String,
    rooms: Vec<RoomRow>,
    list_loaded: bool,
    sync: SyncStatus,
    error: Option<String>,
    search: String,
    open: Option<RoomView>,
    confirm_sign_out_until: Option<Instant>,

    /// When to measure "rooms shown in" from: launch, or pressing Sign in.
    measure_from: Instant,
    rooms_shown_after: Option<Duration>,
    memory_mb: Option<usize>,
    memory_checked: Option<Instant>,
    title: String,
}

impl App {
    fn new(backend: Backend, started: Instant) -> Self {
        Self {
            backend,
            phase: Phase::Starting,
            form: SignInForm::default(),
            user_id: String::new(),
            rooms: Vec::new(),
            list_loaded: false,
            sync: SyncStatus::Connecting,
            error: None,
            search: String::new(),
            open: None,
            confirm_sign_out_until: None,
            measure_from: started,
            rooms_shown_after: None,
            memory_mb: None,
            memory_checked: None,
            title: "Ngwa".into(),
        }
    }

    fn handle_events(&mut self) {
        while let Some(event) = self.backend.try_recv() {
            match event {
                Event::SignedOut => {
                    self.phase = Phase::SignedOut;
                    self.rooms.clear();
                    self.list_loaded = false;
                    self.open = None;
                    self.search.clear();
                    self.form.password.clear();
                    self.form.focused = false;
                }
                Event::LoginFailed(message) => {
                    self.phase = Phase::SignedOut;
                    self.form.error = Some(message);
                }
                Event::SignedIn { user_id } => {
                    self.phase = Phase::SignedIn;
                    self.user_id = user_id;
                    self.form = SignInForm::default();
                    self.error = None;
                    self.sync = SyncStatus::Connecting;
                }
                Event::Rooms(rooms) => {
                    if self.rooms_shown_after.is_none() && !rooms.is_empty() {
                        self.rooms_shown_after = Some(self.measure_from.elapsed());
                    }
                    self.rooms = rooms;
                    self.follow_open_room();
                }
                Event::ListLoaded => self.list_loaded = true,
                Event::Sync(status) => self.sync = status,
                Event::Timeline { room_id, rows } => {
                    if let Some(view) = self.open.as_mut().filter(|v| v.id == room_id) {
                        if older_messages_added(&view.rows, &rows) {
                            view.anchor_height = Some(view.content_height);
                        }
                        view.rows = rows;
                        view.loaded = true;
                    }
                }
                Event::RoomInfo { room_id, can_send } => {
                    if let Some(view) = self.open.as_mut().filter(|v| v.id == room_id) {
                        view.can_send = can_send;
                    }
                }
                Event::History {
                    room_id,
                    loading,
                    reached_start,
                } => {
                    if let Some(view) = self.open.as_mut().filter(|v| v.id == room_id) {
                        view.history_loading = loading;
                        view.reached_start |= reached_start;
                    }
                }
                Event::Error(message) => {
                    if self.phase == Phase::SignedIn {
                        self.error = Some(message);
                    } else {
                        self.form.error = Some(message);
                    }
                }
            }
        }
    }

    /// Keep the open room in step with the room list: close it if it's
    /// gone (left, or an invite was declined), and reopen it once an
    /// accepted invite becomes a joined room, so its messages load.
    fn follow_open_room(&mut self) {
        let Some(view) = &mut self.open else { return };
        match self.rooms.iter().find(|r| r.id == view.id) {
            None => self.open = None,
            Some(room) if view.was_invite && !room.is_invite => {
                let id = view.id.clone();
                *view = RoomView::new(id.clone(), false);
                self.backend.send(Command::OpenRoom(id));
            }
            Some(_) => {}
        }
    }

    fn open_room(&mut self, id: &str) {
        if self.open.as_ref().is_some_and(|v| v.id == id) {
            return;
        }
        let is_invite = self.rooms.iter().any(|r| r.id == id && r.is_invite);
        self.open = Some(RoomView::new(id.to_owned(), is_invite));
        self.backend.send(Command::OpenRoom(id.to_owned()));
    }

    /// Show the total unread count in the window title, like other chat apps.
    fn update_title(&mut self, ctx: &egui::Context) {
        let unread: u64 = self.rooms.iter().map(|r| r.unread).sum();
        let title = if self.phase == Phase::SignedIn && unread > 0 {
            format!("Ngwa ({unread})")
        } else {
            "Ngwa".into()
        };
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }

    fn refresh_memory(&mut self, ctx: &egui::Context) {
        let due = self
            .memory_checked
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(2));
        if due {
            self.memory_mb = memory_stats::memory_stats().map(|m| m.physical_mem / (1024 * 1024));
            self.memory_checked = Some(Instant::now());
        }
        ctx.request_repaint_after(Duration::from_secs(2));
    }

    fn submit_sign_in(&mut self) {
        if self.phase == Phase::SigningIn {
            return;
        }
        self.form.error = None;
        self.phase = Phase::SigningIn;
        self.measure_from = Instant::now();
        self.rooms_shown_after = None;
        self.backend.send(Command::Login {
            id: self.form.id.trim().to_owned(),
            homeserver: self.form.homeserver.trim().to_owned(),
            password: self.form.password.clone(),
        });
    }

    // ---- Screens ---------------------------------------------------------

    fn starting_ui(&mut self, ui: &mut Ui) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.centered_and_justified(|ui| {
                ui.spinner();
            });
        });
    }

    fn sign_in_ui(&mut self, ui: &mut Ui) {
        let signing_in = self.phase == Phase::SigningIn;
        let pal = palette(ui);
        let mut submit = false;

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(pal.sidebar))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space((ui.available_height() * 0.16).max(32.0));
                        ui.label(RichText::new("Ngwa").font(semibold(40.0)).color(ACCENT));
                        ui.label(RichText::new("A fast, native Matrix client").color(pal.weak));
                        ui.add_space(24.0);

                        let width = 320.0_f32.min(ui.available_width() - 24.0);
                        egui::Frame::new()
                            .fill(pal.main)
                            .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
                            .inner_margin(Margin::same(22))
                            .corner_radius(14)
                            .show(ui, |ui| {
                                ui.set_width(width);
                                ui.with_layout(Layout::top_down(Align::LEFT), |ui| {
                                    ui.spacing_mut().item_spacing.y = 6.0;

                                    ui.label(RichText::new("Matrix ID").color(pal.weak));
                                    let id = ui.add_enabled(
                                        !signing_in,
                                        field(&mut self.form.id).hint_text("@you:matrix.org"),
                                    );
                                    if !self.form.focused {
                                        id.request_focus();
                                        self.form.focused = true;
                                    }

                                    // A full Matrix ID names its server already.
                                    if !self.form.id.contains(':') {
                                        ui.add_space(6.0);
                                        ui.label(RichText::new("Homeserver").color(pal.weak));
                                        ui.add_enabled(
                                            !signing_in,
                                            field(&mut self.form.homeserver)
                                                .hint_text("matrix.org"),
                                        );
                                    }

                                    ui.add_space(6.0);
                                    ui.label(RichText::new("Password").color(pal.weak));
                                    ui.add_enabled(
                                        !signing_in,
                                        field(&mut self.form.password).password(true),
                                    );

                                    if let Some(error) = &self.form.error {
                                        ui.add_space(4.0);
                                        ui.label(
                                            RichText::new(error).color(ui.visuals().error_fg_color),
                                        );
                                    }

                                    ui.add_space(10.0);
                                    let ready = !self.form.id.trim().is_empty()
                                        && !self.form.password.is_empty();
                                    let label = if signing_in {
                                        "Signing in…"
                                    } else {
                                        "Sign in"
                                    };
                                    if accent_button(ui, label, ready && !signing_in, 38.0) {
                                        submit = true;
                                    }
                                    if signing_in {
                                        ui.vertical_centered(|ui| ui.spinner());
                                    }
                                });
                            });

                        ui.add_space(14.0);
                        ui.label(
                            RichText::new("Your password goes only to your homeserver.")
                                .small()
                                .color(pal.weak),
                        );
                    });
                });
            });

        let enter = ui.input(|i| i.key_pressed(Key::Enter));
        if (submit || enter) && !self.form.id.trim().is_empty() && !self.form.password.is_empty() {
            self.submit_sign_in();
        }
    }

    fn main_ui(&mut self, ui: &mut Ui) {
        let pal = palette(ui);
        let narrow = ui.available_width() < NARROW;

        if narrow {
            if self.open.is_some() {
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(pal.main))
                    .show(ui, |ui| self.room_ui(ui, true));
            } else {
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(pal.sidebar))
                    .show(ui, |ui| self.sidebar_ui(ui));
            }
            return;
        }

        egui::Panel::left("sidebar")
            .resizable(true)
            .default_size(320.0)
            .size_range(260.0..=460.0)
            .frame(egui::Frame::new().fill(pal.sidebar))
            .show(ui, |ui| self.sidebar_ui(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(pal.main))
            .show(ui, |ui| {
                if self.open.is_some() {
                    self.room_ui(ui, false);
                } else {
                    welcome_ui(ui, &pal);
                }
            });
    }

    // ---- Sidebar ---------------------------------------------------------

    fn sidebar_ui(&mut self, ui: &mut Ui) {
        let pal = palette(ui);

        egui::Panel::top("sidebar-header")
            .frame(egui::Frame::new().inner_margin(Margin {
                left: 16,
                right: 12,
                top: 12,
                bottom: 8,
            }))
            .show_separator_line(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Ngwa").font(semibold(19.0)).color(ACCENT));
                    ui.add_space(2.0);
                    sync_indicator(ui, &self.sync, &pal);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        self.sign_out_button(ui);
                    });
                });
                ui.add_space(6.0);
                let search = ui.add(
                    TextEdit::singleline(&mut self.search)
                        .hint_text("Search rooms   Ctrl+K")
                        .desired_width(f32::INFINITY)
                        .margin(Margin::symmetric(10, 7)),
                );
                if ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::K)) {
                    search.request_focus();
                }
                if search.has_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
                    self.search.clear();
                }
                if let Some(error) = self.error.clone() {
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(error)
                                .small()
                                .color(ui.visuals().error_fg_color),
                        );
                        if ui.small_button("Dismiss").clicked() {
                            self.error = None;
                        }
                    });
                }
            });

        egui::Panel::bottom("sidebar-footer")
            .frame(egui::Frame::new().inner_margin(Margin::symmetric(16, 8)))
            .show_separator_line(false)
            .show(ui, |ui| {
                ui.label(RichText::new(&self.user_id).small().color(pal.weak));
                let mut parts = vec![format!("{} rooms", self.rooms.len())];
                if let Some(after) = self.rooms_shown_after {
                    parts.push(format!("shown in {} ms", after.as_millis()));
                }
                if let Some(mb) = self.memory_mb {
                    parts.push(format!("{mb} MB"));
                }
                ui.label(RichText::new(parts.join(" · ")).small().color(pal.weak))
                    .on_hover_text(format!(
                        "Ngwa {}\nRooms in your list · time until they were on screen · memory in use",
                        env!("CARGO_PKG_VERSION")
                    ));
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new().inner_margin(Margin::symmetric(6, 0)))
            .show(ui, |ui| self.room_list_ui(ui, &pal));
    }

    fn sign_out_button(&mut self, ui: &mut Ui) {
        let confirming = self
            .confirm_sign_out_until
            .is_some_and(|until| Instant::now() < until);
        let label = if confirming { "Sign out?" } else { "Sign out" };
        let button = ui
            .add(egui::Button::new(RichText::new(label).small()).frame(confirming))
            .on_hover_text(format!("Signed in as {}", self.user_id));
        if button.clicked() {
            if confirming {
                self.confirm_sign_out_until = None;
                self.backend.send(Command::Logout);
            } else {
                self.confirm_sign_out_until = Some(Instant::now() + Duration::from_secs(4));
                ui.ctx().request_repaint_after(Duration::from_secs(4));
            }
        }
    }

    fn room_list_ui(&mut self, ui: &mut Ui, pal: &Palette) {
        let query = self.search.trim().to_lowercase();
        let visible: Vec<usize> = self
            .rooms
            .iter()
            .enumerate()
            .filter(|(_, r)| query.is_empty() || r.name.to_lowercase().contains(&query))
            .map(|(i, _)| i)
            .collect();

        if visible.is_empty() {
            room_list_empty(ui, self.list_loaded, &self.rooms, &self.search, pal);
            return;
        }

        let mut clicked = None;
        egui::ScrollArea::vertical().auto_shrink(false).show_rows(
            ui,
            ROOM_ROW_HEIGHT,
            visible.len(),
            |ui, range| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for &index in &visible[range] {
                    let room = &self.rooms[index];
                    let selected = self.open.as_ref().is_some_and(|v| v.id == room.id);
                    if room_row(ui, room, selected, pal).clicked() {
                        clicked = Some(room.id.clone());
                    }
                }
            },
        );
        if let Some(id) = clicked {
            self.open_room(&id);
        }
    }

    // ---- The open room ---------------------------------------------------

    fn room_ui(&mut self, ui: &mut Ui, narrow: bool) {
        let pal = palette(ui);
        let Some(view) = self.open.as_ref() else {
            return;
        };
        let room = self.rooms.iter().find(|r| r.id == view.id).cloned();
        let name = room
            .as_ref()
            .map_or_else(|| view.id.clone(), |r| r.name.clone());
        let is_invite = room.as_ref().is_some_and(|r| r.is_invite);

        let mut go_back = false;
        egui::Panel::top("room-header")
            .frame(egui::Frame::new().inner_margin(Margin::symmetric(16, 10)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if narrow
                        && ui
                            .add(egui::Button::new(RichText::new("←").size(18.0)).frame(false))
                            .on_hover_text("Back to rooms (Esc)")
                            .clicked()
                    {
                        go_back = true;
                    }
                    let (rect, _) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::hover());
                    avatar(ui, rect.center(), 17.0, &name, &view.id);
                    ui.label(RichText::new(&name).font(semibold(16.5)).color(pal.strong));
                });
            });
        if narrow && ui.input(|i| i.key_pressed(Key::Escape)) && !self.composer_busy() {
            go_back = true;
        }
        if go_back {
            self.open = None;
            return;
        }

        if is_invite {
            egui::CentralPanel::default()
                .frame(egui::Frame::new())
                .show(ui, |ui| self.invite_ui(ui, &name, &pal));
            return;
        }

        self.composer_ui(ui, &name, &pal);
        egui::CentralPanel::default()
            .frame(egui::Frame::new())
            .show(ui, |ui| self.timeline_ui(ui, &pal));
    }

    fn composer_busy(&self) -> bool {
        self.open
            .as_ref()
            .is_some_and(|v| v.reply_to.is_some() || !v.draft.is_empty())
    }

    fn invite_ui(&mut self, ui: &mut Ui, name: &str, pal: &Palette) {
        let id = self.open.as_ref().map(|v| v.id.clone()).unwrap_or_default();
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.25).max(24.0));
            let (rect, _) = ui.allocate_exact_size(vec2(72.0, 72.0), Sense::hover());
            avatar(ui, rect.center(), 36.0, name, &id);
            ui.add_space(10.0);
            ui.label(RichText::new(name).font(semibold(20.0)).color(pal.strong));
            ui.label(RichText::new("You've been invited to join this room.").color(pal.weak));
            ui.add_space(14.0);
            egui::Frame::new().show(ui, |ui| {
                ui.set_width(220.0);
                ui.with_layout(Layout::top_down(Align::Center), |ui| {
                    if accent_button(ui, "Join", true, 36.0) {
                        self.backend.send(Command::AcceptInvite);
                    }
                    if ui
                        .add_sized(
                            vec2(ui.available_width(), 32.0),
                            egui::Button::new("Decline"),
                        )
                        .clicked()
                    {
                        self.backend.send(Command::DeclineInvite);
                    }
                });
            });
        });
    }

    fn timeline_ui(&mut self, ui: &mut Ui, pal: &Palette) {
        let Some(view) = self.open.as_mut() else {
            return;
        };

        if !view.loaded {
            ui.centered_and_justified(|ui| {
                ui.spinner();
            });
            return;
        }

        let mut scroll = egui::ScrollArea::vertical()
            .id_salt(("timeline", &view.id))
            .auto_shrink(false)
            .stick_to_bottom(true);
        if let Some(offset) = view.scroll_to.take() {
            scroll = scroll.vertical_scroll_offset(offset);
        }

        let mut actions = TimelineActions::default();
        let output = scroll.show(ui, |ui| {
            ui.add_space(8.0);
            if view.history_loading {
                ui.vertical_centered(|ui| ui.spinner());
            }
            draw_rows(ui, &view.rows, pal, &mut actions);
            ui.add_space(12.0);
        });

        // Older messages arrived above: shift down by their height so what
        // the user was reading stays put.
        if let Some(before) = view.anchor_height.take() {
            let added = output.content_size.y - before;
            if added > 0.0 {
                view.scroll_to = Some(output.state.offset.y + added);
                ui.ctx().request_repaint();
            }
        }
        view.content_height = output.content_size.y;

        // Near the top: fetch older messages.
        let near_top = output.state.offset.y < 120.0;
        if near_top
            && !view.history_loading
            && !view.reached_start
            && view.requested_at_len != Some(view.rows.len())
        {
            view.requested_at_len = Some(view.rows.len());
            self.backend.send(Command::LoadOlder);
        }

        if let Some(reply) = actions.reply {
            view.reply_to = Some(reply);
            view.focus_composer = true;
        }
        if let Some(key) = actions.retry {
            self.backend.send(Command::Retry(key));
        }
        if let Some(key) = actions.delete {
            self.backend.send(Command::DeleteUnsent(key));
        }
    }

    fn composer_ui(&mut self, ui: &mut Ui, room_name: &str, pal: &Palette) {
        let Some(view) = self.open.as_mut() else {
            return;
        };
        if !view.can_send {
            egui::Panel::bottom("composer")
                .frame(egui::Frame::new().inner_margin(Margin::symmetric(16, 16)))
                .show_separator_line(false)
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label(
                            RichText::new("Only admins can post in this room.")
                                .small()
                                .color(pal.weak),
                        );
                    });
                });
            return;
        }
        let mut send = false;

        egui::Panel::bottom("composer")
            .frame(egui::Frame::new().inner_margin(Margin {
                left: 16,
                right: 16,
                top: 8,
                bottom: 14,
            }))
            .show_separator_line(false)
            .show(ui, |ui| {
                if let Some(reply) = view.reply_to.clone() {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("Replying to {}", reply.sender_name))
                                .font(semibold(12.5))
                                .color(pal.text),
                        );
                        ui.label(
                            RichText::new(truncate(&reply.body, 60))
                                .small()
                                .color(pal.weak),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui
                                .add(egui::Button::new(RichText::new("×").size(16.0)).frame(false))
                                .on_hover_text("Cancel reply (Esc)")
                                .clicked()
                            {
                                view.reply_to = None;
                            }
                        });
                    });
                }

                let id = Id::new(("composer", &view.id));
                let focused = ui.memory(|m| m.has_focus(id));
                // Enter sends; Shift+Enter makes a new line.
                if focused && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) {
                    send = true;
                }
                if focused && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                    view.reply_to = None;
                }

                egui::Frame::new()
                    .fill(pal.raised)
                    .corner_radius(12)
                    .inner_margin(Margin {
                        left: 4,
                        right: 6,
                        top: 4,
                        bottom: 4,
                    })
                    .show(ui, |ui| {
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            let width = ui.available_width() - 74.0;
                            let edit = ui.add(
                                TextEdit::multiline(&mut view.draft)
                                    .id(id)
                                    .frame(egui::Frame::NONE)
                                    .hint_text(format!("Message {room_name}"))
                                    .desired_rows(1)
                                    .desired_width(width)
                                    .margin(Margin::symmetric(8, 7)),
                            );
                            if view.focus_composer {
                                edit.request_focus();
                                view.focus_composer = false;
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let ready = !view.draft.trim().is_empty();
                                if accent_button(ui, "Send", ready, 32.0) {
                                    send = true;
                                }
                            });
                        });
                    });
            });

        if send && !view.draft.trim().is_empty() {
            let body = view.draft.trim_end().to_owned();
            let reply_to = view.reply_to.take().map(|r| r.event_id);
            view.draft.clear();
            view.focus_composer = true;
            self.backend.send(Command::Send { body, reply_to });
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.handle_events();
        let ctx = ui.ctx().clone();
        self.update_title(&ctx);

        match self.phase {
            Phase::Starting => self.starting_ui(ui),
            Phase::SignedOut | Phase::SigningIn => self.sign_in_ui(ui),
            Phase::SignedIn => {
                self.refresh_memory(&ctx);
                self.main_ui(ui);
            }
        }
    }
}

// ---- Pieces --------------------------------------------------------------

/// A full-width text field for the sign-in form.
fn field(text: &mut String) -> TextEdit<'_> {
    TextEdit::singleline(text)
        .desired_width(f32::INFINITY)
        .margin(Margin::symmetric(10, 8))
}

/// A filled accent button with centred text. Returns true when clicked.
fn accent_button(ui: &mut Ui, label: &str, enabled: bool, height: f32) -> bool {
    let button = egui::Button::new(
        RichText::new(label)
            .font(semibold(14.0))
            .color(Color32::WHITE),
    )
    .fill(ACCENT)
    .corner_radius(10);
    let width = if ui.layout().is_horizontal() {
        (label.len() as f32 * 9.0 + 28.0).max(64.0)
    } else {
        ui.available_width()
    };
    ui.add_enabled_ui(enabled, |ui| {
        ui.add_sized(vec2(width, height), button).clicked()
    })
    .inner
}

fn welcome_ui(ui: &mut Ui, pal: &Palette) {
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() * 0.36).max(24.0));
        ui.label(
            RichText::new("Ngwa")
                .font(semibold(44.0))
                .color(ACCENT.gamma_multiply(0.85)),
        );
        ui.add_space(4.0);
        ui.label(RichText::new("Pick a room to start chatting").color(pal.weak));
        ui.label(RichText::new("Ctrl+K to search").small().color(pal.weak));
    });
}

fn sync_indicator(ui: &mut Ui, status: &SyncStatus, pal: &Palette) {
    let (color, text, detail) = match status {
        SyncStatus::Connecting => (Color32::from_rgb(214, 160, 40), "Connecting", None),
        SyncStatus::Live => (Color32::from_rgb(52, 180, 110), "", None),
        SyncStatus::Offline => (
            Color32::GRAY,
            "Offline",
            Some("Retrying when the network is back"),
        ),
        SyncStatus::Failed(e) => (
            ui.visuals().error_fg_color,
            "Sync problem",
            Some(e.as_str()),
        ),
    };
    let (dot, response) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
    ui.painter().circle_filled(dot.center(), 3.5, color);
    if text.is_empty() {
        response.on_hover_text("Connected and up to date");
    } else {
        let label = ui.label(RichText::new(text).small().color(pal.weak));
        if let Some(detail) = detail {
            label.on_hover_text(detail);
        }
    }
}

fn room_list_empty(ui: &mut Ui, loaded: bool, rooms: &[RoomRow], search: &str, pal: &Palette) {
    ui.add_space(40.0);
    ui.vertical_centered(|ui| {
        if !rooms.is_empty() {
            ui.label(RichText::new(format!("No rooms match “{}”", search.trim())).color(pal.weak));
        } else if !loaded {
            ui.spinner();
            ui.label(RichText::new("Loading your rooms…").color(pal.weak));
        } else {
            ui.label(RichText::new("No rooms yet").font(semibold(15.0)));
            ui.label(RichText::new("Rooms you join will show up here.").color(pal.weak));
        }
    });
}

fn avatar(ui: &Ui, center: egui::Pos2, radius: f32, name: &str, id: &str) {
    let painter = ui.painter();
    painter.circle_filled(center, radius, color_for(id));
    painter.text(
        center,
        Align2::CENTER_CENTER,
        initial(name),
        semibold(radius * 0.9),
        Color32::WHITE,
    );
}

/// One room in the list. Painted by hand so that thousands of rooms stay
/// cheap: only the rows on screen are ever drawn.
fn room_row(ui: &mut Ui, room: &RoomRow, selected: bool, pal: &Palette) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), ROOM_ROW_HEIGHT), Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let painter = ui.painter();

    let background = if selected {
        Some(ACCENT.gamma_multiply(0.16))
    } else if response.hovered() {
        Some(pal.hover)
    } else {
        None
    };
    if let Some(fill) = background {
        painter.rect_filled(rect.shrink2(vec2(2.0, 2.0)), 10, fill);
    }

    let center = pos2(rect.left() + 12.0 + 20.0, rect.center().y);
    avatar(ui, center, 20.0, &room.name, &room.id);
    let painter = ui.painter();

    let left = center.x + 20.0 + 12.0;
    let mut right = rect.right() - 12.0;
    let top = rect.center().y - 10.0;
    let bottom = rect.center().y + 10.0;
    let unread = room.unread > 0 || room.is_invite;

    // Top line: time on the right, name on the left.
    if let Some(ts) = room.timestamp {
        let color = if room.mentions > 0 { ACCENT } else { pal.weak };
        let galley = painter.layout_no_wrap(list_time(ts), FontId::proportional(11.5), color);
        let pos = pos2(right - galley.size().x, top - galley.size().y / 2.0);
        let width = galley.size().x;
        painter.galley(pos, galley, color);
        right -= width + 8.0;
    }
    let name_font = if unread {
        semibold(14.5)
    } else {
        FontId::proportional(14.5)
    };
    let name_color = if unread { pal.strong } else { pal.text };
    one_line(
        painter,
        &room.name,
        name_font,
        name_color,
        left,
        top,
        right - left,
    );

    // Bottom line: badge on the right, preview on the left.
    let mut right = rect.right() - 12.0;
    let badge = if room.is_invite {
        Some(("Invite".to_owned(), ACCENT, Color32::WHITE))
    } else if room.unread > 0 {
        let text = if room.unread > 99 {
            "99+".to_owned()
        } else {
            room.unread.to_string()
        };
        if room.mentions > 0 {
            Some((text, ACCENT, Color32::WHITE))
        } else {
            Some((text, pal.raised, pal.strong))
        }
    } else {
        None
    };
    if let Some((text, fill, text_color)) = badge {
        let galley = painter.layout_no_wrap(text, semibold(11.0), text_color);
        let size = vec2((galley.size().x + 12.0).max(20.0), 18.0);
        let pill = Rect::from_min_size(pos2(right - size.x, bottom - size.y / 2.0), size);
        painter.rect_filled(pill, 9, fill);
        painter.galley(pill.center() - galley.size() / 2.0, galley, text_color);
        right = pill.left() - 8.0;
    }
    if let Some(preview) = &room.preview {
        let color = if unread { pal.text } else { pal.weak };
        one_line(
            painter,
            preview,
            FontId::proportional(13.0),
            color,
            left,
            bottom,
            right - left,
        );
    }

    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Paint one line of text, cut short with an ellipsis if it's too wide.
fn one_line(
    painter: &egui::Painter,
    text: &str,
    font: FontId,
    color: Color32,
    left: f32,
    center_y: f32,
    width: f32,
) {
    let flat: String = text
        .chars()
        .map(|c| if c == '\n' { ' ' } else { c })
        .collect();
    let mut job = LayoutJob::single_section(flat, TextFormat::simple(font, color));
    job.wrap = TextWrapping::truncate_at_width(width.max(20.0));
    let galley = painter.layout_job(job);
    painter.galley(pos2(left, center_y - galley.size().y / 2.0), galley, color);
}

// ---- Timeline ------------------------------------------------------------

fn draw_rows(ui: &mut Ui, rows: &[TimelineRow], pal: &Palette, actions: &mut TimelineActions) {
    let mut previous: Option<&MessageRow> = None;
    for row in rows {
        match row {
            TimelineRow::Message(message) => {
                let starts_group = previous.is_none_or(|p| {
                    p.sender_id != message.sender_id
                        || message.timestamp - p.timestamp > GROUP_GAP_MS
                });
                message_ui(ui, message, starts_group, pal, actions);
                previous = Some(message);
            }
            TimelineRow::Day(ts) => {
                day_divider(ui, *ts, pal);
                previous = None;
            }
            TimelineRow::ReadMarker => {
                read_marker(ui);
            }
            TimelineRow::Notice { text, .. } => {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add_space(64.0);
                    ui.label(RichText::new(text).small().color(pal.weak));
                });
                previous = None;
            }
            TimelineRow::Start => {
                ui.add_space(16.0);
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new("This is the beginning of the conversation.")
                            .small()
                            .color(pal.weak),
                    );
                });
            }
        }
    }
}

fn message_ui(
    ui: &mut Ui,
    message: &MessageRow,
    starts_group: bool,
    pal: &Palette,
    actions: &mut TimelineActions,
) {
    ui.add_space(if starts_group { 10.0 } else { 1.0 });
    // Reserve the hover background now so it sits behind the text.
    let background = ui.painter().add(Shape::Noop);
    let full_width = ui.available_width();

    let inner = ui.horizontal_top(|ui| {
        ui.add_space(16.0);
        let gutter_height = if starts_group { 36.0 } else { 20.0 };
        let (gutter, _) = ui.allocate_exact_size(vec2(36.0, gutter_height), Sense::hover());
        if starts_group {
            avatar(
                ui,
                pos2(gutter.center().x, gutter.top() + 18.0),
                18.0,
                &message.sender_name,
                &message.sender_id,
            );
        }
        ui.add_space(12.0);

        ui.vertical(|ui| {
            ui.set_max_width((full_width - 64.0 - 76.0).max(120.0));
            ui.spacing_mut().item_spacing.y = 3.0;
            if starts_group {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(&message.sender_name)
                            .font(semibold(14.0))
                            .color(color_for(&message.sender_id)),
                    );
                    ui.label(
                        RichText::new(clock(message.timestamp))
                            .small()
                            .color(pal.weak),
                    );
                });
            }
            if let Some(reply) = &message.reply {
                reply_quote(ui, reply, pal);
            }
            message_body(ui, message, pal, actions);
        });
    });

    let rect = inner.response.rect;
    let row = Rect::from_min_max(
        pos2(ui.max_rect().left() + 6.0, rect.top() - 2.0),
        pos2(ui.max_rect().right() - 6.0, rect.bottom() + 2.0),
    );
    if ui.rect_contains_pointer(row) {
        ui.painter()
            .set(background, Shape::rect_filled(row, 8, pal.hover));
        if !starts_group {
            ui.painter().text(
                pos2(rect.left() + 16.0 + 18.0, rect.top() + 10.0),
                Align2::CENTER_CENTER,
                clock(message.timestamp),
                FontId::proportional(10.5),
                pal.weak,
            );
        }
        if let Some(event_id) = &message.event_id {
            let button =
                Rect::from_min_size(pos2(row.right() - 66.0, row.top() + 3.0), vec2(58.0, 22.0));
            // A child Ui, so the button floats over the row instead of
            // moving the layout cursor (which made rows overlap).
            let mut overlay = ui.new_child(egui::UiBuilder::new().max_rect(button));
            let reply = overlay.add(egui::Button::new(RichText::new("Reply").small()));
            if reply.clicked() {
                actions.reply = Some(Reply {
                    event_id: event_id.clone(),
                    sender_name: message.sender_name.clone(),
                    body: message.body.clone(),
                });
            }
        }
    }
}

fn message_body(ui: &mut Ui, message: &MessageRow, pal: &Palette, actions: &mut TimelineActions) {
    let base = match message.state {
        SendState::Sending => pal.weak,
        _ => pal.text,
    };
    let text = match message.kind {
        BodyKind::Emote => RichText::new(format!("* {} {}", message.sender_name, message.body))
            .italics()
            .color(base),
        BodyKind::Notice => RichText::new(&message.body).color(pal.weak),
        BodyKind::Media | BodyKind::Placeholder => {
            RichText::new(&message.body).italics().color(pal.weak)
        }
        BodyKind::Text => RichText::new(&message.body).color(base),
    };
    if message.kind == BodyKind::Text && has_link(&message.body) {
        body_with_links(ui, &message.body, base);
    } else {
        ui.add(egui::Label::new(text).wrap());
    }

    let mut notes = Vec::new();
    if message.edited {
        notes.push(RichText::new("(edited)").small().color(pal.weak));
    }
    if message.state == SendState::Sending {
        notes.push(RichText::new("Sending…").small().color(pal.weak));
    }
    if !notes.is_empty() {
        ui.horizontal(|ui| {
            for note in notes {
                ui.label(note);
            }
        });
    }

    if message.state == SendState::Failed {
        let reason = message.error.as_deref().unwrap_or("Something went wrong.");
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(format!("Not sent. {reason}"))
                    .small()
                    .color(ui.visuals().error_fg_color),
            );
            if ui.small_button("Retry").clicked() {
                actions.retry = Some(message.key.clone());
            }
            if ui.small_button("Delete").clicked() {
                actions.delete = Some(message.key.clone());
            }
        });
    }
}

fn reply_quote(ui: &mut Ui, reply: &crate::timeline::ReplyPreview, pal: &Palette) {
    let frame = egui::Frame::new()
        .fill(pal.raised)
        .corner_radius(6)
        .inner_margin(Margin {
            left: 12,
            right: 10,
            top: 5,
            bottom: 5,
        })
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            if !reply.sender_name.is_empty() {
                ui.label(
                    RichText::new(&reply.sender_name)
                        .font(semibold(12.0))
                        .color(pal.text),
                );
            }
            ui.label(
                RichText::new(truncate(&reply.body, 120))
                    .small()
                    .color(pal.weak),
            );
        });
    let rect = frame.response.rect;
    ui.painter().rect_filled(
        Rect::from_min_size(rect.left_top(), vec2(3.0, rect.height())),
        egui::CornerRadius {
            nw: 6,
            sw: 6,
            ne: 0,
            se: 0,
        },
        ACCENT.gamma_multiply(0.8),
    );
}

fn day_divider(ui: &mut Ui, ts: i64, pal: &Palette) {
    ui.add_space(14.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
    let painter = ui.painter();
    let galley = painter.layout_no_wrap(day_label(ts), semibold(11.5), pal.weak);
    let text_rect = Rect::from_center_size(rect.center(), galley.size() + vec2(20.0, 0.0));
    let line = Stroke::new(1.0, pal.raised);
    painter.hline(rect.left() + 16.0..=text_rect.left(), rect.center().y, line);
    painter.hline(
        text_rect.right()..=rect.right() - 16.0,
        rect.center().y,
        line,
    );
    painter.galley(rect.center() - galley.size() / 2.0, galley, pal.weak);
}

fn read_marker(ui: &mut Ui) {
    ui.add_space(6.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 14.0), Sense::hover());
    let painter = ui.painter();
    let galley = painter.layout_no_wrap("New".into(), semibold(11.0), ACCENT);
    let label_left = rect.right() - 16.0 - galley.size().x;
    painter.hline(
        rect.left() + 16.0..=label_left - 8.0,
        rect.center().y,
        Stroke::new(1.0, ACCENT.gamma_multiply(0.7)),
    );
    painter.galley(
        pos2(label_left, rect.center().y - galley.size().y / 2.0),
        galley,
        ACCENT,
    );
}

/// True if `new` has messages above the first message of `old`: older
/// history was loaded rather than a new message arriving.
fn older_messages_added(old: &[TimelineRow], new: &[TimelineRow]) -> bool {
    let first_key = |rows: &[TimelineRow]| {
        rows.iter().find_map(|r| match r {
            TimelineRow::Message(m) => Some(m.key.clone()),
            _ => None,
        })
    };
    match (first_key(old), first_key(new)) {
        (Some(old_first), Some(new_first)) => {
            old_first != new_first
                && new
                    .iter()
                    .any(|r| matches!(r, TimelineRow::Message(m) if m.key == old_first))
        }
        _ => false,
    }
}

/// Message text with its web links clickable.
fn body_with_links(ui: &mut Ui, body: &str, color: Color32) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (text, is_link) in split_links(body) {
            if is_link {
                ui.hyperlink_to(RichText::new(text).color(ACCENT), text);
            } else {
                ui.label(RichText::new(text).color(color));
            }
        }
    });
}

fn has_link(text: &str) -> bool {
    text.contains("https://") || text.contains("http://")
}

/// Split text into plain runs and web links, in order.
fn split_links(text: &str) -> Vec<(&str, bool)> {
    let mut parts = Vec::new();
    let mut rest = text;
    while let Some(start) = [rest.find("https://"), rest.find("http://")]
        .into_iter()
        .flatten()
        .min()
    {
        if start > 0 {
            parts.push((&rest[..start], false));
        }
        let tail = &rest[start..];
        let mut end = tail.find(char::is_whitespace).unwrap_or(tail.len());
        // Leave trailing punctuation out of the link: "see https://x.org."
        while end > 0 && tail[..end].ends_with(['.', ',', ')', '!', '?', ':', ';', '"', '\'']) {
            end -= 1;
        }
        if end <= "https://".len() {
            parts.push((&tail[..end.max(1)], false));
            rest = &tail[end.max(1)..];
            continue;
        }
        parts.push((&tail[..end], true));
        rest = &tail[end..];
    }
    if !rest.is_empty() {
        parts.push((rest, false));
    }
    parts
}

// ---- Small helpers -------------------------------------------------------

/// The first letter or digit of a name, for its avatar.
fn initial(name: &str) -> String {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "#".into())
}

fn color_for(id: &str) -> Color32 {
    // FNV-1a: tiny, and stable across runs and platforms.
    let hash = id.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
    });
    PEOPLE_COLORS[(hash % PEOPLE_COLORS.len() as u64) as usize]
}

fn truncate(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or("");
    if line.chars().count() > max {
        let cut: String = line.chars().take(max).collect();
        format!("{}…", cut.trim_end())
    } else {
        line.to_owned()
    }
}

fn local(ms: i64) -> Option<chrono::DateTime<Local>> {
    Local.timestamp_millis_opt(ms).single()
}

/// "14:05"
fn clock(ms: i64) -> String {
    local(ms).map_or_else(String::new, |t| t.format("%H:%M").to_string())
}

/// For the room list: a time today, "Yesterday", a weekday this week, or
/// a date.
fn list_time(ms: i64) -> String {
    let Some(time) = local(ms) else {
        return String::new();
    };
    let today = Local::now().date_naive();
    let days = (today - time.date_naive()).num_days();
    match days {
        ..=0 => time.format("%H:%M").to_string(),
        1 => "Yesterday".into(),
        2..=6 => time.format("%a").to_string(),
        _ if time.year() == today.year() => time.format("%-d %b").to_string(),
        _ => time.format("%-d %b %Y").to_string(),
    }
}

/// For day dividers: "Today", "Yesterday", or "Wednesday 8 October".
fn day_label(ms: i64) -> String {
    let Some(time) = local(ms) else {
        return String::new();
    };
    let today = Local::now().date_naive();
    match (today - time.date_naive()).num_days() {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        _ if time.year() == today.year() => time.format("%A %-d %B").to_string(),
        _ => time.format("%A %-d %B %Y").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_skip_symbols() {
        assert_eq!(initial("Matrix HQ"), "M");
        assert_eq!(initial("#rust:matrix.org"), "R");
        assert_eq!(initial("ìgbò"), "Ì");
        assert_eq!(initial("!!!"), "#");
    }

    #[test]
    fn colors_are_stable() {
        assert_eq!(color_for("!a:b"), color_for("!a:b"));
    }

    #[test]
    fn truncation_adds_an_ellipsis() {
        assert_eq!(truncate("hello world", 5), "hello…");
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("two\nlines", 10), "two");
    }

    #[test]
    fn links_are_split_out() {
        assert_eq!(
            split_links("see https://ngwa.chat. thanks"),
            vec![
                ("see ", false),
                ("https://ngwa.chat", true),
                (". thanks", false)
            ]
        );
        assert_eq!(split_links("no links"), vec![("no links", false)]);
        assert_eq!(
            split_links("http://a.b/c?d=1"),
            vec![("http://a.b/c?d=1", true)]
        );
    }

    #[test]
    fn detects_older_history() {
        let msg = |key: &str| {
            TimelineRow::Message(MessageRow {
                key: key.into(),
                event_id: None,
                sender_id: String::new(),
                sender_name: String::new(),
                is_own: false,
                body: String::new(),
                kind: BodyKind::Text,
                timestamp: 0,
                reply: None,
                edited: false,
                state: SendState::Sent,
                error: None,
            })
        };
        let old = vec![msg("b"), msg("c")];
        assert!(older_messages_added(&old, &[msg("a"), msg("b"), msg("c")]));
        assert!(!older_messages_added(&old, &[msg("b"), msg("c"), msg("d")]));
        assert!(!older_messages_added(&[], &[msg("a")]));
    }
}
