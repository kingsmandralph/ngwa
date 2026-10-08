//! The desktop window: sign-in, and a live room list.

use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use eframe::egui::{
    self, Align, Align2, Color32, FontId, Key, Layout, Margin, RichText, Sense, TextEdit, Ui, pos2,
    text::{LayoutJob, TextFormat, TextWrapping},
    vec2,
};

use crate::backend::{Backend, Command, Event, SyncStatus};
use crate::matrix::RoomRow;

/// Ngwa's accent: a warm orange.
const ACCENT: Color32 = Color32::from_rgb(232, 116, 59);
const ROW_HEIGHT: f32 = 52.0;
const AVATAR_RADIUS: f32 = 18.0;
/// Room avatar colours, picked by a hash of the room ID so each room keeps
/// its colour between runs.
const AVATAR_COLORS: [Color32; 8] = [
    Color32::from_rgb(76, 110, 245),
    Color32::from_rgb(18, 150, 120),
    Color32::from_rgb(196, 72, 120),
    Color32::from_rgb(214, 140, 30),
    Color32::from_rgb(120, 86, 220),
    Color32::from_rgb(36, 140, 190),
    Color32::from_rgb(200, 80, 60),
    Color32::from_rgb(90, 130, 60),
];

/// Open the window and run until it is closed.
pub fn run(demo: bool, started: Instant) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("ngwa-sync")
        .build()?;
    let handle = runtime.handle().clone();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Ngwa")
            .with_app_id("chat.ngwa.Ngwa")
            .with_inner_size([400.0, 680.0])
            .with_min_inner_size([320.0, 420.0]),
        ..Default::default()
    };
    let result = eframe::run_native(
        "Ngwa",
        options,
        Box::new(move |cc| {
            setup_style(&cc.egui_ctx);
            let backend = Backend::start(&handle, cc.egui_ctx.clone(), demo);
            Ok(Box::new(App::new(backend, started)))
        }),
    );

    // The window is gone; give the engine a moment to stop syncing cleanly.
    runtime.shutdown_timeout(Duration::from_secs(2));
    result.map_err(|e| anyhow!("could not open the window: {e}"))
}

fn setup_style(ctx: &egui::Context) {
    use egui::TextStyle::{Body, Button, Heading, Monospace, Small};
    ctx.all_styles_mut(|style| {
        style.text_styles.insert(Body, FontId::proportional(14.0));
        style.text_styles.insert(Button, FontId::proportional(14.0));
        style
            .text_styles
            .insert(Heading, FontId::proportional(22.0));
        style.text_styles.insert(Small, FontId::proportional(11.5));
        style.text_styles.insert(Monospace, FontId::monospace(13.0));
        style.spacing.item_spacing = vec2(8.0, 8.0);
        style.spacing.button_padding = vec2(12.0, 6.0);
        style.visuals.selection.bg_fill = ACCENT.gamma_multiply(0.45);
    });
}

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
    selected: Option<String>,
    confirm_sign_out_until: Option<Instant>,

    /// When to measure "rooms shown in" from: launch, or pressing Sign in.
    measure_from: Instant,
    /// Time from `measure_from` until the first rooms were on screen.
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
            selected: None,
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
                    self.selected = None;
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
                }
                Event::ListLoaded => self.list_loaded = true,
                Event::Sync(status) => self.sync = status,
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

    /// Show the total unread count in the window title, like other chat apps.
    fn update_title(&mut self, ctx: &egui::Context) {
        let unread: u64 = self
            .rooms
            .iter()
            .filter(|r| r.unread > 0)
            .map(|r| r.unread)
            .sum();
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

    // ---- Screens -------------------------------------------------------

    fn starting_ui(&mut self, ui: &mut Ui) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.centered_and_justified(|ui| {
                ui.spinner();
            });
        });
    }

    fn sign_in_ui(&mut self, ui: &mut Ui) {
        let signing_in = self.phase == Phase::SigningIn;
        let mut submit = false;

        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space((ui.available_height() * 0.12).max(32.0));
                    ui.label(RichText::new("Ngwa").size(36.0).strong().color(ACCENT));
                    ui.label(RichText::new("A fast, native Matrix client").weak());
                    ui.add_space(20.0);

                    let width = 300.0_f32.min(ui.available_width() - 16.0);
                    egui::Frame::group(ui.style())
                        .inner_margin(Margin::same(18))
                        .corner_radius(10)
                        .show(ui, |ui| {
                            ui.set_width(width);
                            ui.with_layout(Layout::top_down(Align::LEFT), |ui| {
                                ui.spacing_mut().item_spacing.y = 6.0;
                                ui.label("Matrix ID");
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
                                    ui.add_space(4.0);
                                    ui.label("Homeserver");
                                    ui.add_enabled(
                                        !signing_in,
                                        field(&mut self.form.homeserver).hint_text("matrix.org"),
                                    );
                                }

                                ui.add_space(4.0);
                                ui.label("Password");
                                ui.add_enabled(
                                    !signing_in,
                                    field(&mut self.form.password).password(true),
                                );

                                if let Some(error) = &self.form.error {
                                    ui.add_space(4.0);
                                    ui.label(RichText::new(error).color(error_color(ui)));
                                }

                                ui.add_space(8.0);
                                let ready = !self.form.id.trim().is_empty()
                                    && !self.form.password.is_empty();
                                let label = if signing_in {
                                    "Signing in…"
                                } else {
                                    "Sign in"
                                };
                                let button = egui::Button::new(
                                    RichText::new(label).color(Color32::WHITE).strong(),
                                )
                                .fill(ACCENT);
                                let size = vec2(ui.available_width(), 34.0);
                                let clicked = ui
                                    .add_enabled_ui(ready && !signing_in, |ui| {
                                        ui.add_sized(size, button).clicked()
                                    })
                                    .inner;
                                if clicked {
                                    submit = true;
                                }
                                if signing_in {
                                    ui.vertical_centered(|ui| ui.spinner());
                                }
                            });
                        });

                    ui.add_space(12.0);
                    ui.label(
                        RichText::new("Your password goes only to your homeserver.")
                            .small()
                            .weak(),
                    );
                });
            });
        });

        let enter = ui.input(|i| i.key_pressed(Key::Enter));
        if (submit || enter) && !self.form.id.trim().is_empty() && !self.form.password.is_empty() {
            self.submit_sign_in();
        }
    }

    fn rooms_ui(&mut self, ui: &mut Ui) {
        self.header(ui);
        self.footer(ui);

        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(ui.style()).inner_margin(Margin::symmetric(6, 8)))
            .show(ui, |ui| {
                if let Some(error) = self.error.clone() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(error).color(error_color(ui)));
                        if ui.small_button("Dismiss").clicked() {
                            self.error = None;
                        }
                    });
                }

                let search = ui.add(
                    TextEdit::singleline(&mut self.search)
                        .hint_text("Search rooms  (Ctrl+K)")
                        .desired_width(f32::INFINITY)
                        .margin(Margin::symmetric(10, 7)),
                );
                if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::K)) {
                    search.request_focus();
                }
                if search.has_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
                    self.search.clear();
                }
                ui.add_space(2.0);

                let query = self.search.trim().to_lowercase();
                let visible: Vec<usize> = self
                    .rooms
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| query.is_empty() || r.name.to_lowercase().contains(&query))
                    .map(|(i, _)| i)
                    .collect();

                if visible.is_empty() {
                    empty_state(ui, self.list_loaded, &self.rooms, &self.search);
                    return;
                }

                egui::ScrollArea::vertical().auto_shrink(false).show_rows(
                    ui,
                    ROW_HEIGHT,
                    visible.len(),
                    |ui, range| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for &index in &visible[range] {
                            let room = &self.rooms[index];
                            let selected = self.selected.as_deref() == Some(room.id.as_str());
                            if room_row(ui, room, selected).clicked() {
                                self.selected = Some(room.id.clone());
                            }
                        }
                    },
                );
            });
    }

    fn header(&mut self, ui: &mut Ui) {
        egui::Panel::top("header")
            .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(Margin::symmetric(14, 10)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Ngwa").size(18.0).strong().color(ACCENT));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let confirming = self
                            .confirm_sign_out_until
                            .is_some_and(|until| Instant::now() < until);
                        let label = if confirming {
                            "Confirm sign out"
                        } else {
                            "Sign out"
                        };
                        let button = ui
                            .button(label)
                            .on_hover_text(format!("Signed in as {}", self.user_id));
                        if button.clicked() {
                            if confirming {
                                self.confirm_sign_out_until = None;
                                self.backend.send(Command::Logout);
                            } else {
                                self.confirm_sign_out_until =
                                    Some(Instant::now() + Duration::from_secs(4));
                                ui.ctx().request_repaint_after(Duration::from_secs(4));
                            }
                        }
                        sync_indicator(ui, &self.sync);
                    });
                });
            });
    }

    fn footer(&mut self, ui: &mut Ui) {
        egui::Panel::bottom("footer")
            .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(Margin::symmetric(14, 6)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&self.user_id).small().weak());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let mut parts = vec![format!("{} rooms", self.rooms.len())];
                        if let Some(after) = self.rooms_shown_after {
                            parts.push(format!("shown in {} ms", after.as_millis()));
                        }
                        if let Some(mb) = self.memory_mb {
                            parts.push(format!("{mb} MB"));
                        }
                        ui.label(RichText::new(parts.join(" · ")).small().weak())
                            .on_hover_text(
                                "Rooms in your list · time from launch until they were on \
                                 screen · memory Ngwa is using",
                            );
                    });
                });
            });
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
                self.rooms_ui(ui);
            }
        }
    }
}

// ---- Pieces --------------------------------------------------------------

/// A full-width text field for the sign-in form.
fn field(text: &mut String) -> TextEdit<'_> {
    TextEdit::singleline(text)
        .desired_width(f32::INFINITY)
        .margin(Margin::symmetric(8, 6))
}

fn error_color(ui: &Ui) -> Color32 {
    ui.visuals().error_fg_color
}

fn sync_indicator(ui: &mut Ui, status: &SyncStatus) {
    let (color, text, detail) = match status {
        SyncStatus::Connecting => (Color32::from_rgb(214, 160, 40), "Connecting", None),
        SyncStatus::Live => (Color32::from_rgb(40, 170, 100), "Live", None),
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
    // Laid out right to left: text first, then the dot to its left.
    let label = ui.label(RichText::new(text).small().weak());
    let (dot, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
    ui.painter().circle_filled(dot.center(), 4.0, color);
    if let Some(detail) = detail {
        label.on_hover_text(detail);
    }
}

fn empty_state(ui: &mut Ui, loaded: bool, rooms: &[RoomRow], search: &str) {
    ui.add_space(40.0);
    ui.vertical_centered(|ui| {
        if !rooms.is_empty() {
            ui.label(RichText::new(format!("No rooms match “{}”", search.trim())).weak());
        } else if !loaded {
            ui.spinner();
            ui.label(RichText::new("Loading your rooms…").weak());
        } else {
            ui.label(RichText::new("No rooms yet").strong());
            ui.label(RichText::new("Rooms you join will show up here.").weak());
        }
    });
}

/// One room in the list. Painted by hand so that thousands of rooms stay
/// cheap: only the rows on screen are ever drawn.
fn room_row(ui: &mut Ui, room: &RoomRow, selected: bool) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let visuals = ui.visuals().clone();
    let painter = ui.painter();

    let background = if selected {
        Some(ACCENT.gamma_multiply(0.22))
    } else if response.hovered() {
        Some(visuals.widgets.hovered.weak_bg_fill)
    } else {
        None
    };
    if let Some(fill) = background {
        painter.rect_filled(rect.shrink2(vec2(2.0, 2.0)), 8, fill);
    }

    // Avatar: a coloured circle with the room's initial.
    let avatar = pos2(rect.left() + 12.0 + AVATAR_RADIUS, rect.center().y);
    painter.circle_filled(avatar, AVATAR_RADIUS, avatar_color(&room.id));
    painter.text(
        avatar,
        Align2::CENTER_CENTER,
        initial(&room.name),
        FontId::proportional(16.0),
        Color32::WHITE,
    );

    // Badge on the right: invite, mentions (accent) or unread (neutral).
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
            Some((
                text,
                visuals.widgets.inactive.bg_fill,
                visuals.strong_text_color(),
            ))
        }
    } else {
        None
    };
    if let Some((text, fill, text_color)) = badge {
        let galley = painter.layout_no_wrap(text, FontId::proportional(12.0), text_color);
        let size = vec2((galley.size().x + 14.0).max(22.0), 20.0);
        let pill =
            egui::Rect::from_min_size(pos2(right - size.x, rect.center().y - size.y / 2.0), size);
        painter.rect_filled(pill, 10, fill);
        painter.galley(pill.center() - galley.size() / 2.0, galley, text_color);
        right = pill.left() - 8.0;
    }

    // Name, cut short with an ellipsis if it doesn't fit.
    let left = avatar.x + AVATAR_RADIUS + 12.0;
    let color = if room.unread > 0 || room.is_invite {
        visuals.strong_text_color()
    } else {
        visuals.text_color()
    };
    let mut job = LayoutJob::single_section(
        room.name.clone(),
        TextFormat::simple(FontId::proportional(15.0), color),
    );
    job.wrap = TextWrapping::truncate_at_width((right - left).max(20.0));
    let galley = painter.layout_job(job);
    painter.galley(
        pos2(left, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );

    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The first letter or digit of a room's name, for its avatar.
fn initial(name: &str) -> String {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "#".into())
}

fn avatar_color(room_id: &str) -> Color32 {
    // FNV-1a: tiny, and stable across runs and platforms.
    let hash = room_id.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
    });
    AVATAR_COLORS[(hash % AVATAR_COLORS.len() as u64) as usize]
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
    fn avatar_colors_are_stable() {
        assert_eq!(avatar_color("!a:b"), avatar_color("!a:b"));
    }
}
