//! Shared state between the poller thread and the UI thread,
//! plus the command channel used to steer the poller.

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use crate::track::TrackSnapshot;

/// Decoded cover ready for the UI to upload into a texture.
#[derive(Debug, Clone)]
pub struct CoverImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Everything the UI renders. Written by the poller, read by egui.
#[derive(Default)]
pub struct UiState {
    pub track: Option<TrackSnapshot>,
    pub cover: Option<CoverImage>,
    /// Bumps whenever `cover` changes so the UI re-uploads the texture.
    pub cover_version: u64,
    pub presence_enabled: bool,
    pub client_id: String,
    pub discord_connected: bool,
    pub last_error: Option<String>,
}

impl UiState {
    pub fn status_line(&self) -> String {
        if !self.presence_enabled {
            return "Выключено".into();
        }
        match &self.last_error {
            Some(err) => format!("Discord: {err}"),
            None if self.discord_connected => "Discord: подключён".into(),
            None => "Discord: ожидание".into(),
        }
    }
}

pub type SharedUiState = Arc<Mutex<UiState>>;

/// UI -> poller commands.
#[derive(Debug)]
pub enum Command {
    SetEnabled(bool),
    SetClientId(String),
    Shutdown,
}

/// Locks the state and runs `f` with its contents.
pub fn with_state<T>(state: &SharedUiState, f: impl FnOnce(&mut UiState) -> T) -> T {
    let mut guard = state.lock().expect("ui state poisoned");
    f(&mut guard)
}

pub fn send_command(tx: &Sender<Command>, command: Command) {
    // The UI outlives the poller in normal operation; a dropped receiver
    // only happens during teardown.
    let _ = tx.send(command);
}
