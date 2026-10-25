//! egui window: current track, cover, activity toggle, Client ID input.

use std::sync::mpsc::Sender;
use std::time::Duration;

use eframe::egui::{self, ColorImage, RichText, TextureHandle, TextureOptions, ViewportBuilder};

use crate::app_state::{send_command, with_state, Command, SharedUiState};
use crate::config;
use crate::track::TrackSnapshot;

pub struct PresenceApp {
    state: SharedUiState,
    commands: Sender<Command>,
    /// UI-owned copy of the Client ID; synced to the poller via commands.
    client_id: String,
    enabled: bool,
    track: Option<TrackSnapshot>,
    cover: Option<TextureHandle>,
    last_cover_version: u64,
    status: String,
    discord_connected: bool,
}

impl PresenceApp {
    pub fn new(
        ctx: &egui::Context,
        state: SharedUiState,
        commands: Sender<Command>,
        client_id: String,
        enabled: bool,
    ) -> Self {
        let (track, cover_version, cover_image) = with_state(&state, |s| {
            (s.track.clone(), s.cover_version, s.cover.clone())
        });
        let cover = cover_image.map(|img| upload_cover(ctx, &img));
        Self {
            state,
            commands,
            client_id,
            enabled,
            track,
            cover,
            last_cover_version: cover_version,
            status: String::new(),
            discord_connected: false,
        }
    }
}

impl eframe::App for PresenceApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let (cover_version, cover_image, status, connected) = with_state(&self.state, |s| {
            (
                s.cover_version,
                s.cover.clone(),
                s.status_line(),
                s.discord_connected,
            )
        });
        self.track = with_state(&self.state, |s| s.track.clone());
        self.status = status;
        self.discord_connected = connected;

        if cover_version != self.last_cover_version {
            self.last_cover_version = cover_version;
            self.cover = cover_image.map(|img| upload_cover(&ctx, &img));
        }

        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                show_cover(ui, self.cover.as_ref());
                ui.add_space(12.0);
                ui.vertical(|ui| match &self.track {
                    Some(track) => {
                        ui.heading(RichText::new(&track.title).strong());
                        ui.label(RichText::new(&track.artist).size(16.0));
                        if let Some(album) = &track.album {
                            ui.label(RichText::new(album).weak().size(14.0));
                        }
                        ui.label(
                            RichText::new(format!("Источник: {}", track.source_app))
                                .weak()
                                .size(12.0),
                        );
                    }
                    None => {
                        ui.heading("Музыка не найдена");
                        ui.label(
                            RichText::new("Включите плеер (например YouTube Music в браузере)")
                                .weak(),
                        );
                    }
                });
            });

            ui.add_space(8.0);
            ui.separator();

            if ui
                .checkbox(&mut self.enabled, "Показывать в Discord")
                .changed()
            {
                send_command(&self.commands, Command::SetEnabled(self.enabled));
            }

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Client ID:");
                let field = egui::TextEdit::singleline(&mut self.client_id)
                    .hint_text("ID приложения с discord.com/developers");
                if ui.add(field).changed() {
                    send_command(
                        &self.commands,
                        Command::SetClientId(self.client_id.trim().to_string()),
                    );
                    let _ = config::save(&config::Config {
                        client_id: self.client_id.trim().to_string(),
                    });
                }
            });

            ui.add_space(8.0);
            ui.separator();
            ui.label(RichText::new(&self.status).weak().size(12.0));
        });

        // Cover/status arrive from another thread.
        ctx.request_repaint_after(Duration::from_millis(500));
    }
}

fn show_cover(ui: &mut egui::Ui, cover: Option<&TextureHandle>) {
    let size = egui::vec2(96.0, 96.0);
    match cover {
        Some(texture) => {
            ui.add(
                egui::Image::from_texture(texture)
                    .fit_to_exact_size(size)
                    .corner_radius(6.0),
            );
        }
        None => {
            let placeholder = egui::Rect::from_min_size(ui.cursor().min, size);
            ui.allocate_rect(placeholder, egui::Sense::hover());
            ui.painter()
                .rect_filled(placeholder, 6.0, egui::Color32::from_gray(60));
            ui.painter().text(
                placeholder.center(),
                egui::Align2::CENTER_CENTER,
                "♪",
                egui::FontId::proportional(32.0),
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
            .with_inner_size([420.0, 320.0])
            .with_title("MusicPresence"),
        ..Default::default()
    }
}
