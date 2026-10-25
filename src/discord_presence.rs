//! Discord Rich Presence client.
//!
//! Wraps `discord-rich-presence` with reconnection/backoff and maps
//! our domain snapshot to a "Listening to ..." activity.

use discord_rich_presence::activity::{self, Activity};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};

use crate::track::{truncate_chars, TrackSnapshot};

/// What the poller wants Discord to display right now.
#[derive(Debug, Clone)]
pub enum Presence {
    /// A track is playing. Timestamps are absent when the source does not
    /// report a timeline (a fake "elapsed" would be misleading).
    Playing {
        track: TrackSnapshot,
        artwork_url: Option<String>,
        start_unix: Option<u64>,
        end_unix: Option<u64>,
    },
    /// A track is loaded but paused.
    Paused {
        track: TrackSnapshot,
        artwork_url: Option<String>,
    },
    /// Nothing to show (no player, stopped, or disabled).
    Hidden,
}

pub struct DiscordPresence {
    client: Option<DiscordIpcClient>,
    client_id: String,
    backoff_secs: u64,
    next_attempt: std::time::Instant,
    /// Whether Discord currently shows something from us.
    active: bool,
}

impl DiscordPresence {
    pub fn new() -> Self {
        Self {
            client: None,
            client_id: String::new(),
            backoff_secs: 1,
            next_attempt: std::time::Instant::now(),
            active: false,
        }
    }

    /// Applies the desired presence. `client_id` changes recreate the
    /// client. Connection failures are retried with growing backoff.
    /// Returns a human-readable error to surface in the UI, if any.
    pub fn apply(&mut self, client_id: &str, desired: &Presence) -> Result<(), String> {
        self.reset_if_client_changed(client_id);

        match desired {
            Presence::Hidden => {
                // Only one clear per visible-state loss; avoids IPC spam.
                if self.active {
                    let result = self.clear();
                    self.backoff_secs = 1;
                    result
                } else {
                    Ok(())
                }
            }
            _ => self.send(desired),
        }
    }

    /// Clears whatever Discord shows. Called on shutdown too.
    pub fn clear(&mut self) -> Result<(), String> {
        self.active = false;
        let Some(client) = self.client.as_mut() else {
            return Ok(());
        };
        client
            .clear_activity()
            .map_err(|e| format!("clear_activity: {e}"))
    }

    /// Best-effort shutdown; errors are ignored by callers.
    pub fn shutdown(&mut self) {
        let _ = self.clear();
        if let Some(client) = self.client.as_mut() {
            let _ = client.close();
        }
        self.client = None;
    }

    pub fn is_connected(&self) -> bool {
        self.client.is_some()
    }

    fn reset_if_client_changed(&mut self, client_id: &str) {
        if self.client_id == client_id {
            return;
        }
        if let Some(mut client) = self.client.take() {
            let _ = client.close();
        }
        self.client_id = client_id.to_string();
        self.backoff_secs = 1;
        self.next_attempt = std::time::Instant::now();
        self.active = false;
    }

    fn send(&mut self, desired: &Presence) -> Result<(), String> {
        self.ensure_connected()?;

        let Some(client) = self.client.as_mut() else {
            return Err("Discord не подключён".into());
        };

        let payload = build_activity(desired);
        client
            .set_activity(payload)
            .map_err(|e| format!("set_activity: {e}"))?;
        self.active = true;
        self.backoff_secs = 1;
        Ok(())
    }

    fn ensure_connected(&mut self) -> Result<(), String> {
        if self.client.is_some() {
            return Ok(());
        }
        if self.client_id.is_empty() {
            return Err("не задан Client ID приложения Discord".into());
        }
        if std::time::Instant::now() < self.next_attempt {
            return Err("ожидание повтора подключения".into());
        }

        let mut client = DiscordIpcClient::new(&self.client_id);
        client.connect().map_err(|e| {
            self.backoff_secs = (self.backoff_secs * 2).min(30);
            self.next_attempt =
                std::time::Instant::now() + std::time::Duration::from_secs(self.backoff_secs);
            format!("connect: {e} (Discord запущен?)")
        })?;
        self.client = Some(client);
        self.backoff_secs = 1;
        Ok(())
    }
}

/// Maps a domain presence to the Discord activity payload.
pub fn build_activity(desired: &Presence) -> Activity<'static> {
    match desired {
        Presence::Hidden => Activity::new(),
        Presence::Paused { track, artwork_url } => {
            let mut a = base_activity(track, artwork_url);
            a = a.state(format!("⏸ Пауза — {}", truncate_chars(&track.artist, 100)));
            a
        }
        Presence::Playing {
            track,
            artwork_url,
            start_unix,
            end_unix,
        } => {
            let mut a = base_activity(track, artwork_url);
            if let Some(start) = start_unix {
                let mut timestamps = activity::Timestamps::new().start(*start as i64);
                if let Some(end) = end_unix {
                    timestamps = timestamps.end(*end as i64);
                }
                a = a.timestamps(timestamps);
            }
            a
        }
    }
}

fn base_activity(track: &TrackSnapshot, artwork_url: &Option<String>) -> Activity<'static> {
    let mut activity = Activity::new()
        .activity_type(activity::ActivityType::Listening)
        .details(truncate_chars(&track.title, 128))
        .state(truncate_chars(&track.artist, 128));

    if let Some(url) = artwork_url {
        activity = activity.assets(activity::Assets::new().large_image(url.clone()).large_text(
            truncate_chars(track.album.as_deref().unwrap_or(&track.title), 128),
        ));
    }
    activity
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::PlaybackStatus;
    use std::time::Duration;

    fn track(status: PlaybackStatus) -> TrackSnapshot {
        TrackSnapshot {
            title: "Aerodynamic".into(),
            artist: "Daft Punk".into(),
            album: Some("Discovery".into()),
            source_app: "chrome.exe".into(),
            status,
            position: Some(Duration::from_secs(30)),
            duration: Some(Duration::from_secs(200)),
            position_updated_unix: 0,
        }
    }

    #[test]
    fn playing_activity_has_timestamps_and_assets() {
        let t = track(PlaybackStatus::Playing);
        let presence = Presence::Playing {
            track: t,
            artwork_url: Some("https://example.com/600x600bb.jpg".into()),
            start_unix: Some(1000),
            end_unix: Some(1200),
        };
        let activity = build_activity(&presence);
        let json = serde_json::to_string(&activity).expect("payload serializes");
        assert!(json.contains("Aerodynamic"));
        assert!(json.contains("https://example.com/600x600bb.jpg"));
        assert!(json.contains("\"start\":1000"));
        assert!(json.contains("\"end\":1200"));
        // Activity type 2 == Listening.
        assert!(json.contains("\"type\":2"));
    }

    #[test]
    fn paused_activity_mentions_pause_without_timestamps() {
        let t = track(PlaybackStatus::Paused);
        let presence = Presence::Paused {
            track: t,
            artwork_url: None,
        };
        let json = serde_json::to_string(&build_activity(&presence)).unwrap();
        assert!(json.contains("Пауза"));
        assert!(!json.contains("timestamps"));
    }

    #[test]
    fn hidden_activity_is_empty() {
        let json = serde_json::to_string(&build_activity(&Presence::Hidden)).unwrap();
        assert!(!json.contains("details"));
    }

    #[test]
    fn playing_without_timeline_has_no_timestamps() {
        let t = track(PlaybackStatus::Playing);
        let presence = Presence::Playing {
            track: t,
            artwork_url: None,
            start_unix: None,
            end_unix: None,
        };
        let json = serde_json::to_string(&build_activity(&presence)).unwrap();
        assert!(json.contains("Aerodynamic"));
        assert!(!json.contains("timestamps"));
    }
}
