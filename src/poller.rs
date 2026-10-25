//! Poller thread: SMTC -> artwork -> Discord -> shared UI state.
//!
//! A single blocking loop; no async runtime. Every tick it reads the
//! newest SMTC session, decides whether Discord's presence must change,
//! and republishes the snapshot for the UI.

use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::app_state::{with_state, Command, CoverImage, SharedUiState};
use crate::artwork::{find_artwork_url, ArtworkCache};
use crate::discord_presence::{DiscordPresence, Presence};
use crate::smtc_source::SmtcSource;
use crate::timeline;
use crate::track::{PlaybackStatus, TrackSnapshot};

const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// A seek differs from the projected position by more than this.
const SEEK_TOLERANCE_SECS: u64 = 3;

/// What we last successfully pushed to Discord, for change detection.
struct LastSent {
    track: TrackSnapshot,
    sent_at: Instant,
    position_at_sent_secs: u64,
}

pub struct Poller {
    state: SharedUiState,
    commands: Receiver<Command>,
    smtc: Option<SmtcSource>,
    agent: ureq::Agent,
    artwork_cache: ArtworkCache,
    presence: DiscordPresence,
    last_sent: Option<LastSent>,
    /// Cover bytes from the most recent SMTC poll.
    pending_cover: Option<Vec<u8>>,
    /// Key of the track whose cover is currently in the UI state.
    cover_track_key: Option<String>,
}

pub fn spawn(state: SharedUiState, commands: Receiver<Command>) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("poller".into())
        .spawn(move || {
            let mut poller = Poller {
                state,
                commands,
                smtc: None,
                agent: ureq::Agent::new_with_config(ureq::config::Config::default()),
                artwork_cache: ArtworkCache::new(),
                presence: DiscordPresence::new(),
                last_sent: None,
                pending_cover: None,
                cover_track_key: None,
            };
            poller.run();
        })
        .expect("spawn poller thread")
}

impl Poller {
    fn run(&mut self) {
        loop {
            match self.commands.recv_timeout(POLL_INTERVAL) {
                Ok(Command::Shutdown) => {
                    self.presence.shutdown();
                    return;
                }
                Ok(Command::SetEnabled(enabled)) => {
                    with_state(&self.state, |s| s.presence_enabled = enabled);
                    self.last_sent = None;
                }
                Ok(Command::SetClientId(id)) => {
                    with_state(&self.state, |s| s.client_id = id.clone());
                    self.last_sent = None;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    self.presence.shutdown();
                    return;
                }
            }
            self.tick();
        }
    }

    fn tick(&mut self) {
        let (enabled, client_id) =
            with_state(&self.state, |s| (s.presence_enabled, s.client_id.clone()));

        let snapshot = self.poll_smtc();
        self.publish_track(snapshot);
        self.refresh_cover_if_track_changed();

        let desired = self.decide_presence(enabled, client_id.as_str());
        if !self.needs_send(&desired) {
            return;
        }

        let artwork_url = match desired {
            Presence::Hidden => None,
            _ => self.artwork_for_current_track(),
        };
        let desired = apply_artwork(desired, artwork_url);

        match self.presence.apply(client_id.as_str(), &desired) {
            Ok(()) => {
                self.record_sent(&desired);
                with_state(&self.state, |s| {
                    s.last_error = None;
                    s.discord_connected = self.presence.is_connected();
                });
            }
            Err(err) => {
                with_state(&self.state, |s| {
                    s.last_error = Some(err);
                    s.discord_connected = self.presence.is_connected();
                });
            }
        }
    }

    /// Reads the newest SMTC session; remembers its cover bytes.
    fn poll_smtc(&mut self) -> Option<TrackSnapshot> {
        if self.smtc.is_none() {
            self.smtc = SmtcSource::new().ok();
        }
        let source = self.smtc.as_ref()?;
        match source.poll() {
            Ok(Some((snapshot, thumb))) => {
                self.pending_cover = thumb;
                Some(snapshot)
            }
            Ok(None) => {
                self.pending_cover = None;
                None
            }
            Err(err) => {
                self.pending_cover = None;
                with_state(&self.state, |s| {
                    s.last_error = Some(format!("SMTC: {err}"));
                });
                None
            }
        }
    }

    fn publish_track(&mut self, snapshot: Option<TrackSnapshot>) {
        with_state(&self.state, |s| s.track = snapshot.clone());
    }

    fn refresh_cover_if_track_changed(&mut self) {
        let key = self.track_key();
        if key == self.cover_track_key {
            return;
        }
        self.cover_track_key = key;
        let cover = self.pending_cover.take().as_deref().and_then(decode_cover);
        with_state(&self.state, |s| {
            s.cover = cover;
            s.cover_version += 1;
        });
    }

    fn track_key(&self) -> Option<String> {
        with_state(&self.state, |s| {
            s.track.as_ref().map(|t| {
                format!(
                    "{}|{}|{}",
                    t.title,
                    t.artist,
                    t.album.as_deref().unwrap_or("")
                )
            })
        })
    }

    fn decide_presence(&self, enabled: bool, client_id: &str) -> Presence {
        if !enabled || client_id.is_empty() {
            return Presence::Hidden;
        }
        let Some(track) = with_state(&self.state, |s| s.track.clone()) else {
            return Presence::Hidden;
        };
        match track.status {
            PlaybackStatus::Stopped => Presence::Hidden,
            PlaybackStatus::Paused => Presence::Paused {
                track,
                artwork_url: None,
            },
            PlaybackStatus::Playing => {
                let now_unix = unix_now();
                let position_secs = track.position.as_ref().map(|p| p.as_secs());
                let duration_secs = track.duration.as_ref().map(|d| d.as_secs());
                let (start_unix, end_unix) =
                    timeline::discord_timestamps(true, position_secs, duration_secs, now_unix)
                        .unwrap_or((now_unix, None));
                let start_unix = position_secs.map(|_| start_unix);
                Presence::Playing {
                    track,
                    artwork_url: None,
                    start_unix,
                    end_unix,
                }
            }
        }
    }

    fn needs_send(&self, desired: &Presence) -> bool {
        let Some(last) = &self.last_sent else {
            return !matches!(desired, Presence::Hidden);
        };
        // A dropped Discord connection must be restored even mid-track.
        let reconnect_needed = !self.presence.is_connected();
        let last_track = &last.track;
        match desired {
            Presence::Hidden => true, // leaving a visible state sends once
            Presence::Playing { track, .. } => {
                if reconnect_needed
                    || !track.same_track(last_track)
                    || track.status != last_track.status
                {
                    return true;
                }
                // Detect seeks: compare the fresh position with what we
                // expect the cursor to be at by now.
                let expected = last.position_at_sent_secs + last.sent_at.elapsed().as_secs();
                let actual = track.position.map_or(0, |p| p.as_secs());
                actual.abs_diff(expected) > SEEK_TOLERANCE_SECS
            }
            Presence::Paused { track, .. } => {
                reconnect_needed
                    || !track.same_track(last_track)
                    || track.status != last_track.status
            }
        }
    }

    fn artwork_for_current_track(&mut self) -> Option<String> {
        let track = with_state(&self.state, |s| s.track.clone())?;
        let album = track.album.as_deref().unwrap_or("");
        if let Some(hit) = self.artwork_cache.get(&track.artist, album, &track.title) {
            return Some(hit.clone());
        }
        let url = find_artwork_url(&self.agent, &track.artist, album, &track.title);
        self.artwork_cache
            .put(&track.artist, album, &track.title, url.clone());
        url
    }

    fn record_sent(&mut self, desired: &Presence) {
        self.last_sent = match desired {
            Presence::Hidden => None,
            Presence::Paused { track, .. } | Presence::Playing { track, .. } => Some(LastSent {
                track: track.clone(),
                sent_at: Instant::now(),
                position_at_sent_secs: track.position.map_or(0, |p| p.as_secs()),
            }),
        };
    }
}

fn apply_artwork(desired: Presence, artwork_url: Option<String>) -> Presence {
    match desired {
        Presence::Paused { track, .. } => Presence::Paused { track, artwork_url },
        Presence::Playing {
            track,
            start_unix,
            end_unix,
            ..
        } => Presence::Playing {
            track,
            artwork_url,
            start_unix,
            end_unix,
        },
        other => other,
    }
}

fn decode_cover(bytes: &[u8]) -> Option<CoverImage> {
    let img = image::load_from_memory(bytes).ok()?;
    let img = img.thumbnail(256, 256);
    let width = img.width();
    let height = img.height();
    let rgba = img.to_rgba8().into_raw();
    Some(CoverImage {
        width,
        height,
        rgba,
    })
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
