use super::theme::{self, Palette, regular, semibold};
use super::widgets::{self, Direction};
use omnidl::config::{AudioQuality, Config, Cookies, Format, VideoQuality};
use omnidl::deps::DepsStatus;
use omnidl::engine::{self, Command};
use omnidl::job::{Event, JobId, JobState};
use omnidl::jobs::{Job, Jobs, Tone as StatusTone};
use omnidl::launch::Launch;
use omnidl::schedule::{self, Preset};
use omnidl::update::{self, Status as UpdateStatus};
use omnidl::{detect, extension, util};
use egui::{
    Align, Align2, Color32, Frame, Label, Layout, Margin, PopupCloseBehavior, Rect, RichText, Sense, Stroke,
    StrokeKind, Ui, ViewportCommand, pos2, vec2,
};
use std::path::PathBuf;

pub struct App {
    cmd: tokio::sync::mpsc::UnboundedSender<Command>,
    events: std::sync::mpsc::Receiver<Event>,
    cfg: Config,
    base: PathBuf,
    input: String,
    jobs: Jobs,
    deps: DepsStatus,
    /// A complete tool check has come back. Until then nothing is reported missing.
    deps_checked: bool,
    update: UpdateStatus,
    /// The release last offered, for installing and retrying.
    release: Option<update::Release>,
    update_hidden: bool,
    /// Whether the browser extension can reach this instance.
    bridge: Result<(), String>,
    ext_error: Option<String>,
    settings_open: bool,
    focus_field: bool,
    hide_warning: bool,
    /// Custom start time in the "later" menu: hour, and minutes in 5-minute steps.
    later_hour: usize,
    later_min: usize,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        cfg: Config,
        base: PathBuf,
        listener: Result<std::net::TcpListener, String>,
        launch: Launch,
    ) -> Self {
        theme::install(&cc.egui_ctx);
        let bridge = listener.as_ref().map(|_| ()).map_err(Clone::clone);
        let (tx, events) = std::sync::mpsc::channel::<Event>();
        let (tx, ctx) = (std::sync::Mutex::new(tx), cc.egui_ctx.clone());
        let sink: engine::Sink = std::sync::Arc::new(move |ev| {
            if tx.lock().unwrap().send(ev).is_ok() {
                ctx.request_repaint();
            }
        });
        let cmd = engine::start(engine::Start {
            sink,
            base: base.clone(),
            cfg: cfg.clone(),
            journal: Some(base.join("queue.json")),
            listener: listener.ok(),
            launch,
            check_updates: true,
        });
        Self {
            cmd,
            events,
            cfg,
            base,
            input: String::new(),
            jobs: Jobs::default(),
            deps: DepsStatus::default(),
            deps_checked: false,
            update: UpdateStatus::Idle,
            release: None,
            update_hidden: false,
            bridge,
            ext_error: None,
            settings_open: false,
            focus_field: true,
            hide_warning: false,
            later_hour: schedule::next_full_hour(schedule::local_now()) as usize,
            later_min: 0,
        }
    }

    fn send(&self, c: Command) {
        let _ = self.cmd.send(c);
    }

    /// Saves the settings and hands them to the engine for links from outside.
    fn config_changed(&self) {
        self.cfg.save();
        self.send(Command::SetConfig(self.cfg.clone()));
    }

    fn drain_events(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                Event::Deps(d) => {
                    // Busy-only updates carry no tool versions; keep what we know.
                    if d.ytdlp.is_none() && d.busy.is_some() {
                        self.deps.busy = d.busy;
                    } else {
                        self.deps_checked |= d.busy.is_none();
                        self.deps = d;
                    }
                }
                Event::Job(id, update) => self.jobs.apply(id, update),
                Event::Update(s) => self.on_update(s),
                Event::Remote { focus } => {
                    if focus {
                        ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
                        ctx.send_viewport_cmd(ViewportCommand::Focus);
                    }
                }
            }
        }
    }

    fn on_update(&mut self, s: UpdateStatus) {
        // Once swapped in, only a restart changes anything; while loading, a
        // background check must not replace the progress.
        match (&self.update, &s) {
            (UpdateStatus::Ready(_), _) => return,
            (UpdateStatus::Downloading { .. }, UpdateStatus::Checking | UpdateStatus::Available(_)) => return,
            _ => {}
        }
        if let UpdateStatus::Available(rel) = &s {
            if self.release.as_ref() != Some(rel) {
                self.update_hidden = false;
            }
            self.release = Some(rel.clone());
        }
        self.update = s;
    }

    fn start_download(&mut self, start_at: Option<i64>) {
        let urls = detect::split_urls(&self.input);
        if urls.is_empty() {
            return;
        }
        self.input.clear();
        self.send(Command::Add { urls, cfg: self.cfg.clone(), start_at });
    }

    fn perform(&mut self, action: Action, ctx: &egui::Context) {
        match action {
            Action::UpdateTools => self.send(Command::UpdateTools),
            Action::InstallUpdate => {
                if let Some(rel) = self.release.clone() {
                    self.send(Command::InstallUpdate(rel));
                }
            }
            Action::Restart => match std::env::current_exe().and_then(|exe| update::restart(&exe)) {
                Ok(()) => ctx.send_viewport_cmd(ViewportCommand::Close),
                Err(e) => self.update = UpdateStatus::Failed(format!("Neustart fehlgeschlagen: {e}")),
            },
        }
    }

    fn banner(&self) -> Option<Banner> {
        let d = &self.deps;
        if let Some(busy) = &d.busy {
            // The routine check at startup takes a moment; not worth a banner.
            if !self.deps_checked && busy.starts_with("Prüfe") {
                return None;
            }
            return Some(Banner::new(Tone::Busy, busy.clone(), None));
        }
        if !self.deps_checked {
            return self.update_banner();
        }
        if let Some(err) = &d.error {
            let text = format!("Werkzeuge konnten nicht eingerichtet werden: {err}");
            return Some(Banner::new(Tone::Error, text, Some(("Erneut versuchen", Action::UpdateTools))));
        }
        if d.ytdlp.is_none() {
            let text = "yt-dlp fehlt oder startet nicht.".into();
            return Some(Banner::new(Tone::Error, text, Some(("Neu laden", Action::UpdateTools))));
        }
        if !d.ffmpeg {
            let text = "ffmpeg fehlt – ohne geht keine Umwandlung.".into();
            return Some(Banner::new(Tone::Error, text, Some(("Laden", Action::UpdateTools))));
        }
        if let Some(b) = self.update_banner() {
            return Some(b);
        }
        if self.hide_warning {
            return None;
        }
        if d.js.is_none() {
            let text = "Keine JavaScript-Laufzeit gefunden – YouTube braucht eine.".into();
            return Some(Banner::new(Tone::Warning, text, Some(("Laden", Action::UpdateTools))).dismiss(Dismiss::ToolWarning));
        }
        if let Some(age) = d.ytdlp_age_days.filter(|&a| a > 14) {
            let text =
                format!("yt-dlp wurde seit {age} Tagen nicht aktualisiert. Scheitern YouTube-Downloads, hilft meist ein Update.");
            return Some(
                Banner::new(Tone::Warning, text, Some(("Aktualisieren", Action::UpdateTools))).dismiss(Dismiss::ToolWarning),
            );
        }
        None
    }

    fn update_banner(&self) -> Option<Banner> {
        match &self.update {
            UpdateStatus::Available(rel) if !self.update_hidden => {
                let text = format!("omnidl {} ist verfügbar.", rel.version);
                Some(Banner::new(Tone::Info, text, Some(("Aktualisieren", Action::InstallUpdate))).dismiss(Dismiss::Update))
            }
            UpdateStatus::Downloading { version, frac } => {
                let text = match frac {
                    Some(f) => format!("Lade omnidl {version} … {:.0} %", f * 100.0),
                    None => format!("Lade omnidl {version} …"),
                };
                Some(Banner::new(Tone::Busy, text, None))
            }
            UpdateStatus::Ready(v) => {
                let text = format!("omnidl {v} ist installiert. Laufende Downloads gehen nach dem Neustart weiter.");
                Some(Banner::new(Tone::Info, text, Some(("Neu starten", Action::Restart))))
            }
            UpdateStatus::Failed(e) if self.release.is_some() && !self.update_hidden => {
                let text = format!("Update fehlgeschlagen: {e}");
                Some(Banner::new(Tone::Error, text, Some(("Erneut versuchen", Action::InstallUpdate))).dismiss(Dismiss::Update))
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Tone {
    Busy,
    Info,
    Warning,
    Error,
}

#[derive(Clone, Copy, PartialEq)]
enum Action {
    UpdateTools,
    InstallUpdate,
    Restart,
}

#[derive(Clone, Copy, PartialEq)]
enum Dismiss {
    ToolWarning,
    Update,
}

struct Banner {
    tone: Tone,
    text: String,
    action: Option<(&'static str, Action)>,
    dismiss: Option<Dismiss>,
}

impl Banner {
    fn new(tone: Tone, text: String, action: Option<(&'static str, Action)>) -> Self {
        Self { tone, text, action, dismiss: None }
    }

    fn dismiss(mut self, d: Dismiss) -> Self {
        self.dismiss = Some(d);
        self
    }
}

#[derive(Default)]
struct RowActions {
    cancel: Option<JobId>,
    start_now: Option<JobId>,
    retry: Option<JobId>,
    reveal: Option<PathBuf>,
    toggle: Option<usize>,
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Also runs while the window is minimized, so links from the browser
        // and finished downloads are handled without a visible frame.
        self.drain_events(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let p = theme::palette(ui);
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Comma)) {
            self.settings_open = !self.settings_open;
        }

        let pad = |top: i8, bottom: i8| {
            Frame::new().fill(p.background).inner_margin(Margin { left: 20, right: 20, top, bottom })
        };
        egui::Panel::top("head")
            .frame(pad(18, 14))
            .show_separator_line(false)
            .resizable(false)
            .show(ui, |ui| self.header(ui));
        egui::Panel::bottom("foot")
            .frame(pad(8, 12))
            .show_separator_line(false)
            .resizable(false)
            .show(ui, |ui| self.footer(ui));
        egui::CentralPanel::default().frame(pad(4, 0)).show(ui, |ui| self.queue(ui));

        if self.settings_open {
            self.settings(ui);
        }
        let busy_banner = self.deps.busy.is_some() || matches!(self.update, UpdateStatus::Checking | UpdateStatus::Downloading { .. });
        if self.jobs.animating() || busy_banner {
            ui.ctx().request_repaint();
        }
    }
}

// ---------------------------------------------------------------- main window

impl App {
    fn header(&mut self, ui: &mut Ui) {
        let p = theme::palette(ui);
        let urls = detect::split_urls(&self.input);
        // Links may be added before the tools are ready; they wait for them.
        let can_start = !urls.is_empty();
        let shortcut = ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter));

        let mut submit = false;
        let mut later = None;
        ui.horizontal(|ui| {
            let (button_w, symbol_w) = (88.0, 32.0);
            let field_w = ui.available_width() - button_w - 2.0 * symbol_w - 3.0 * ui.spacing().item_spacing.x;
            submit |= self.link_field(ui, field_w, &urls);

            submit |= widgets::primary_button(ui, "Laden", vec2(button_w, 36.0), can_start).clicked();

            let clock = widgets::symbol_button(ui, symbol_w, "later", |painter, rect, hovered| {
                widgets::clock(painter, rect.center(), 8.5, if hovered { p.label } else { p.secondary });
            });
            let clock = clock.on_hover_text("Später laden");
            later = egui::Popup::menu(&clock)
                .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| self.later_menu(ui, can_start))
                .and_then(|r| r.inner);

            let gear = widgets::symbol_button(ui, symbol_w, "settings", |painter, rect, hovered| {
                widgets::gear(painter, rect.center(), 9.0, if hovered { p.label } else { p.secondary });
            });
            if gear.on_hover_text("Einstellungen  (Strg+,)").clicked() {
                self.settings_open = true;
            }
        });
        if (submit || shortcut) && can_start {
            self.start_download(None);
            self.focus_field = true;
        }
        if let Some(at) = later {
            self.start_download(Some(at));
        }

        ui.add_space(12.0);
        self.options(ui);

        if let Some(banner) = self.banner() {
            ui.add_space(14.0);
            self.show_banner(ui, banner);
        }
    }

    /// "Later" pop-up: presets and a custom time. Returns the chosen start (Unix seconds).
    fn later_menu(&mut self, ui: &mut Ui, can: bool) -> Option<i64> {
        let p = theme::palette(ui);
        ui.set_min_width(268.0);
        ui.spacing_mut().item_spacing.y = 0.0;
        let now = schedule::local_now();
        let mut chosen = None;

        menu_caption(ui, "Später laden");
        ui.add_enabled_ui(can, |ui| {
            for preset in Preset::ALL {
                if widgets::menu_item(ui, false, &preset.label()).clicked() {
                    chosen = Some(preset.start(now));
                }
            }
        });

        menu_separator(ui);
        menu_caption(ui, "Eigene Uhrzeit");
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            widgets::stepper(ui, "later-hour", &mut self.later_hour, 0..=23);
            let time = format!("{}:{:02}", self.later_hour, self.later_min * 5);
            let (r, _) = ui.allocate_exact_size(vec2(46.0, 28.0), Sense::hover());
            ui.painter().text(r.center(), Align2::CENTER_CENTER, time, semibold(15.0), p.label);
            widgets::stepper(ui, "later-min", &mut self.later_min, 0..=11);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(6.0);
                if widgets::primary_button(ui, "Planen", vec2(66.0, 28.0), can).clicked() {
                    chosen = Some(schedule::next_at(now, self.later_hour as u32, self.later_min as u32 * 5));
                }
            });
        });
        let at = schedule::next_at(now, self.later_hour as u32, self.later_min as u32 * 5);
        menu_note(ui, &format!("Startet {}.", schedule::describe(at, now)), false);
        if !can {
            menu_note(ui, "Erst oben einen Link einfügen.", true);
        }

        if chosen.is_some() {
            ui.close();
        }
        chosen.map(schedule::to_unix)
    }

    /// Search-field-style input with a source badge and a clear button.
    /// Returns `true` when Enter was pressed.
    fn link_field(&mut self, ui: &mut Ui, width: f32, urls: &[String]) -> bool {
        let p = theme::palette(ui);
        let (rect, bg) = ui.allocate_exact_size(vec2(width, 36.0), Sense::click());
        let frame = ui.painter().add(egui::Shape::Noop);

        let mut right = rect.right() - 8.0;
        let clear = (!self.input.is_empty()).then(|| {
            let r = Rect::from_center_size(pos2(right - 10.0, rect.center().y), vec2(22.0, 22.0));
            right -= 26.0;
            r
        });
        let badge_text = match urls.len() {
            0 => None,
            1 => Some(detect::detect(&urls[0]).label().to_string()),
            n => Some(format!("{n} Links")),
        };
        let badge = badge_text.map(|t| {
            let galley = ui.painter().layout_no_wrap(t, semibold(11.5), p.blue);
            let size = galley.size() + vec2(14.0, 6.0);
            let r = Rect::from_min_size(pos2(right - size.x, rect.center().y - size.y / 2.0), size);
            right -= size.x + 8.0;
            (galley, r)
        });

        let text_rect = Rect::from_min_max(pos2(rect.left() + 12.0, rect.top()), pos2(right, rect.bottom()));
        let edit = egui::TextEdit::singleline(&mut self.input)
            .id(egui::Id::new("link-field"))
            .hint_text(RichText::new("Link einfügen – YouTube, TikTok, Spotify …").color(p.tertiary))
            .font(regular(14.5))
            .frame(Frame::NONE)
            .margin(Margin::ZERO)
            .vertical_align(Align::Center)
            .desired_width(text_rect.width());
        // A child that does not move the row's cursor: the field keeps its full width.
        let mut inner = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(text_rect)
                .layout(Layout::centered_and_justified(egui::Direction::TopDown)),
        );
        let resp = inner.add(edit);
        if resp.changed() {
            // Badge and button state are computed before the edit; show them now.
            ui.ctx().request_repaint();
        }
        if bg.on_hover_cursor(egui::CursorIcon::Text).clicked() {
            self.focus_field = true;
        }
        if self.focus_field {
            resp.request_focus();
            self.focus_field = false;
        }

        let focused = resp.has_focus();
        let border = if focused {
            Stroke::new(1.0, p.blue)
        } else {
            Stroke::new(1.0, if p.dark { Color32::TRANSPARENT } else { p.border })
        };
        let mut shapes = vec![egui::Shape::from(egui::epaint::RectShape::new(
            rect,
            9,
            p.card,
            border,
            StrokeKind::Inside,
        ))];
        if focused {
            // macOS focus ring.
            shapes.push(
                egui::epaint::RectShape::stroke(
                    rect,
                    9,
                    Stroke::new(3.5, p.blue.gamma_multiply(0.35)),
                    StrokeKind::Outside,
                )
                .into(),
            );
        }
        ui.painter().set(frame, egui::Shape::Vec(shapes));

        if let Some((galley, r)) = badge {
            ui.painter().rect_filled(r, r.height() / 2.0, p.blue.gamma_multiply(0.14));
            ui.painter().galley(r.center() - galley.size() / 2.0, galley, p.blue);
        }
        if let Some(r) = clear {
            let b = widgets::symbol_button_at(ui, r, ui.id().with("clear"), |painter, r, hovered| {
                widgets::xmark_circle(painter, r.center(), 7.5, if hovered { p.secondary } else { p.tertiary }, p.card);
            });
            if b.on_hover_text("Leeren").clicked() {
                self.input.clear();
                self.focus_field = true;
            }
        }

        resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
    }

    fn options(&mut self, ui: &mut Ui) {
        let p = theme::palette(ui);
        ui.horizontal(|ui| {
            let mut changed = false;

            let mut kind = usize::from(self.cfg.format.is_audio());
            if widgets::segmented(ui, "kind", &mut kind, &["Video", "Audio"], 150.0) {
                self.cfg.set_audio(kind == 1);
                changed = true;
            }

            let formats: &[Format] = if self.cfg.format.is_audio() { &Format::AUDIO } else { &Format::VIDEO };
            widgets::popup(ui, "format", self.cfg.format.label(), 76.0, |ui| {
                for &f in formats {
                    if widgets::menu_item(ui, self.cfg.format == f, f.label()).clicked() {
                        self.cfg.set_format(f);
                        changed = true;
                    }
                }
            });

            if !self.cfg.format.is_audio() {
                widgets::popup(ui, "vq", self.cfg.video_quality.label(), 128.0, |ui| {
                    for q in VideoQuality::ALL {
                        if widgets::menu_item(ui, self.cfg.video_quality == q, q.label()).clicked() {
                            self.cfg.video_quality = q;
                            changed = true;
                        }
                    }
                });
            } else if self.cfg.format.has_bitrate() {
                widgets::popup(ui, "aq", self.cfg.audio_quality.label(), 128.0, |ui| {
                    for q in AudioQuality::ALL {
                        if widgets::menu_item(ui, self.cfg.audio_quality == q, q.label()).clicked() {
                            self.cfg.audio_quality = q;
                            changed = true;
                        }
                    }
                    menu_note(ui, "Die Quelle hat meist 128–160 kbps. Höhere Werte machen die Datei größer, nicht besser.", true);
                });
            } else {
                ui.add_enabled_ui(false, |ui| widgets::popup(ui, "lossless", "Verlustfrei", 128.0, |_| {}))
                    .response
                    .on_disabled_hover_text(
                        "Die Quelle ist verlustbehaftet (~128–160 kbps).\nFLAC und WAV machen die Datei größer, nicht besser.",
                    );
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                changed |= widgets::toggle(ui, &mut self.cfg.playlist)
                    .on_hover_text("Aus: nur das einzelne Video, auch wenn der Link zu einer Playlist gehört")
                    .changed();
                ui.label(RichText::new("Ganze Playlist").color(p.label));
            });

            if changed {
                self.config_changed();
            }
        });
    }

    fn show_banner(&mut self, ui: &mut Ui, banner: Banner) {
        let p = theme::palette(ui);
        let tint = match banner.tone {
            Tone::Busy | Tone::Info => p.blue,
            Tone::Warning => p.orange,
            Tone::Error => p.red,
        };
        let time = ui.input(|i| i.time);
        let mut clicked = None;
        Frame::new()
            .fill(tint.gamma_multiply(if p.dark { 0.2 } else { 0.1 }))
            .corner_radius(10)
            .inner_margin(Margin::symmetric(12, 7))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                    match banner.tone {
                        Tone::Busy => {
                            widgets::ring(ui.painter(), r.center(), 8.0, None, time, p.blue.gamma_multiply(0.2), p.blue)
                        }
                        Tone::Info => widgets::arrow_down_circle(ui.painter(), r.center(), 7.5, p.blue),
                        _ => widgets::exclamation_circle(ui.painter(), r.center(), 8.0, tint),
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if let Some(d) = banner.dismiss {
                            let close = widgets::symbol_button(ui, 22.0, "dismiss", |painter, r, hovered| {
                                let c = if hovered { p.secondary } else { p.tertiary };
                                widgets::xmark_circle(painter, r.center(), 7.0, c, p.background);
                            });
                            if close.on_hover_text("Ausblenden").clicked() {
                                match d {
                                    Dismiss::ToolWarning => self.hide_warning = true,
                                    Dismiss::Update => self.update_hidden = true,
                                }
                            }
                        }
                        if let Some((label, action)) = banner.action {
                            if widgets::text_button(ui, label, p.blue).clicked() {
                                clicked = Some(action);
                            }
                        }
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            ui.add(Label::new(RichText::new(&banner.text).font(regular(13.0)).color(p.label)).truncate());
                        });
                    });
                });
            });
        if let Some(a) = clicked {
            self.perform(a, ui.ctx());
        }
    }

    fn queue(&mut self, ui: &mut Ui) {
        let p = theme::palette(ui);
        let scheduled = self.jobs.top_level().filter(|j| matches!(j.state, JobState::Scheduled(_))).count();
        let pending = self.jobs.top_level().filter(|j| !j.state.is_finished()).count();
        let running = pending - scheduled;
        let finished = self.jobs.top_level().any(|j| j.state.is_finished());

        ui.horizontal(|ui| {
            ui.label(RichText::new("Downloads").font(semibold(15.0)).color(p.label));
            let counts: Vec<String> = [(running, "aktiv"), (scheduled, "geplant")]
                .iter()
                .filter(|(n, _)| *n > 0)
                .map(|(n, what)| format!("{n} {what}"))
                .collect();
            if !counts.is_empty() {
                ui.label(RichText::new(counts.join(" · ")).font(regular(13.0)).color(p.secondary));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if finished && widgets::text_button(ui, "Liste leeren", p.blue).clicked() {
                    self.jobs.clear_finished();
                }
                if pending > 0 && widgets::text_button(ui, "Alle stoppen", p.red).clicked() {
                    self.send(Command::CancelAll);
                }
            });
        });
        ui.add_space(4.0);

        if self.jobs.list.is_empty() {
            empty_state(ui, p);
            return;
        }

        let rows = self.jobs.visible_rows();
        let time = ui.input(|i| i.time);
        let mut actions = RowActions::default();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            Frame::new()
                .fill(p.card)
                .corner_radius(12)
                .inner_margin(Margin::same(4))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for (n, &(idx, child)) in rows.iter().enumerate() {
                        self.row(ui, idx, child, n + 1 == rows.len(), time, &mut actions);
                    }
                });
            ui.add_space(14.0);
        });

        if let Some(id) = actions.cancel {
            self.send(Command::Cancel(id));
        }
        if let Some(id) = actions.start_now {
            self.send(Command::StartNow(id));
        }
        if let Some(id) = actions.retry {
            if let Some(url) = self.jobs.get(id).map(|j| j.url.clone()) {
                self.jobs.remove_tree(id);
                self.send(Command::Add { urls: vec![url], cfg: self.cfg.clone(), start_at: None });
            }
        }
        if let Some(path) = actions.reveal {
            util::reveal(&path);
        }
        if let Some(i) = actions.toggle {
            self.jobs.list[i].expanded = !self.jobs.list[i].expanded;
        }
    }

    fn row(&self, ui: &mut Ui, idx: usize, child: bool, last: bool, time: f64, act: &mut RowActions) {
        let p = theme::palette(ui);
        let job = &self.jobs.list[idx];
        let height = if child { 46.0 } else { 54.0 };
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
        if !ui.is_rect_visible(rect) {
            return;
        }
        let painter = ui.painter().clone();
        let id = ui.id().with(("job", job.id));
        if resp.hovered() {
            painter.rect_filled(rect, 8, p.hover);
        }

        let r = if child { 9.0 } else { 11.0 };
        let icon = pos2(rect.left() + 12.0 + if child { 30.0 } else { 0.0 } + r, rect.center().y);
        status_symbol(&painter, icon, r, job, time, p);
        let text_x = icon.x + r + 12.0;

        // Trailing actions, laid out from the right edge.
        let mut right = rect.right() - 8.0;
        let mut slot = || {
            let r = Rect::from_center_size(pos2(right - 13.0, rect.center().y), vec2(26.0, 26.0));
            right -= 30.0;
            r
        };
        if !job.state.is_finished() {
            let b = widgets::symbol_button_at(ui, slot(), id.with("cancel"), |painter, r, hovered| {
                widgets::xmark_circle(painter, r.center(), 8.0, if hovered { p.secondary } else { p.tertiary }, p.card);
            });
            if b.on_hover_text("Stoppen").clicked() {
                act.cancel = Some(job.id);
            }
        }
        if matches!(job.state, JobState::Scheduled(_)) {
            let b = widgets::symbol_button_at(ui, slot(), id.with("now"), |painter, r, _| {
                widgets::play(painter, r.center() + vec2(1.0, 0.0), 8.0, p.blue);
            });
            if b.on_hover_text("Jetzt starten").clicked() {
                act.start_now = Some(job.id);
            }
        }
        if !child && job.state.can_retry() && !job.url.is_empty() {
            let b = widgets::symbol_button_at(ui, slot(), id.with("retry"), |painter, r, _| {
                widgets::arrow_clockwise(painter, r.center(), 9.0, p.blue);
            });
            if b.on_hover_text("Erneut versuchen").clicked() {
                act.retry = Some(job.id);
            }
        }
        if let Some(path) = &job.output {
            let b = widgets::symbol_button_at(ui, slot(), id.with("reveal"), |painter, r, _| {
                widgets::magnifier(painter, r.center(), 8.5, p.blue);
            });
            if b.on_hover_text("Im Explorer zeigen").clicked() {
                act.reveal = Some(path.clone());
            }
        }
        if job.is_group() {
            let dir = if job.expanded { Direction::Down } else { Direction::Right };
            widgets::chevron(&painter, pos2(right - 10.0, rect.center().y), 4.0, dir, Stroke::new(1.8, p.tertiary));
            right -= 24.0;
        }

        let max_w = right - text_x - 8.0;
        let title = widgets::one_line(ui, &job.title, regular(if child { 13.5 } else { 14.0 }), p.label, max_w);
        let (status, color) = status_line(job, child, p);
        let status_galley = widgets::one_line(ui, &status, regular(12.0), color, max_w);
        let gap = 1.0;
        let top = rect.center().y - (title.size().y + gap + status_galley.size().y) / 2.0;
        let elided = title.elided || status_galley.elided;
        let title_h = title.size().y;
        painter.galley(pos2(text_x, top), title, p.label);
        painter.galley(pos2(text_x, top + title_h + gap), status_galley, color);

        if !last {
            painter.hline(text_x..=rect.right() - 8.0, rect.bottom() - 0.5, Stroke::new(1.0, p.separator));
        }

        if job.is_group() && resp.clicked() {
            act.toggle = Some(idx);
        }
        if resp.double_clicked() {
            if let Some(path) = &job.output {
                act.reveal = Some(path.clone());
            }
        }
        if elided || matches!(job.state, JobState::Failed(_)) {
            let detail = match &job.state {
                JobState::Failed(e) => e.clone(),
                _ => status,
            };
            resp.on_hover_text(format!("{}\n\n{detail}", job.title));
        }
    }

    fn footer(&mut self, ui: &mut Ui) {
        let p = theme::palette(ui);
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(16.0, 13.0), Sense::hover());
            widgets::folder(ui.painter(), r, p.blue);
            let dir = self.cfg.download_dir.display().to_string();
            let link = ui
                .add(
                    Label::new(RichText::new(truncate_start(&dir, 64)).font(regular(12.5)).color(p.secondary))
                        .sense(Sense::click()),
                )
                .on_hover_text("Im Explorer öffnen");
            if link.clicked() {
                let _ = std::fs::create_dir_all(&self.cfg.download_dir);
                util::reveal(&self.cfg.download_dir);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(v) = &self.deps.ytdlp {
                    ui.label(RichText::new(format!("yt-dlp {v}")).font(regular(12.0)).color(p.tertiary));
                }
            });
        });
    }
}

// ---------------------------------------------------------------- settings sheet

impl App {
    fn settings(&mut self, ui: &Ui) {
        let ctx = ui.ctx().clone();
        let p = theme::palette(ui);
        let max_height = (ctx.content_rect().height() - 150.0).max(120.0);
        let mut done = false;
        let mut action = None;
        let modal = egui::Modal::new(egui::Id::new("settings"))
            .backdrop_color(Color32::from_black_alpha(if p.dark { 110 } else { 50 }))
            .frame(
                Frame::new()
                    .fill(p.background)
                    .corner_radius(14)
                    .inner_margin(Margin::same(20))
                    .stroke(Stroke::new(1.0, p.separator))
                    .shadow(ui.visuals().window_shadow),
            )
            .show(&ctx, |ui| {
                ui.set_width(440.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("Einstellungen").font(semibold(17.0)).color(p.label));
                });

                egui::ScrollArea::vertical().max_height(max_height).show(ui, |ui| {
                    section(ui, p, "Downloads", None, |ui| {
                        let dir = self.cfg.download_dir.display().to_string();
                        form_row(ui, "Speicherort", p.label, |ui| {
                            if widgets::text_button(ui, "Ändern …", p.blue).clicked() {
                                if let Some(dir) = rfd::FileDialog::new().set_directory(&self.cfg.download_dir).pick_folder() {
                                    self.cfg.download_dir = dir;
                                    self.config_changed();
                                }
                            }
                            ui.label(RichText::new(truncate_start(&dir, 30)).color(p.secondary)).on_hover_text(&dir);
                        });
                        form_separator(ui, p);
                        form_row(ui, "Gleichzeitige Downloads", p.label, |ui| {
                            let mut n = self.cfg.parallel;
                            if widgets::stepper(ui, "parallel", &mut n, 1..=16) {
                                self.cfg.parallel = n;
                                self.config_changed();
                                self.send(Command::SetParallel(n));
                            }
                            ui.label(RichText::new(n.to_string()).color(p.label));
                        });
                    });

                    let cookies_note = "Für private oder altersbeschränkte Inhalte. Unter Windows klappt Firefox am zuverlässigsten.";
                    section(ui, p, "Anmeldung", Some((cookies_note, p.secondary)), |ui| {
                        form_row(ui, "Cookies aus Browser", p.label, |ui| {
                            widgets::popup(ui, "cookies", self.cfg.cookies.label(), 110.0, |ui| {
                                for c in Cookies::ALL {
                                    if widgets::menu_item(ui, self.cfg.cookies == c, c.label()).clicked() {
                                        self.cfg.cookies = c;
                                        self.config_changed();
                                    }
                                }
                            });
                        });
                    });

                    let browser_note = self.browser_note(p);
                    section(ui, p, "Browser", Some((&browser_note.0, browser_note.1)), |ui| {
                        form_row(ui, "Erweiterung für Chrome, Edge, Firefox", p.label, |ui| {
                            if widgets::text_button(ui, "Ordner öffnen …", p.blue).clicked() {
                                match extension::export(&self.base) {
                                    Ok(dir) => {
                                        self.ext_error = None;
                                        util::reveal(&dir);
                                    }
                                    Err(e) => self.ext_error = Some(e.to_string()),
                                }
                            }
                        });
                        form_separator(ui, p);
                        form_row(ui, "Kurzbefehl im Browser", p.label, |ui| {
                            ui.label(RichText::new("Alt+Umschalt+D").color(p.secondary));
                        });
                    });

                    let tools_note = match (&self.deps.busy, &self.deps.error) {
                        (Some(busy), _) => (busy.clone(), p.blue),
                        (None, Some(err)) => (err.clone(), p.red),
                        _ => ("Scheitern YouTube-Downloads, zuerst yt-dlp aktualisieren.".to_string(), p.secondary),
                    };
                    section(ui, p, "Werkzeuge", Some((&tools_note.0, tools_note.1)), |ui| self.tool_rows(ui, p));

                    let app_note = self.update_note(p);
                    section(ui, p, "App", Some((&app_note.0, app_note.1)), |ui| {
                        form_row(ui, "Version", p.label, |ui| {
                            let (label, act) = match &self.update {
                                UpdateStatus::Available(_) => (Some("Installieren"), Some(Action::InstallUpdate)),
                                UpdateStatus::Ready(_) => (Some("Neu starten"), Some(Action::Restart)),
                                UpdateStatus::Checking | UpdateStatus::Downloading { .. } => (None, None),
                                _ => (Some("Nach Updates suchen"), None),
                            };
                            if let Some(label) = label {
                                if widgets::text_button(ui, label, p.blue).clicked() {
                                    match act {
                                        Some(a) => action = Some(a),
                                        None => self.send(Command::CheckUpdate { manual: true }),
                                    }
                                }
                            }
                            ui.label(RichText::new(update::VERSION).color(p.secondary));
                        });
                        form_separator(ui, p);
                        form_row(ui, "Automatisch nach Updates suchen", p.label, |ui| {
                            if widgets::toggle(ui, &mut self.cfg.check_updates).changed() {
                                self.config_changed();
                            }
                        });
                    });
                });

                ui.add_space(18.0);
                ui.horizontal(|ui| {
                    ui.add_space(ui.available_width() - 96.0);
                    if widgets::primary_button(ui, "Fertig", vec2(96.0, 32.0), true).clicked() {
                        done = true;
                    }
                });
            });
        if let Some(a) = action {
            self.perform(a, &ctx);
        }
        if done || modal.should_close() {
            self.settings_open = false;
        }
    }

    fn browser_note(&self, p: &Palette) -> (String, Color32) {
        match (&self.bridge, &self.ext_error) {
            (Err(e), _) => (format!("Nicht verfügbar: {e}."), p.red),
            (Ok(()), Some(e)) => (format!("Ordner nicht anlegbar: {e}"), p.red),
            (Ok(()), None) => (
                "In Chrome oder Edge unter Erweiterungen den Entwicklermodus einschalten, „Entpackte Erweiterung laden“ \
                 wählen und diesen Ordner nehmen. Danach schickt ein Klick aufs omnidl-Symbol die offene Seite hierher."
                    .into(),
                p.secondary,
            ),
        }
    }

    fn update_note(&self, p: &Palette) -> (String, Color32) {
        match &self.update {
            UpdateStatus::Idle => (format!("Updates kommen von github.com/{}.", update::REPO), p.secondary),
            UpdateStatus::Checking => ("Suche nach Updates …".into(), p.blue),
            UpdateStatus::UpToDate => ("omnidl ist auf dem neuesten Stand.".into(), p.secondary),
            UpdateStatus::Available(rel) => (format!("Version {} ist verfügbar.", rel.version), p.blue),
            UpdateStatus::Downloading { version, frac } => {
                let pct = frac.map(|f| format!(" {:.0} %", f * 100.0)).unwrap_or_default();
                (format!("Lade Version {version} …{pct}"), p.blue)
            }
            UpdateStatus::Ready(v) => (format!("Version {v} ist installiert und startet mit dem nächsten Neustart."), p.secondary),
            UpdateStatus::Failed(e) => (e.clone(), p.red),
        }
    }

    fn tool_rows(&self, ui: &mut Ui, p: &Palette) {
        let d = &self.deps;
        let (version, color) = match &d.ytdlp {
            Some(v) if d.ytdlp_age_days.is_some_and(|a| a > 14) => (format!("{v} · veraltet"), p.orange),
            Some(v) => (v.clone(), p.secondary),
            None => ("fehlt".into(), p.red),
        };
        form_row(ui, "yt-dlp", p.label, |ui| {
            if d.busy.is_none() && widgets::text_button(ui, "Aktualisieren", p.blue).clicked() {
                self.send(Command::UpdateTools);
            }
            ui.label(RichText::new(version).color(color));
        });

        let present = |ok: bool, missing: Color32| {
            if ok { ("bereit".to_string(), p.secondary) } else { ("fehlt".to_string(), missing) }
        };
        let js = match d.js.as_deref() {
            Some("deno") => ("Deno".to_string(), p.secondary),
            Some(_) => ("Node.js".to_string(), p.secondary),
            None => ("fehlt".to_string(), p.orange),
        };
        let rows = [
            ("ffmpeg", present(d.ffmpeg, p.red)),
            ("JavaScript-Laufzeit", js),
            ("gallery-dl", present(d.gallery, p.orange)),
        ];
        for (name, (value, color)) in rows {
            form_separator(ui, p);
            form_row(ui, name, p.label, |ui| {
                ui.label(RichText::new(value).color(color));
            });
        }
    }
}

// ---------------------------------------------------------------- pieces

fn status_symbol(painter: &egui::Painter, c: egui::Pos2, r: f32, job: &Job, time: f64, p: &Palette) {
    match &job.state {
        JobState::Done => widgets::check_circle(painter, c, r, p.green),
        JobState::Failed(_) => widgets::exclamation_circle(painter, c, r, p.red),
        JobState::NoMatch => widgets::exclamation_circle(painter, c, r, p.orange),
        JobState::Cancelled => widgets::minus_circle(painter, c, r, p.gray),
        JobState::Scheduled(_) => widgets::clock(painter, c, r, p.blue),
        JobState::Queued => widgets::clock(painter, c, r, p.tertiary),
        JobState::Downloading | JobState::Group => widgets::ring(painter, c, r, job.frac, time, p.track, p.blue),
        JobState::Resolving | JobState::Processing => widgets::ring(painter, c, r, None, time, p.track, p.blue),
    }
}

fn status_line(job: &Job, child: bool, p: &Palette) -> (String, Color32) {
    let (text, tone) = job.status(child);
    let color = match tone {
        StatusTone::Normal => p.secondary,
        StatusTone::Warning => p.orange,
        StatusTone::Error => p.red,
    };
    (text, color)
}

/// Shown instead of the list while nothing has been added.
fn empty_state(ui: &mut Ui, p: &Palette) {
    let (rect, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
    let painter = ui.painter();
    let c = pos2(rect.center().x, rect.top() + rect.height() * 0.42);
    widgets::arrow_down_circle(painter, c - vec2(0.0, 36.0), 24.0, p.tertiary);
    painter.text(c + vec2(0.0, 12.0), Align2::CENTER_CENTER, "Keine Downloads", semibold(17.0), p.secondary);
    painter.text(
        c + vec2(0.0, 36.0),
        Align2::CENTER_CENTER,
        "Link oben einfügen und Enter drücken.",
        regular(13.0),
        p.secondary,
    );
    painter.text(
        c + vec2(0.0, 56.0),
        Align2::CENTER_CENTER,
        "YouTube · TikTok · Spotify · Instagram · SoundCloud · rund 1800 weitere Seiten",
        regular(12.0),
        p.tertiary,
    );
}

/// Grouped form section with a caption above and an optional note below.
fn section(ui: &mut Ui, p: &Palette, title: &str, note: Option<(&str, Color32)>, add: impl FnOnce(&mut Ui)) {
    ui.add_space(12.0);
    Frame::new().inner_margin(Margin::symmetric(14, 0)).show(ui, |ui| {
        ui.label(RichText::new(title).font(semibold(12.5)).color(p.secondary));
    });
    ui.add_space(-3.0);
    Frame::new()
        .fill(p.card)
        .corner_radius(10)
        .inner_margin(Margin::symmetric(14, 0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            add(ui);
        });
    if let Some((note, color)) = note {
        ui.add_space(-2.0);
        Frame::new().inner_margin(Margin::symmetric(14, 0)).show(ui, |ui| {
            ui.add(Label::new(RichText::new(note).font(regular(12.0)).color(color)).wrap());
        });
    }
}

/// Label on the left, controls on the right.
fn form_row(ui: &mut Ui, label: &str, color: Color32, trailing: impl FnOnce(&mut Ui)) {
    let width = ui.available_width();
    ui.allocate_ui_with_layout(vec2(width, 36.0), Layout::right_to_left(Align::Center), |ui| {
        ui.set_min_height(36.0);
        trailing(ui);
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            ui.add(Label::new(RichText::new(label).color(color)).truncate());
        });
    });
}

fn form_separator(ui: &mut Ui, p: &Palette) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(r.x_range(), r.center().y, Stroke::new(1.0, p.separator));
}

/// Small heading inside a pop-up menu.
fn menu_caption(ui: &mut Ui, text: &str) {
    let p = theme::palette(ui);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), semibold(11.5), p.secondary);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
    ui.painter().galley(pos2(rect.left() + 10.0, rect.center().y - galley.size().y / 2.0), galley, p.secondary);
}

fn menu_separator(ui: &mut Ui) {
    let p = theme::palette(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 9.0), Sense::hover());
    ui.painter().hline(rect.x_range().shrink(4.0), rect.center().y, Stroke::new(1.0, p.separator));
}

/// Footnote in a pop-up menu, optionally under a separator line.
fn menu_note(ui: &mut Ui, text: &str, rule: bool) {
    let p = theme::palette(ui);
    let galley = ui.painter().layout(text.to_owned(), regular(12.0), p.secondary, 240.0);
    let top = if rule { 11.0 } else { 6.0 };
    let (rect, _) = ui.allocate_exact_size(vec2(galley.size().x + 20.0, galley.size().y + top + 5.0), Sense::hover());
    if rule {
        ui.painter().hline(rect.x_range().shrink(4.0), rect.top() + 4.0, Stroke::new(1.0, p.separator));
    }
    ui.painter().galley(rect.min + vec2(10.0, top), galley, p.secondary);
}

fn truncate_start(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    let skip = n - max + 1;
    format!("…{}", s.chars().skip(skip).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;
    use omnidl::job::JobUpdate;

    #[test]
    fn long_paths_keep_their_end() {
        assert_eq!(truncate_start("C:/kurz", 10), "C:/kurz");
        assert_eq!(truncate_start("C:/Users/name/Downloads/omnidl", 12), "…oads/omnidl");
    }

    #[test]
    fn status_colors_follow_the_tone() {
        let mut jobs = Jobs::default();
        let url = "https://example.com/1".to_string();
        jobs.apply(1, JobUpdate::New { parent: None, url: url.clone(), title: url, source: "Web" });
        jobs.apply(1, JobUpdate::State(JobState::Failed("kaputt".into())));
        let p = theme::of(false);
        assert_eq!(status_line(jobs.get(1).unwrap(), false, p), ("Web · kaputt".to_string(), p.red));
    }
}
