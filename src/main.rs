//! MusicPresence — shows the music you listen to (YouTube Music in a
//! browser, desktop players, ...) as Discord Rich Presence.
//!
//! Source of truth: Windows SMTC. Output: Discord IPC.

// В релизе — GUI-приложение без консольного окна; в отладке консоль
// остаётся, чтобы видеть логи и паники.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod app_state;
mod autostart;
mod artwork;
mod config;
mod discord_presence;
mod poller;
mod smtc_source;
mod timeline;
mod track;

use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use crate::app_state::{send_command, Command, UiState};

fn main() {
    if std::env::args().any(|arg| arg == "--dump") {
        std::process::exit(dump_once());
    }
    if let Err(err) = run_gui() {
        eprintln!("Ошибка: {err}");
        std::process::exit(1);
    }
}

fn run_gui() -> Result<(), String> {
    let cfg = config::load();
    let client_id = cfg.client_id.trim().to_string();

    let state = Arc::new(Mutex::new(UiState {
        presence_enabled: true,
        client_id: client_id.clone(),
        ..UiState::default()
    }));

    let (commands_tx, commands_rx) = mpsc::channel();
    let poller_handle = poller::spawn(state.clone(), commands_rx);
    let ui_commands_tx = commands_tx.clone();

    let options = app::window_options();
    let run_result = eframe::run_native(
        "MusicPresence",
        options,
        Box::new(move |cc| {
            Ok(Box::new(app::PresenceApp::new(
                &cc.egui_ctx,
                state.clone(),
                ui_commands_tx.clone(),
                client_id.clone(),
                true,
                cfg.autostart,
                cfg.dark_mode,
            )))
        }),
    );

    // Window closed: make the poller clear the presence and stop.
    send_command(&commands_tx, Command::Shutdown);
    let _ = poller_handle.join();

    run_result.map_err(|e| format!("GUI: {e}"))
}

/// `--dump`: prints the current SMTC media session as JSON and exits.
/// Used to verify the source end-to-end without the GUI or Discord.
fn dump_once() -> i32 {
    let source = match smtc_source::SmtcSource::new() {
        Ok(source) => source,
        Err(err) => {
            eprintln!("SMTC недоступен: {err}");
            return 1;
        }
    };
    match source.poll() {
        Ok(Some((track, cover_bytes))) => {
            let json = serde_json::json!({
                "title": track.title,
                "artist": track.artist,
                "album": track.album,
                "source_app": track.source_app,
                "status": format!("{:?}", track.status),
                "position_secs": track.position.map(|p| p.as_secs()),
                "duration_secs": track.duration.map(|d| d.as_secs()),
            });
            println!("{json:#}");
            if let Some(bytes) = cover_bytes {
                let path = save_cover_dump(&bytes);
                println!("cover: {path}");
            }
            0
        }
        Ok(None) => {
            println!("нет активных медиасессий SMTC (запустите плеер)");
            0
        }
        Err(err) => {
            eprintln!("опрос SMTC не удался: {err}");
            1
        }
    }
}

fn save_cover_dump(bytes: &[u8]) -> String {
    let ext = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        "png"
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        "jpg"
    } else {
        "bin"
    };
    let mut path = std::env::temp_dir();
    path.push(format!("musicpresence-cover.{ext}"));
    match std::fs::write(&path, bytes) {
        Ok(()) => path.to_string_lossy().into_owned(),
        Err(err) => format!("<не удалось сохранить: {err}>"),
    }
}
