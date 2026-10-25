//! Domain types describing what is playing right now.
//!
//! This module is platform-agnostic: neither SMTC nor Discord specifics
//! leak in here, so all decision logic can be unit tested.

use std::time::Duration;

/// Playback state reported by the media source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackStatus {
    Playing,
    Paused,
    Stopped,
}

/// A snapshot of the currently playing track as reported by the source.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackSnapshot {
    /// Song title (e.g. "Harder, Better, Faster, Stronger").
    pub title: String,
    /// Artist or channel name (e.g. "Daft Punk").
    pub artist: String,
    /// Album name if the source provides one (browsers often do not).
    pub album: Option<String>,
    /// Identifier of the app that owns the media session (raw AUMID).
    pub source_app: String,
    pub status: PlaybackStatus,
    /// Position inside the track, if the source reports a timeline.
    pub position: Option<Duration>,
    /// Total track length, if the source reports one.
    pub duration: Option<Duration>,
    /// Unix seconds when the reported position was accurate.
    pub position_updated_unix: u64,
}

impl TrackSnapshot {
    /// Two snapshots describe the same track when title/artist/album match
    /// (case-insensitive) and at least the title is present.
    pub fn same_track(&self, other: &TrackSnapshot) -> bool {
        eq_ignore_case(&self.title, &other.title)
            && eq_ignore_case(&self.artist, &other.artist)
            && match (&self.album, &other.album) {
                (Some(a), Some(b)) => eq_ignore_case(a, b),
                _ => true,
            }
    }
}

fn eq_ignore_case(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// Discord truncates activity fields around 128 bytes. Cut on a UTF-8
/// character boundary so titles with emoji/cyrillic do not panic.
pub fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    text.chars().take(max_chars.saturating_sub(1)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(title: &str, artist: &str, album: Option<&str>) -> TrackSnapshot {
        TrackSnapshot {
            title: title.into(),
            artist: artist.into(),
            album: album.map(Into::into),
            source_app: "test".into(),
            status: PlaybackStatus::Playing,
            position: None,
            duration: None,
            position_updated_unix: 0,
        }
    }

    #[test]
    fn same_track_matches_case_insensitive() {
        let a = snap("Track", "Artist", Some("Album"));
        let b = snap("TRACK", "artist", Some("ALBUM"));
        assert!(a.same_track(&b));
    }

    #[test]
    fn different_title_is_not_same_track() {
        let a = snap("Track A", "Artist", None);
        let b = snap("Track B", "Artist", None);
        assert!(!a.same_track(&b));
    }

    #[test]
    fn missing_album_still_matches() {
        let a = snap("Track", "Artist", None);
        let b = snap("Track", "Artist", Some("Album"));
        assert!(a.same_track(&b));
    }

    #[test]
    fn truncate_keeps_utf8_boundary() {
        let s = "Привет мир 🙂 ещё текст";
        let t = truncate_chars(s, 10);
        assert!(t.chars().count() <= 10);
        // Result must still be valid UTF-8 (this would panic on a bad cut).
        let _ = t.chars().count();
    }

    #[test]
    fn truncate_noop_for_short() {
        assert_eq!(truncate_chars("abc", 10), "abc");
    }
}
