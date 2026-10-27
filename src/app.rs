//! egui window: current track with a live progress bar, session history,
//! presence toggle, autostart, Client ID and theme switch.

use std::sync::mpsc::Sender;
use std::time::Duration;

use eframe::egui::{self, ColorImage, RichText, TextureHandle, TextureOptions, ViewportBuilder};

use crate::app_state::{send_command, with_state, Command, SharedUiState};
use crate::autostart;
use crate::config;
use crate::track::TrackSnapshot;

pub struct PresenceApp {
    state: SharedUiState,
    commands: Sender<Command>,
    /// UI-owned copy of the Client ID; synced to the poller via commands.
    client_id: String,
    enabled: bool,
    autostart: bool,
    dark_mode: bool,
    tray_minimize: bool,
    quit_requested: bool,
    tray: crate::tray::Tray,
    track: Option<TrackSnapshot>,
    history: Vec<TrackSnapshot>,
    cover: Option<TextureHandle>,
    last_cover_version: u64,
    status: String,
    status_ok: Option<bool>,
    discord_connected: bool,
    ctx: egui::Context,
}

impl PresenceApp {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ctx: &egui::Context,
        state: SharedUiState,
        commands: Sender<Command>,
        client_id: String,
        enabled: bool,
        autostart: bool,
        dark_mode: bool,
        tray_minimize: bool,
        tray: crate::tray::Tray,
    ) -> Self {
        let (track, cover_version, cover_image, history) = with_state(&state, |s| {
            (
                s.track.clone(),
                s.cover_version,
                s.cover.clone(),
                s.history.clone(),
            )
        });
        let cover = cover_image.map(|img| upload_cover(ctx, &img));
        if dark_mode {
            ctx.set_visuals(egui::Visuals::dark());
        } else {
            ctx.set_visuals(egui::Visuals::light());
        }
        Self {
            state,
            commands,
            client_id,
            enabled,
            autostart,
            dark_mode,
            tray_minimize,
            quit_requested: false,
            tray,
            track,
            history,
            cover,
            last_cover_version: cover_version,
            status: String::new(),
            status_ok: None,
            discord_connected: false,
            ctx: ctx.clone(),
        }
    }

    fn hide_to_tray(&self) {
        self.ctx
            .send_viewport_cmd(egui::ViewportCommand::Visible(false));
    }

    fn save_config(&mut self) {
        let cfg = config::Config {
            client_id: self.client_id.trim().to_string(),
            autostart: self.autostart,
            dark_mode: self.dark_mode,
            tray_minimize: self.tray_minimize,
        };
        if let Err(err) = config::save(&cfg) {
            self.status = err;
            self.status_ok = Some(false);
        }
    }

    fn save_client_id(&mut self) {
        let trimmed = self.client_id.trim().to_string();
        self.client_id = trimmed.clone();
        send_command(&self.commands, Command::SetClientId(trimmed));
        self.save_config();
    }

    fn toggle_autostart(&mut self, enable: bool) {
        match autostart::set_enabled(enable) {
            Ok(()) => {
                self.autostart = enable;
                self.status = if enable {
                    "Автозапуск включён".into()
                } else {
                    "Автозапуск выключен".into()
                };
                self.status_ok = Some(true);
                self.save_config();
            }
            Err(err) => {
                self.status = err;
                self.status_ok = Some(false);
                // Reflect the real registry state if the write failed.
                self.autostart = autostart::is_enabled();
            }
        }
    }
}

impl eframe::App for PresenceApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        match self.tray.poll_action() {
            Some(crate::tray::TrayAction::Show) => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            Some(crate::tray::TrayAction::Quit) => {
                self.quit_requested = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            None => {}
        }

        // Hide to tray instead of exiting when the window [X] is pressed.
        let close_requested = ctx.input(|i| i.viewport().close_requested());
        if close_requested && self.tray_minimize && !self.quit_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.hide_to_tray();
        }

        let (cover_version, cover_image, status, connected, history) =
            with_state(&self.state, |s| {
                (
                    s.cover_version,
                    s.cover.clone(),
                    s.status_line(),
                    s.discord_connected,
                    s.history.clone(),
                )
            });
        self.track = with_state(&self.state, |s| s.track.clone());
        self.history = history;
        self.status = status;
        self.discord_connected = connected;

        if cover_version != self.last_cover_version {
            self.last_cover_version = cover_version;
            self.cover = cover_image.map(|img| upload_cover(&ctx, &img));
        }

        let playing = self
            .track
            .as_ref()
            .is_some_and(|t| t.status == crate::track::PlaybackStatus::Playing);

        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_space(6.0);

            // --- Header: connection dot, status, theme toggle -----------------
            ui.horizontal(|ui| {
                let (dot, text) = if !self.enabled {
                    (egui::Color32::from_gray(120), "Выключено")
                } else if self.discord_connected {
                    (egui::Color32::from_rgb(80, 200, 120), "Discord: подключён")
                } else {
                    (egui::Color32::from_rgb(230, 160, 60), "Discord: ожидание")
                };
                self.status_ok = Some(self.enabled && self.discord_connected);
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 5.0, dot);
                ui.label(RichText::new(text).weak());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let theme_label = if self.dark_mode { "☀" } else { "🌙" };
                    if ui
                        .small_button(theme_label)
                        .on_hover_text("Сменить тему")
                        .clicked()
                    {
                        self.dark_mode = !self.dark_mode;
                        ctx.set_visuals(if self.dark_mode {
                            egui::Visuals::dark()
                        } else {
                            egui::Visuals::light()
                        });
                        self.save_config();
                    }
                });
            });

            ui.add_space(10.0);

            // --- Track: big cover + info --------------------------------------
            ui.horizontal(|ui| {
                show_cover(ui, self.cover.as_ref(), 132.0);
                ui.add_space(14.0);
                ui.vertical(|ui| match &self.track {
                    Some(track) => {
                        ui.add_space(4.0);
                        ui.label(RichText::new(&track.title).strong().size(19.0));
                        ui.label(RichText::new(&track.artist).size(15.0));
                        if let Some(album) = &track.album {
                            ui.label(RichText::new(album).weak().size(13.0));
                        }
                        ui.add_space(2.0);
                        ui.label(
                            RichText::new(format!("Источник: {}", track.source_app))
                                .weak()
                                .size(11.0),
                        );
                    }
                    None => {
                        ui.add_space(4.0);
                        ui.heading("Музыка не найдена");
                        ui.label(
                            RichText::new("Включите плеер (например YouTube Music в браузере)")
                                .weak(),
                        );
                    }
                });
            });

            // --- Live progress bar --------------------------------------------
            if let Some(track) = &self.track {
                if let Some((position, duration)) = track.live_timeline(unix_now()) {
                    ui.add_space(12.0);
                    let fraction = (position / duration).clamp(0.0, 1.0);
                    ui.add(
                        egui::ProgressBar::new(fraction as f32)
                            .desired_width(ui.available_width())
                            .text(format!(
                                "{} / {}",
                                format_secs(position),
                                format_secs(duration)
                            )),
                    );
                }
            }

            ui.add_space(10.0);
            ui.separator();

            // --- Toggles ------------------------------------------------------
            if ui
                .checkbox(&mut self.enabled, "Показывать в Discord")
                .changed()
            {
                send_command(&self.commands, Command::SetEnabled(self.enabled));
            }
            if ui
                .checkbox(&mut self.autostart, "Запускать вместе с Windows")
                .changed()
            {
                let enable = self.autostart;
                self.toggle_autostart(enable);
            }
            if ui
                .checkbox(&mut self.tray_minimize, "Сворачивать в трей вместо выхода")
                .changed()
            {
                self.save_config();
            }

            ui.add_space(6.0);

            // --- Client ID (collapsed by default once set) --------------------
            let id_set = !self.client_id.trim().is_empty();
            egui::CollapsingHeader::new(RichText::new(if id_set {
                "Client ID (настроен)"
            } else {
                "Client ID (нужно настроить)"
            }))
            .id_salt("client_id_section")
            .default_open(!id_set)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Client ID:");
                    let field = egui::TextEdit::singleline(&mut self.client_id)
                        .hint_text("ID приложения с discord.com/developers");
                    if ui.add(field).lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        self.save_client_id();
                    }
                });
                if ui.small_button("Сохранить").clicked() {
                    self.save_client_id();
                }
                ui.label(
                    RichText::new(
                        "Свой ID — одна минута на discord.com/developers; бот создавать не нужно.",
                    )
                    .weak()
                    .size(11.0),
                );
            });

            ui.add_space(8.0);
            ui.separator();

            // --- Session history ----------------------------------------------
            if !self.history.is_empty() {
                egui::CollapsingHeader::new(RichText::new(format!(
                    "История сессии ({})",
                    self.history.len()
                )))
                .id_salt("history_section")
                .default_open(false)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(96.0)
                        .show(ui, |ui| {
                            for t in self.history.iter().rev() {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("♪").weak());
                                    ui.label(RichText::new(&t.title).size(12.0));
                                    ui.label(
                                        RichText::new(format!("— {}", t.artist)).weak().size(12.0),
                                    );
                                });
                            }
                        });
                });
                ui.add_space(6.0);
            }

            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                ui.label(RichText::new(&self.status).weak().size(11.0));
            });
        });

        // Cover/status arrive from another thread; faster repaint keeps the
        // progress bar ticking when a track is playing.
        let repaint = if playing { 250 } else { 500 };
        ctx.request_repaint_after(Duration::from_millis(repaint));
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn format_secs(secs: f64) -> String {
    let total = secs.round() as u64;
    format!("{}:{:02}", total / 60, total % 60)
}

fn show_cover(ui: &mut egui::Ui, cover: Option<&TextureHandle>, side: f32) {
    let size = egui::vec2(side, side);
    match cover {
        Some(texture) => {
            ui.add(
                egui::Image::from_texture(texture)
                    .fit_to_exact_size(size)
                    .corner_radius(10.0),
            );
        }
        None => {
            let placeholder = egui::Rect::from_min_size(ui.cursor().min, size);
            ui.allocate_rect(placeholder, egui::Sense::hover());
            ui.painter()
                .rect_filled(placeholder, 10.0, egui::Color32::from_gray(60));
            ui.painter().text(
                placeholder.center(),
                egui::Align2::CENTER_CENTER,
                "♪",
                egui::FontId::proportional(40.0),
                egui::Color32::from_gray(150),
            );
        }
    }
}

fn upload_cover(ctx: &egui::Context, img: &crate::app_state::CoverImage) -> TextureHandle {
    ctx.load_texture(
        "cover",
        ColorImage::from_rgba_unmultiplied([img.width as usize, img.height as usize], &img.rgba),
        TextureOptions::default(),
    )
}

pub fn window_options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: ViewportBuilder::default()
            .with_inner_size([480.0, 640.0])
            .with_title("MusicPresence"),
        ..Default::default()
    }
}
