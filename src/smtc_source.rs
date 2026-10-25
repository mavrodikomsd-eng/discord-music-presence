//! Windows SMTC (System Media Transport Controls) source.
//!
//! Reads what media players publish to the system: browsers with
//! YouTube Music, desktop players, Spotify, etc. This is the only
//! Windows-API module; everything it produces is a [`TrackSnapshot`].

use std::time::Duration;

use windows::core::Interface;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession, GlobalSystemMediaTransportControlsSessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus,
};

use crate::timeline;
use crate::track::{PlaybackStatus, TrackSnapshot};

/// Blocks until a WinRT async operation completes.
fn block_on_op<T, F>(op: windows::core::Result<F>) -> windows::core::Result<T>
where
    F: std::future::IntoFuture<Output = windows::core::Result<T>>,
{
    pollster::block_on(op?.into_future())
}

pub struct SmtcSource {
    manager: GlobalSystemMediaTransportControlsSessionManager,
}

impl SmtcSource {
    pub fn new() -> windows::core::Result<Self> {
        let manager =
            block_on_op(GlobalSystemMediaTransportControlsSessionManager::RequestAsync())?;
        Ok(Self { manager })
    }

    /// Snapshots the most relevant media session, if any.
    /// Returns the snapshot together with the raw cover bytes.
    pub fn poll(&self) -> windows::core::Result<Option<(TrackSnapshot, Option<Vec<u8>>)>> {
        let sessions = self.manager.GetSessions()?;
        let count = sessions.Size()?;

        let mut candidates: Vec<GlobalSystemMediaTransportControlsSession> = Vec::new();
        for i in 0..count {
            candidates.push(sessions.GetAt(i)?);
        }

        // Prefer a playing session; otherwise keep the last non-closed one.
        let session = candidates
            .iter()
            .find(|s| status_of(s) == Ok(Some(PlaybackStatus::Playing)))
            .or_else(|| candidates.iter().find(|s| status_of(s).is_ok()))
            .cloned();

        let Some(session) = session else {
            return Ok(None);
        };

        Ok(self.read_snapshot(&session))
    }

    fn read_snapshot(
        &self,
        session: &GlobalSystemMediaTransportControlsSession,
    ) -> Option<(TrackSnapshot, Option<Vec<u8>>)> {
        let source_app = session.SourceAppUserModelId().ok()?.to_string();

        let playback_info = session.GetPlaybackInfo().ok()?;
        let status = playback_info.PlaybackStatus().ok().and_then(map_status)?;
        let rate = playback_info
            .PlaybackRate()
            .ok()
            .and_then(|r| r.Value().ok())
            .unwrap_or(1.0);

        let props = block_on_op(session.TryGetMediaPropertiesAsync()).ok()?;
        let title = props.Title().map(|h| h.to_string()).unwrap_or_default();
        let artist = props.Artist().map(|h| h.to_string()).unwrap_or_default();
        let album = props.AlbumTitle().map(|h| h.to_string()).ok();
        let album = non_empty(album);
        let thumbnail = read_thumbnail_bytes(&props);

        let timeline_props = session.GetTimelineProperties().ok();

        let end_ticks = timeline_props
            .as_ref()
            .and_then(|t| t.EndTime().ok())
            .map(|ts| ts.Duration);

        // While paused the reported position must not keep advancing.
        let effective_rate = if status == PlaybackStatus::Playing {
            rate
        } else {
            0.0
        };

        let (position, position_updated_unix) = match &timeline_props {
            Some(t) => match (t.Position(), t.LastUpdatedTime()) {
                (Ok(raw), Ok(last_updated)) => {
                    let now_ticks = now_filetime_ticks();
                    let resolved = timeline::resolve_position_ticks(
                        raw.Duration,
                        end_ticks,
                        last_updated.UniversalTime,
                        now_ticks,
                        effective_rate,
                    );
                    match resolved {
                        Some(projected) => (
                            Some(Duration::from_secs((projected / 10_000_000).max(0) as u64)),
                            timeline::filetime_ticks_to_unix_seconds(now_ticks),
                        ),
                        // Unreliable timeline: no position beats a wrong one
                        // (a bogus value would reset the Discord timer).
                        None => (None, 0),
                    }
                }
                _ => (None, 0),
            },
            None => (None, 0),
        };

        let duration = end_ticks
            .and_then(timeline::duration_ticks_to_seconds)
            .map(Duration::from_secs);

        Some((
            TrackSnapshot {
                title,
                artist,
                album,
                source_app,
                status,
                position,
                duration,
                position_updated_unix,
            },
            thumbnail,
        ))
    }
}

/// Current wall clock as FILETIME ticks (the `LastUpdatedTime` axis):
/// 100-ns intervals since 1601-01-01 UTC.
pub fn now_filetime_ticks() -> i64 {
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    unix.as_secs() as i64 * 10_000_000
        + unix.subsec_nanos() as i64 / 100
        + timeline::FILETIME_OFFSET_TICKS
}

fn non_empty(value: Option<String>) -> Option<String> {
    match value {
        Some(v) if !v.trim().is_empty() => Some(v),
        _ => None,
    }
}

fn status_of(
    session: &GlobalSystemMediaTransportControlsSession,
) -> windows::core::Result<Option<PlaybackStatus>> {
    let raw = session.GetPlaybackInfo()?.PlaybackStatus()?;
    Ok(map_status(raw))
}

/// Maps the WinRT enum to our domain value. Pure, unit-testable.
pub fn map_status(
    raw: GlobalSystemMediaTransportControlsSessionPlaybackStatus,
) -> Option<PlaybackStatus> {
    match raw.0 {
        4 => Some(PlaybackStatus::Playing),
        5 => Some(PlaybackStatus::Paused),
        0..=3 => Some(PlaybackStatus::Stopped),
        _ => None,
    }
}

/// Reads the raw thumbnail bytes from media properties, capped to 4 MB.
fn read_thumbnail_bytes(
    props: &windows::Media::Control::GlobalSystemMediaTransportControlsSessionMediaProperties,
) -> Option<Vec<u8>> {
    const MAX_BYTES: u32 = 4 * 1024 * 1024;
    let reference = props.Thumbnail().ok()?;
    let stream = block_on_op(reference.OpenReadAsync()).ok()?;
    let size = stream.Size().ok()? as u32;
    if size == 0 || size > MAX_BYTES {
        return None;
    }
    let input = stream
        .cast::<windows::Storage::Streams::IInputStream>()
        .ok()?;
    let reader = windows::Storage::Streams::DataReader::CreateDataReader(&input).ok()?;
    block_on_op(reader.LoadAsync(size)).ok()?;
    let mut bytes = vec![0u8; size as usize];
    reader.ReadBytes(&mut bytes).ok()?;
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Media::Control::GlobalSystemMediaTransportControlsSessionPlaybackStatus;

    #[test]
    fn status_mapping() {
        assert_eq!(
            map_status(GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing),
            Some(PlaybackStatus::Playing)
        );
        assert_eq!(
            map_status(GlobalSystemMediaTransportControlsSessionPlaybackStatus::Paused),
            Some(PlaybackStatus::Paused)
        );
        assert_eq!(
            map_status(GlobalSystemMediaTransportControlsSessionPlaybackStatus::Stopped),
            Some(PlaybackStatus::Stopped)
        );
        assert_eq!(
            map_status(GlobalSystemMediaTransportControlsSessionPlaybackStatus::Closed),
            Some(PlaybackStatus::Stopped)
        );
    }

    #[test]
    fn now_filetime_ticks_close_to_unix_now() {
        let unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let converted = timeline::filetime_ticks_to_unix_seconds(now_filetime_ticks());
        assert!((unix as i64 - converted as i64).abs() <= 2);
    }
}
