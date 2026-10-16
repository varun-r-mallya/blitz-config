//! egui front end. Holds no protocol knowledge: every control calls a `Profile` setter,
//! and edits reach the controller through the device worker, debounced like the
//! reference app (220 ms after the last change, one write in flight at a time).

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};

use crate::device::{self, Cmd, Discovery, Event, Handle as Device, WriteResult};
use crate::profile::{
    self, Button, CurvePoint, GYRO_BUTTONS, GyroMethod, GyroOutput, Handle, MAP_GROUPS, MAP_TARGETS, Profile, Range,
    SLEEP_MINUTES, Side,
};
use crate::protocol::{self as p, Gamepad};

const DEBOUNCE: Duration = Duration::from_millis(220);
const ACCENT: Color32 = Color32::from_rgb(0xFF, 0x9E, 0x1B);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Mappings,
    Sticks,
    Triggers,
    Vibration,
    Motion,
    Extended,
}

impl Section {
    const ALL: [Section; 6] =
        [Section::Mappings, Section::Sticks, Section::Triggers, Section::Vibration, Section::Motion, Section::Extended];

    fn label(self) -> &'static str {
        match self {
            Section::Mappings => "Mappings",
            Section::Sticks => "Sticks",
            Section::Triggers => "Triggers",
            Section::Vibration => "Vibration",
            Section::Motion => "Motion",
            Section::Extended => "Extended",
        }
    }
}

struct Session {
    dev: Device,
    transport: &'static str,
    firmware: Option<String>,
    profile: u8,
    /// Profile being edited, and what the controller holds. `None` while loading.
    buf: Option<Profile>,
    base: Option<Profile>,
    dirty_since: Option<Instant>,
    write_in_flight: bool,
    saved_toast: bool,
    name_edit: String,
    last_backup: Option<PathBuf>,
    error: Option<String>,
}

impl Session {
    fn pending(&self) -> bool {
        matches!((&self.buf, &self.base), (Some(b), Some(s)) if b != s)
    }

    /// Queue a write of everything not yet on the controller.
    fn flush(&mut self) {
        self.dirty_since = None;
        if self.write_in_flight || !self.pending() {
            return;
        }
        let (Some(b), Some(s)) = (&self.buf, &self.base) else { return };
        self.dev.send(Cmd::Write { profile: self.profile, next: Box::new(b.0), base: Box::new(s.0) });
        self.write_in_flight = true;
    }

    fn reload(&mut self, profile: u8) {
        self.dirty_since = None;
        self.buf = None;
        self.base = None;
        self.profile = profile;
        self.dev.send(Cmd::ReadProfile(profile));
    }
}

pub struct App {
    dry_run: bool,
    session: Option<Session>,
    connect_msg: Option<String>,
    section: Section,
    stick: Side,
    trigger: Side,
    grip_test: bool,
    confirm_reset: bool,
    toast: Option<(String, Instant)>,
}

impl App {
    pub fn new(dry_run: bool) -> Self {
        Self {
            dry_run,
            session: None,
            connect_msg: None,
            section: Section::Mappings,
            stick: Side::Left,
            trigger: Side::Left,
            grip_test: false,
            confirm_reset: false,
            toast: None,
        }
    }

    fn toast(&mut self, s: impl Into<String>) {
        self.toast = Some((s.into(), Instant::now()));
    }

    fn connect(&mut self, ctx: &egui::Context) {
        self.connect_msg = None;
        let cand = match device::discover() {
            Ok(Discovery::Found(c)) => c,
            Ok(Discovery::NoConfigChannel) => {
                self.connect_msg = Some(
                    "The Blitz is connected but doesn't expose its config channel. It is probably in XInput mode \
                     (\"Xbox 360 controller\") or on Bluetooth. Connect over wired USB or the 2.4 GHz dongle in \
                     DInput mode, then try again."
                        .into(),
                );
                return;
            }
            Ok(Discovery::CannotOpen(e)) => {
                self.connect_msg = Some(format!(
                    "Found the Blitz but couldn't open it ({e}). On Linux, install the udev rule from the README \
                     and replug the controller. Also close any browser tab that is using it."
                ));
                return;
            }
            Ok(Discovery::NotFound) => {
                self.connect_msg = Some(
                    "No Blitz found. Plug it in over USB (or plug in its 2.4 GHz dongle). On Linux, install the udev \
                     rule from the README if the controller is attached but not listed."
                        .into(),
                );
                return;
            }
            Err(e) => {
                self.connect_msg = Some(e);
                return;
            }
        };
        let transport = cand.transport;
        let repaint = ctx.clone();
        match device::connect(cand, self.dry_run, move || repaint.request_repaint()) {
            Ok(dev) => {
                self.grip_test = false;
                self.session = Some(Session {
                    dev,
                    transport,
                    firmware: None,
                    profile: 0,
                    buf: None,
                    base: None,
                    dirty_since: None,
                    write_in_flight: false,
                    saved_toast: false,
                    name_edit: String::new(),
                    last_backup: None,
                    error: None,
                });
            }
            Err(e) => self.connect_msg = Some(e),
        }
    }

    fn disconnect(&mut self) {
        if let Some(mut s) = self.session.take() {
            s.flush(); // queued ahead of the disconnect, so pending edits still land
            s.dev.grip_test.store(false, Ordering::Relaxed);
        }
    }

    fn poll(&mut self) {
        let Some(s) = &mut self.session else { return };
        let mut toast = None;
        let mut ended = None;
        while let Ok(ev) = s.dev.events.try_recv() {
            match ev {
                Event::Connected { firmware } => s.firmware = firmware,
                Event::ProfileLoaded { profile, data, backup } => {
                    if profile == s.profile {
                        let pr = Profile(*data);
                        s.name_edit = pr.name();
                        s.buf = Some(pr.clone());
                        s.base = Some(pr);
                        s.error = None;
                    }
                    s.last_backup = backup;
                }
                Event::Written { profile, result } => {
                    s.write_in_flight = false;
                    if profile != s.profile || s.base.is_none() {
                        continue; // written just before a profile switch; nothing to update
                    }
                    match result {
                        WriteResult::Verified(data) => {
                            s.base = Some(Profile(*data));
                            if s.saved_toast {
                                s.saved_toast = false;
                                toast = Some("Saved to controller");
                            }
                            if s.pending() && s.dirty_since.is_none() {
                                s.dirty_since = Some(Instant::now() - DEBOUNCE); // edited during the write
                            }
                        }
                        WriteResult::Mismatch { actual, detail } => {
                            let pr = Profile(*actual);
                            s.name_edit = pr.name();
                            s.buf = Some(pr.clone());
                            s.base = Some(pr);
                            s.dirty_since = None;
                            s.error = Some(detail);
                        }
                        WriteResult::Unverified(e) => {
                            s.error = Some(e);
                            s.reload(s.profile);
                        }
                    }
                }
                Event::Error(e) => s.error = Some(e),
                Event::Disconnected(e) => ended = Some(e),
            }
        }
        if let Some(t) = toast {
            self.toast(t);
        }
        if let Some(e) = ended {
            self.session = None;
            self.connect_msg = e.map(|e| format!("Disconnected: {e}"));
        }
    }

    fn auto_apply(&mut self, ctx: &egui::Context) {
        let Some(s) = &mut self.session else { return };
        if let Some(t) = s.dirty_since {
            let left = DEBOUNCE.saturating_sub(t.elapsed());
            if left.is_zero() {
                if !s.write_in_flight {
                    s.flush();
                }
            } else {
                ctx.request_repaint_after(left);
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll();
        self.auto_apply(&ctx);

        if self.session.is_none() {
            egui::CentralPanel::default().show(ui, |ui| self.connect_screen(ui, &ctx));
            return;
        }

        egui::Panel::top("header").show(ui, |ui| self.header(ui));
        egui::Panel::left("nav").exact_size(150.0).resizable(false).show(ui, |ui| self.nav(ui));
        egui::Panel::right("live").exact_size(300.0).resizable(false).show(ui, |ui| self.live(ui));
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.section_ui(ui));
        });

        self.reset_dialog(&ctx);
        if let Some((msg, t)) = &self.toast {
            if t.elapsed() < Duration::from_secs(3) {
                egui::Area::new(egui::Id::unique("toast")).anchor(Align2::CENTER_BOTTOM, [0.0, -24.0]).show(&ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| ui.label(RichText::new(msg.as_str()).strong()));
                });
                ctx.request_repaint_after(Duration::from_millis(250));
            } else {
                self.toast = None;
            }
        }
        if self.session.as_ref().is_some_and(|s| s.pending() || s.write_in_flight) {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.disconnect();
    }
}

// ---------------------------------------------------------------- screens

impl App {
    fn connect_screen(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.25);
            ui.heading(RichText::new("Cosmic Byte Blitz").size(28.0).strong());
            ui.label("Native configurator");
            if self.dry_run {
                ui.add_space(6.0);
                ui.label(RichText::new("DRY RUN: settings changes are logged, never sent").color(ACCENT).strong());
            }
            ui.add_space(18.0);
            if ui.add(egui::Button::new(RichText::new("Connect controller").size(16.0)).min_size([200.0, 36.0].into())).clicked() {
                self.connect(ctx);
            }
            if let Some(m) = &self.connect_msg {
                ui.add_space(12.0);
                ui.set_max_width(520.0);
                ui.label(RichText::new(m.as_str()).color(Color32::from_rgb(0xE5, 0x9B, 0x4D)));
            }
            ui.add_space(18.0);
            ui.small(format!("Backups and HID log: {}", device::data_dir().display()));
        });
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        let dry = self.dry_run;
        let Some(s) = &mut self.session else { return };
        let mut action: Option<&str> = None;
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Blitz").size(18.0).strong());
            ui.separator();
            ui.label(s.transport);
            ui.label(format!("FW {}", s.firmware.as_deref().unwrap_or("—")));
            if dry {
                ui.label(RichText::new("DRY RUN").color(ACCENT).strong());
            }
            ui.separator();
            ui.label("Profile");
            for i in 0..p::PROFILE_COUNT {
                if ui.selectable_label(s.profile == i, format!(" {} ", i + 1)).clicked() && s.profile != i {
                    s.flush(); // pending edits belong to the profile they were made on
                    s.dev.send(Cmd::SwitchProfile(i));
                    s.reload(i);
                }
            }
            if let Some(buf) = &mut s.buf {
                let r = ui.add(egui::TextEdit::singleline(&mut s.name_edit).hint_text("Name this profile").char_limit(24).desired_width(170.0));
                if r.lost_focus() && s.name_edit != buf.name() {
                    buf.set_name(&s.name_edit);
                    s.name_edit = buf.name();
                    s.dirty_since = Some(Instant::now());
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Disconnect").clicked() {
                    action = Some("disconnect");
                }
                let loaded = s.buf.is_some();
                if ui.add_enabled(loaded, egui::Button::new("Reset to default")).clicked() {
                    action = Some("reset");
                }
                if ui.add_enabled(loaded, egui::Button::new("Refresh")).on_hover_text("Re-read this profile from the controller, discarding unsaved edits").clicked() {
                    s.reload(s.profile);
                    action = Some("refreshed");
                }
                if ui.add_enabled(loaded, egui::Button::new("Save to controller")).clicked() {
                    if s.pending() {
                        s.saved_toast = true;
                        s.flush();
                    } else {
                        action = Some("saved");
                    }
                }
                let status = if s.buf.is_none() {
                    "Reading…"
                } else if s.write_in_flight {
                    "Writing…"
                } else if s.pending() {
                    "Applying…"
                } else {
                    "In sync"
                };
                ui.label(RichText::new(status).weak());
            });
        });
        if let Some(e) = &s.error {
            ui.colored_label(Color32::from_rgb(0xE5, 0x48, 0x4D), e);
        }
        ui.add_space(4.0);
        match action {
            Some("disconnect") => self.disconnect(),
            Some("reset") => self.confirm_reset = true,
            Some("refreshed") => self.toast("Refreshed from controller"),
            Some("saved") => self.toast("Already saved"),
            _ => {}
        }
    }

    fn reset_dialog(&mut self, ctx: &egui::Context) {
        if !self.confirm_reset {
            return;
        }
        let mut open = true;
        let mut decided = None;
        egui::Window::new("Reset to default?").collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).open(&mut open).show(ctx, |ui| {
            ui.label("Restores factory mappings, sticks, triggers, vibration, motion and lighting on this profile.");
            ui.label("The profile name, wake and sleep settings are kept.");
            ui.horizontal(|ui| {
                if ui.button("Reset").clicked() {
                    decided = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    decided = Some(false);
                }
            });
        });
        if decided == Some(true)
            && let Some(s) = &mut self.session
            && let Some(b) = &mut s.buf
        {
            b.reset_to_defaults();
            s.dirty_since = Some(Instant::now());
        }
        if decided.is_some() || !open {
            self.confirm_reset = false;
        }
    }

    fn nav(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        for sec in Section::ALL {
            let label = RichText::new(sec.label()).size(15.0);
            if ui.add_sized([ui.available_width(), 30.0], egui::Button::selectable(self.section == sec, label)).clicked() {
                self.section = sec;
            }
        }
        if let Some(s) = &self.session
            && let Some(b) = &s.last_backup
        {
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                ui.small(b.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default())
                    .on_hover_text(b.display().to_string());
                ui.small("Last backup");
            });
        }
    }

    fn section_ui(&mut self, ui: &mut egui::Ui) {
        let Some(s) = &mut self.session else { return };
        let Some(buf) = &mut s.buf else {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                if s.error.is_some() {
                    if ui.button("Retry reading profile").clicked() {
                        s.reload(s.profile);
                    }
                } else {
                    ui.spinner();
                    ui.label("Reading profile from the controller…");
                }
            });
            return;
        };
        let before = buf.clone();
        let mut ctl = Controls { ui_rumble: Vec::new() };
        ui.add_space(8.0);
        match self.section {
            Section::Mappings => mappings(ui, buf),
            Section::Sticks => sticks(ui, buf, &mut self.stick),
            Section::Triggers => triggers(ui, buf, &mut self.trigger),
            Section::Vibration => {
                vibration(ui, buf, &mut ctl, &mut self.grip_test, &s.dev);
            }
            Section::Motion => {
                if motion(ui, buf) {
                    s.dev.send(Cmd::CalibrateGyro);
                    self.toast = Some(("Calibration started: keep the controller still".into(), Instant::now()));
                }
            }
            Section::Extended => extended(ui, buf, s.firmware.as_deref()),
        }
        for (l, r, ms) in ctl.ui_rumble {
            s.dev.send(Cmd::Rumble { left: l, right: r, ms });
        }
        if *buf != before {
            s.dirty_since = Some(Instant::now());
        }
    }

    fn live(&mut self, ui: &mut egui::Ui) {
        let Some(s) = &self.session else { return };
        let g = s.dev.live.lock().unwrap().gamepad;
        ui.add_space(8.0);
        ui.label(RichText::new("Live input").strong());
        ui.add_space(4.0);
        draw_gamepad(ui, g.unwrap_or_default(), g.is_some());
    }
}

struct Controls {
    ui_rumble: Vec<(u8, u8, u64)>,
}

// ---------------------------------------------------------------- widgets

fn card(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::group(ui.style()).inner_margin(12.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(title).strong().size(15.0));
        ui.add_space(6.0);
        add(ui);
    });
    ui.add_space(10.0);
}

fn heading(ui: &mut egui::Ui, title: &str, sub: &str) {
    ui.heading(RichText::new(title).size(24.0).strong());
    ui.label(RichText::new(sub).weak());
    ui.add_space(10.0);
}

/// Segmented buttons. Returns the clicked value even if it is already selected:
/// like the reference app, clicking a choice always (re)writes it.
fn segmented<T: Copy + PartialEq>(ui: &mut egui::Ui, current: T, options: &[(T, &str)]) -> Option<T> {
    let mut clicked = None;
    ui.horizontal_wrapped(|ui| {
        for &(v, label) in options {
            if ui.selectable_label(current == v, label).clicked() {
                clicked = Some(v);
            }
        }
    });
    clicked
}

fn side_picker(ui: &mut egui::Ui, side: &mut Side, left: &str, right: &str) {
    ui.horizontal(|ui| {
        ui.selectable_value(side, Side::Left, left);
        ui.selectable_value(side, Side::Right, right);
    });
    ui.add_space(6.0);
}

/// Two-handle 0..=100 slider. Reports (handle, percent) while a handle is dragged.
fn dual_range(ui: &mut egui::Ui, r: Range, id: &str) -> Option<(Handle, i32)> {
    let width = ui.available_width().min(520.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 34.0), Sense::drag());
    let track = Rect::from_min_max(Pos2::new(rect.left() + 9.0, rect.center().y - 3.0), Pos2::new(rect.right() - 9.0, rect.center().y + 3.0));
    let x_of = |pct: i32| track.left() + track.width() * (pct.clamp(0, 100) as f32 / 100.0);
    let painter = ui.painter_at(rect);
    let visuals = ui.visuals();
    painter.rect_filled(track, 3.0, visuals.widgets.inactive.bg_fill);
    painter.rect_filled(Rect::from_min_max(Pos2::new(x_of(r.initial), track.top()), Pos2::new(x_of(r.max), track.bottom())), 3.0, ACCENT);
    for pct in [r.initial, r.max] {
        painter.circle(Pos2::new(x_of(pct), track.center().y), 8.0, visuals.widgets.active.bg_fill, Stroke::new(2.0, ACCENT));
    }

    let key = egui::Id::unique(("dual_range", id));
    if resp.drag_started()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let d0 = (pos.x - x_of(r.initial)).abs();
        let d1 = (pos.x - x_of(r.max)).abs();
        let grabbed = if d0.min(d1) > 14.0 {
            None
        } else if d1 < d0 || (d0 == d1 && pos.x > x_of(r.initial)) {
            Some(Handle::Max)
        } else {
            Some(Handle::Initial)
        };
        ui.data_mut(|d| d.insert_temp(key, grabbed));
    }
    let grabbed: Option<Handle> = ui.data(|d| d.get_temp(key)).flatten();
    if resp.drag_stopped() {
        ui.data_mut(|d| d.remove::<Option<Handle>>(key));
    }
    let row = ui.horizontal(|ui| {
        ui.small("Initial");
        ui.add_space(width - 80.0);
        ui.small("Max");
    });
    drop(row);
    if resp.dragged()
        && resp.drag_delta() != Vec2::ZERO
        && let (Some(h), Some(pos)) = (grabbed, resp.interact_pointer_pos())
    {
        let pct = profile::js_round(((pos.x - track.left()) / track.width() * 100.0) as f64) as i32;
        return Some((h, pct.clamp(0, 100)));
    }
    None
}

fn range_label(r: Range) -> String {
    format!("{}% – {}%", r.initial, r.max)
}

// ---------------------------------------------------------------- sections

fn mappings(ui: &mut egui::Ui, buf: &mut Profile) {
    heading(ui, "Mappings", "Remap a button to another gamepad button. Default keeps its normal function; Off disables it.");
    ui.label(
        RichText::new(
            "If a button has a macro or turbo set with the controller's own button combos, clear that on the \
             controller first; it takes priority over remaps.",
        )
        .weak()
        .small(),
    );
    ui.add_space(8.0);
    for (group, buttons) in MAP_GROUPS {
        card(ui, group, |ui| {
            egui::Grid::new(group).num_columns(2).spacing([18.0, 8.0]).show(ui, |ui| {
                for &src in buttons {
                    ui.label(RichText::new(src.label()).monospace().strong());
                    map_picker(ui, buf, src);
                    ui.end_row();
                }
            });
        });
    }
}

fn map_picker(ui: &mut egui::Ui, buf: &mut Profile, src: Button) {
    let cur = buf.mapping(src);
    let shown = match cur {
        profile::Mapping::Default => "—".to_string(),
        m => format!("→ {}", m.label()),
    };
    egui::ComboBox::from_id_salt(("map", src.code())).selected_text(shown).width(130.0).show_ui(ui, |ui| {
        for t in MAP_TARGETS {
            if ui.selectable_label(cur == t, t.label()).clicked() {
                buf.set_mapping(src, t);
            }
        }
    });
}

fn sticks(ui: &mut egui::Ui, buf: &mut Profile, side: &mut Side) {
    heading(ui, "Sticks", "Per-stick deadzone and saturation, output shape, and D-pad options.");
    side_picker(ui, side, "Left Stick", "Right Stick");
    let st = buf.stick(*side);
    card(ui, &format!("Range   {}", range_label(st.range)), |ui| {
        if let Some((h, pct)) = dual_range(ui, st.range, "stick") {
            buf.drag_stick(*side, h, pct);
        }
        ui.small("Initial is the inner deadzone, Max is where output saturates. An initial value under 10% makes the stick very sensitive and prone to drift.");
    });
    card(ui, "Radial trace", |ui| {
        let mut on = !st.raw;
        if ui.checkbox(&mut on, "Enable radial trace").changed() {
            buf.set_radial_trace(*side, on);
        }
        ui.small("On: output traces a circle. Off: square (raw) output.");
    });
    card(ui, "D-pad", |ui| {
        let mut swap = buf.swap_stick_dpad();
        if ui.checkbox(&mut swap, "Swap left stick and D-pad").changed() {
            buf.set_swap_stick_dpad(swap);
        }
        let mut diag = buf.diagonal_lock();
        if ui.checkbox(&mut diag, "D-pad diagonal lock").changed() {
            buf.set_diagonal_lock(diag);
        }
    });
}

fn triggers(ui: &mut egui::Ui, buf: &mut Profile, side: &mut Side) {
    heading(ui, "Triggers", "Independent press threshold and full-pull point for LT and RT.");
    side_picker(ui, side, "Left Trigger", "Right Trigger");
    let r = buf.trigger(*side);
    card(ui, &format!("Range   {}", range_label(r)), |ui| {
        if let Some((h, pct)) = dual_range(ui, r, "trigger") {
            buf.drag_trigger(*side, h, pct);
        }
        ui.small("Initial is where a press registers, Max is where it reads as fully pulled.");
    });
}

fn vib_pwm(level_pct: u8) -> u8 {
    profile::js_round(level_pct as f64 * 255.0 / 100.0).clamp(0.0, 255.0) as u8
}

fn vibration(ui: &mut egui::Ui, buf: &mut Profile, ctl: &mut Controls, grip: &mut bool, dev: &Device) {
    heading(ui, "Vibration", "Grip motor strength: left is the low-frequency motor, right the high-frequency one.");
    let levels = [(0usize, "Off"), (1, "1"), (2, "2"), (3, "3")];
    for (side, title) in [(Side::Left, "Left grip vibration"), (Side::Right, "Right grip vibration")] {
        card(ui, title, |ui| {
            if let Some(l) = segmented(ui, buf.vibration_level(side), &levels) {
                buf.set_vibration_level(side, l);
                let pct = profile::VIBRATION_LEVELS[l];
                if pct > 0 {
                    let pwm = vib_pwm(pct);
                    ctl.ui_rumble.push(if side == Side::Left { (pwm, 0, 450) } else { (0, pwm, 450) });
                }
            }
        });
    }
    card(ui, "Test", |ui| {
        ui.horizontal(|ui| {
            if ui.button("Strong").clicked() {
                ctl.ui_rumble.push((255, 0, 600));
            }
            if ui.button("Weak").clicked() {
                ctl.ui_rumble.push((0, 255, 600));
            }
            if ui.button("Both").clicked() {
                ctl.ui_rumble.push((255, 255, 600));
            }
        });
        ui.add_space(6.0);
        if ui.checkbox(grip, "Try grip vibration").changed() {
            dev.grip_test.store(*grip, Ordering::Relaxed);
            if !*grip {
                ctl.ui_rumble.push((0, 0, 0));
            }
        }
        ui.small("While on, pressing LT or RT drives the left or right motor in real time.");
    });
}

/// Returns true when "Calibrate" was clicked.
fn motion(ui: &mut egui::Ui, buf: &mut Profile) -> bool {
    heading(ui, "Motion", "Drive a stick with the gyro: activation, deadzone and response curve.");
    let mut calibrate = false;
    card(ui, "Activation method", |ui| {
        let opts = [
            (GyroMethod::Off, "Off"),
            (GyroMethod::PressToTurnOn, "Press to turn on"),
            (GyroMethod::PressToTurnOff, "Press to turn off"),
            (GyroMethod::AlwaysOn, "Always on"),
        ];
        if let Some(m) = segmented(ui, buf.gyro_method(), &opts) {
            buf.set_gyro_method(m);
        }
    });
    card(ui, "Output", |ui| {
        let opts = [(GyroOutput::Off, "Off"), (GyroOutput::LeftStick, "Left Stick"), (GyroOutput::RightStick, "Right Stick")];
        if let Some(o) = segmented(ui, buf.gyro_output(), &opts) {
            buf.set_gyro_output(o);
        }
    });
    card(ui, "Activation button", |ui| {
        let cur = buf.gyro_button();
        ui.horizontal_wrapped(|ui| {
            for code in GYRO_BUTTONS {
                let label = if code == 0 { "None" } else { Button::from_code(code).map_or("?", |b| b.label()) };
                if ui.selectable_label(cur == code, label).clicked() {
                    buf.set_gyro_button(code);
                }
            }
        });
        ui.small("Used by \"Press to turn on\" and \"Press to turn off\".");
    });
    let dz = buf.gyro_deadzone();
    card(ui, &format!("Deadzone   {}", range_label(dz)), |ui| {
        if let Some((h, pct)) = dual_range(ui, dz, "gyro") {
            buf.drag_gyro_deadzone(h, pct);
        }
        ui.small("Inner and outer limits of the gyro response.");
    });
    card(ui, "Response curve", |ui| {
        let opts = [(1u8, "Default"), (0, "Aggressive"), (2, "Smooth"), (profile::CURVE_CUSTOM, "Custom")];
        if let Some(c) = segmented(ui, buf.gyro_curve(), &opts) {
            buf.set_gyro_curve(c);
        }
        ui.add_space(6.0);
        curve_chart(ui, buf);
        ui.small("Custom keeps the curve stored on the controller; drag its two points to change it.");
    });
    card(ui, "Calibrate gyro", |ui| {
        ui.horizontal(|ui| {
            if ui.button("Calibrate").clicked() {
                calibrate = true;
            }
            ui.small("Rest the controller on a flat, still surface first.");
        });
    });
    card(ui, "Axes", |ui| {
        ui.label("Motion axis");
        if let Some(v) = segmented(ui, buf.gyro_motion_axis(), &[(0, "Tilt (default)"), (1, "Rotation")]) {
            buf.set_gyro_motion_axis(v);
        }
        ui.add_space(4.0);
        ui.label("Active axis");
        if let Some(v) = segmented(ui, buf.gyro_active_axis(), &[(0, "Both"), (1, "Vertical only"), (2, "Horizontal only")]) {
            buf.set_gyro_active_axis(v);
        }
        ui.add_space(4.0);
        let mut x = buf.gyro_invert_x();
        if ui.checkbox(&mut x, "Invert X").changed() {
            buf.set_gyro_invert_x(x);
        }
        let mut y = buf.gyro_invert_y();
        if ui.checkbox(&mut y, "Invert Y").changed() {
            buf.set_gyro_invert_y(y);
        }
    });
    calibrate
}

/// Response-curve chart in normalised coordinates. Presets show their nominal shape;
/// Custom is drawn from the stored bytes and its points can be dragged.
fn curve_chart(ui: &mut egui::Ui, buf: &mut Profile) {
    let curve = buf.gyro_curve();
    let [o1, t1, o2, t2] = buf.gyro_curve_bytes();
    let (line, dots): (Vec<[f32; 2]>, [[f32; 2]; 2]) = match curve {
        0 => (vec![[0.0, 0.0], [0.32, 0.70], [1.0, 1.0]], [[0.32, 0.70], [1.0, 1.0]]),
        2 => (vec![[0.0, 0.0], [0.25, 0.17], [0.75, 0.65]], [[0.25, 0.17], [0.75, 0.65]]),
        profile::CURVE_CUSTOM => {
            let a = [o1 as f32 / 255.0, t1 as f32 / 127.0];
            let b = [o2 as f32 / 255.0, t2 as f32 / 127.0];
            let mut l = vec![[0.0, 0.0], a, b];
            if o2 < 255 {
                l.push([1.0, b[1]]);
            }
            (l, [a, b])
        }
        _ => (vec![[0.0, 0.0], [1.0, 1.0]], [[0.0, 0.0], [1.0, 1.0]]),
    };
    let size = 200.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::drag());
    let to_screen = |p: [f32; 2]| Pos2::new(rect.left() + p[0] * size, rect.bottom() - p[1].clamp(0.0, 1.0) * size);
    let painter = ui.painter_at(rect.expand(6.0));
    let v = ui.visuals();
    painter.rect_stroke(rect, 4.0, Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color), egui::StrokeKind::Inside);
    for i in 1..4 {
        let f = i as f32 / 4.0;
        let c = v.widgets.noninteractive.bg_stroke.color.gamma_multiply(0.5);
        painter.line_segment([Pos2::new(rect.left() + f * size, rect.top()), Pos2::new(rect.left() + f * size, rect.bottom())], Stroke::new(0.5, c));
        painter.line_segment([Pos2::new(rect.left(), rect.top() + f * size), Pos2::new(rect.right(), rect.top() + f * size)], Stroke::new(0.5, c));
    }
    painter.add(egui::Shape::line(line.iter().map(|&p| to_screen(p)).collect(), Stroke::new(2.5, ACCENT)));
    for d in dots {
        painter.circle_filled(to_screen(d), 5.0, ACCENT);
    }

    if curve != profile::CURVE_CUSTOM {
        return;
    }
    let key = egui::Id::unique("curve_drag");
    if resp.drag_started()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let d = |p: [f32; 2]| to_screen(p).distance(pos);
        let grabbed = if d(dots[0]).min(d(dots[1])) > 14.0 {
            None
        } else if d(dots[0]) <= d(dots[1]) {
            Some(CurvePoint::P1)
        } else {
            Some(CurvePoint::P2)
        };
        ui.data_mut(|m| m.insert_temp(key, grabbed));
    }
    let grabbed: Option<CurvePoint> = ui.data(|m| m.get_temp(key)).flatten();
    if resp.drag_stopped() {
        ui.data_mut(|m| m.remove::<Option<CurvePoint>>(key));
    }
    if resp.dragged()
        && resp.drag_delta() != Vec2::ZERO
        && let (Some(pt), Some(pos)) = (grabbed, resp.interact_pointer_pos())
    {
        let sx = ((pos.x - rect.left()) / rect.width() * 200.0) as f64;
        let sy = ((pos.y - rect.top()) / rect.height() * 200.0) as f64;
        buf.drag_gyro_curve(pt, sx, sy);
    }
}

fn extended(ui: &mut egui::Ui, buf: &mut Profile, firmware: Option<&str>) {
    heading(ui, "Extended", "Wake behaviour, auto-sleep and firmware.");
    card(ui, "Power", |ui| {
        let mut wake = buf.pick_up_to_wake();
        if ui.checkbox(&mut wake, "Pick up to wake").changed() {
            buf.set_pick_up_to_wake(wake);
        }
        ui.add_space(6.0);
        let cur = buf.sleep_minutes();
        ui.label(format!("Auto sleep when inactive: {}", if cur == 0 { "Off".to_string() } else { format!("{cur} min") }));
        let opts: Vec<(u8, String)> = SLEEP_MINUTES.iter().map(|&m| (m, if m == 0 { "Off".into() } else { format!("{m} min") })).collect();
        let opts: Vec<(u8, &str)> = opts.iter().map(|(m, s)| (*m, s.as_str())).collect();
        if let Some(m) = segmented(ui, cur, &opts) {
            buf.set_sleep_minutes(m);
        }
    });
    card(ui, "Firmware", |ui| {
        egui::Grid::new("fw").num_columns(2).show(ui, |ui| {
            ui.label("Installed");
            ui.monospace(firmware.unwrap_or("—"));
            ui.end_row();
            ui.label("Latest known");
            ui.monospace(p::LATEST_KNOWN_FIRMWARE);
            ui.end_row();
        });
        match firmware {
            Some(fw) if p::firmware_is_newer(p::LATEST_KNOWN_FIRMWARE, fw) => {
                ui.colored_label(ACCENT, format!("Firmware {} is available.", p::LATEST_KNOWN_FIRMWARE));
                ui.small("Flashing uses Cosmic Byte's Windows utility, with the controller connected over USB:");
                ui.hyperlink(p::FIRMWARE_DOWNLOAD_URL);
            }
            Some(_) => {
                ui.small("Up to date with the newest firmware this tool knows about.");
            }
            None => {
                ui.small("Couldn't read the firmware version. Reconnecting over wired USB usually fixes it.");
            }
        }
        ui.small("This tool never flashes firmware.");
    });
}

// ---------------------------------------------------------------- live preview

fn draw_gamepad(ui: &mut egui::Ui, g: Gamepad, has_data: bool) {
    let v = ui.visuals().clone();
    let dim = v.widgets.inactive.bg_fill;
    let line = v.widgets.noninteractive.fg_stroke.color.gamma_multiply(0.5);
    let text = v.text_color();
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 440.0), Sense::hover());
    let pt = ui.painter_at(rect);
    let at = |x: f32, y: f32| Pos2::new(rect.left() + x * w / 280.0, rect.top() + y);
    let on = |b: bool| if b { ACCENT } else { dim };
    let small = FontId::proportional(11.0);

    // triggers
    for (i, (label, val)) in [("LT", g.lt), ("RT", g.rt)].into_iter().enumerate() {
        let x = if i == 0 { 20.0 } else { 150.0 };
        let r = Rect::from_min_size(at(x, 8.0), Vec2::new(110.0 * w / 280.0, 12.0));
        pt.rect_filled(r, 4.0, dim);
        pt.rect_filled(Rect::from_min_size(r.min, Vec2::new(r.width() * val as f32 / 255.0, r.height())), 4.0, ACCENT);
        pt.text(r.left_bottom() + Vec2::new(0.0, 4.0), Align2::LEFT_TOP, format!("{label} {}%", (val as f32 / 255.0 * 100.0).round()), small.clone(), text);
    }
    // bumpers
    for (x, label, b) in [(20.0, "LB", g.lb), (150.0, "RB", g.rb)] {
        let r = Rect::from_min_size(at(x, 46.0), Vec2::new(110.0 * w / 280.0, 20.0));
        pt.rect_filled(r, 8.0, on(b));
        pt.text(r.center(), Align2::CENTER_CENTER, label, small.clone(), text);
    }
    // sticks
    for (cx, label, (x, y)) in [(75.0, "LS", g.left), (205.0, "RS", g.right)] {
        let c = at(cx, 140.0);
        let r = 48.0;
        pt.circle(c, r, Color32::TRANSPARENT, Stroke::new(1.5, line));
        pt.circle(c, r * 0.5, Color32::TRANSPARENT, Stroke::new(0.5, line));
        pt.line_segment([c - Vec2::new(r, 0.0), c + Vec2::new(r, 0.0)], Stroke::new(0.5, line));
        pt.line_segment([c - Vec2::new(0.0, r), c + Vec2::new(0.0, r)], Stroke::new(0.5, line));
        pt.circle_filled(c + Vec2::new(x, y) * r, 7.0, ACCENT);
        pt.text(c + Vec2::new(0.0, r + 6.0), Align2::CENTER_TOP, format!("{label}  X {:.0}  Y {:.0}", x * 100.0, -y * 100.0), small.clone(), text);
    }
    // d-pad
    let d = at(75.0, 280.0);
    for (off, b, label) in [((0.0, -26.0), g.up, "▲"), ((0.0, 26.0), g.down, "▼"), ((-26.0, 0.0), g.left_dpad, "◀"), ((26.0, 0.0), g.right_dpad, "▶")] {
        let c = d + Vec2::new(off.0, off.1);
        pt.rect_filled(Rect::from_center_size(c, Vec2::splat(22.0)), 4.0, on(b));
        pt.text(c, Align2::CENTER_CENTER, label, small.clone(), text);
    }
    // face buttons
    let f = at(205.0, 280.0);
    for (off, b, label) in [((0.0, -26.0), g.y, "Y"), ((0.0, 26.0), g.a, "A"), ((-26.0, 0.0), g.x, "X"), ((26.0, 0.0), g.b, "B")] {
        let c = f + Vec2::new(off.0, off.1);
        pt.circle_filled(c, 12.0, on(b));
        pt.text(c, Align2::CENTER_CENTER, label, FontId::proportional(13.0), text);
    }
    // system
    for (x, b, label) in [(40.0, g.select, "SELECT"), (105.0, g.capture, "CAPTURE"), (175.0, g.home, "HOME"), (240.0, g.start, "START")] {
        let r = Rect::from_center_size(at(x, 370.0), Vec2::new(54.0, 18.0));
        pt.rect_filled(r, 9.0, on(b));
        pt.text(r.center(), Align2::CENTER_CENTER, label, FontId::proportional(9.5), text);
    }
    if !has_data {
        pt.text(at(140.0, 420.0), Align2::CENTER_CENTER, "waiting for input…", small, text);
    }
}
