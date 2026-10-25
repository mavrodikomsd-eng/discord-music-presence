//! Pure timeline math: SMTC tick values -> Discord timestamps.
//!
//! Kept free of `windows` types so it can be unit tested directly.
//! All inputs are raw 100-nanosecond ticks. `TimeSpan` fields (Position,
//! EndTime) are plain intervals without an epoch; `DateTime.UniversalTime`
//! (`LastUpdatedTime`) counts from the FILETIME epoch 1601-01-01 UTC —
//! *not* from the .NET year-0001 epoch.

/// FILETIME epoch offset: ticks between 1601-01-01 and 1970-01-01 (UTC).
pub const FILETIME_OFFSET_TICKS: i64 = 116_444_736_000_000_000;

/// `LastUpdatedTime` earlier than this (FILETIME of 2000-01-01) is garbage.
pub const MIN_VALID_TIMESTAMP_TICKS: i64 = 125_911_584_000_000_000;

/// Tolerated clock skew: `LastUpdatedTime` may be up to 5 s in the future.
pub const FUTURE_SKEW_TICKS: i64 = 50_000_000;

/// A `LastUpdatedTime` older than 24 h is stale, not a live position.
pub const MAX_UPDATE_AGE_TICKS: i64 = 864_000_000_000;

const TICKS_PER_SECOND: i64 = 10_000_000;

/// Converts FILETIME ticks (as in `DateTime.UniversalTime`) to Unix seconds.
pub fn filetime_ticks_to_unix_seconds(ticks: i64) -> u64 {
    let unix_ticks = ticks - FILETIME_OFFSET_TICKS;
    if unix_ticks <= 0 {
        return 0;
    }
    (unix_ticks / TICKS_PER_SECOND) as u64
}

/// Converts an `EndTime` tick count into whole seconds.
/// Non-positive ticks mean "unknown duration".
pub fn duration_ticks_to_seconds(ticks: i64) -> Option<u64> {
    if ticks <= 0 {
        return None;
    }
    Some((ticks / TICKS_PER_SECOND).max(0) as u64)
}

/// Projects the position a playback cursor should be at "now",
/// given the last reported position and its timestamp.
///
/// `rate` below 0.05 (e.g. unknown/zero) is treated as normal speed.
pub fn projected_position_ticks(
    position_ticks: i64,
    last_updated_ticks: i64,
    now_ticks: i64,
    rate: f64,
) -> i64 {
    if now_ticks <= last_updated_ticks {
        return position_ticks.max(0);
    }
    let elapsed_ticks = now_ticks - last_updated_ticks;
    let rate = if rate < 0.05 { 1.0 } else { rate };
    (position_ticks as f64 + elapsed_ticks as f64 * rate) as i64
}

/// Validates SMTC timeline data and projects the live position.
///
/// Returns `None` instead of a wrong value whenever anything is unreliable:
/// - negative position,
/// - position beyond the track end (when a positive `EndTime` is known),
/// - `LastUpdatedTime` that is missing, too old, or from a wrong epoch,
/// - a projection that would run past the track end.
///
/// `effective_rate <= 0` means playback is frozen (paused): the reported
/// position is returned as-is without projection.
pub fn resolve_position_ticks(
    position_ticks: i64,
    end_ticks: Option<i64>,
    last_updated_ticks: i64,
    now_ticks: i64,
    effective_rate: f64,
) -> Option<i64> {
    let end = end_ticks.filter(|ticks| *ticks > 0);
    if position_ticks < 0 {
        return None;
    }
    if let Some(end) = end {
        if position_ticks > end {
            return None;
        }
    }
    if last_updated_ticks < MIN_VALID_TIMESTAMP_TICKS {
        return None;
    }
    let elapsed_ticks = now_ticks - last_updated_ticks;
    if !(-FUTURE_SKEW_TICKS..=MAX_UPDATE_AGE_TICKS).contains(&elapsed_ticks) {
        return None;
    }
    let projected = if effective_rate <= 0.0 {
        position_ticks
    } else {
        projected_position_ticks(
            position_ticks,
            last_updated_ticks,
            now_ticks,
            effective_rate,
        )
    };
    if projected < 0 {
        return None;
    }
    match end {
        Some(end) if projected > end => None,
        _ => Some(projected),
    }
}

/// Builds Discord `start`/`end` unix-seconds for a track.
///
/// Returns `None` while paused/stopped (Discord would otherwise keep
/// counting down a frozen timer) or when the position is unknown.
/// `end` is skipped when duration is unknown.
pub fn discord_timestamps(
    playing: bool,
    position_secs: Option<u64>,
    duration_secs: Option<u64>,
    now_unix: u64,
) -> Option<(u64, Option<u64>)> {
    if !playing {
        return None;
    }
    let position_secs = position_secs?;
    let start = now_unix.saturating_sub(position_secs);
    let end = duration_secs
        .filter(|d| *d > 0)
        .map(|d| start.saturating_add(d));
    Some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEC: i64 = TICKS_PER_SECOND;

    #[test]
    fn filetime_offset_matches_windows_epoch() {
        // 11 644 473 600 s between 1601-01-01 and 1970-01-01. If this
        // drifts, LastUpdatedTime math breaks like the original bug did
        // (50 491 123 220 "seconds" = the .NET-vs-FILETIME axis gap).
        assert_eq!(FILETIME_OFFSET_TICKS / SEC, 11_644_473_600);
        assert_eq!(MIN_VALID_TIMESTAMP_TICKS / SEC, 12_591_158_400);
    }

    #[test]
    fn filetime_ticks_convert_to_unix_seconds() {
        // 2026-01-01T00:00:00Z == 1767225600 unix seconds.
        let ticks = 1_767_225_600i64 * SEC + FILETIME_OFFSET_TICKS;
        assert_eq!(filetime_ticks_to_unix_seconds(ticks), 1_767_225_600);
    }

    #[test]
    fn filetime_ticks_before_epoch_clamp_to_zero() {
        assert_eq!(filetime_ticks_to_unix_seconds(0), 0);
    }

    #[test]
    fn position_advances_with_rate() {
        // Position 10s, reported 5s ago, rate 2.0 -> 20s.
        let now = 1_000 * SEC;
        let out = projected_position_ticks(10 * SEC, now - 5 * SEC, now, 2.0);
        assert_eq!(out, 20 * SEC);
    }

    #[test]
    fn zero_rate_means_normal_speed() {
        let now = 1_000 * SEC;
        let out = projected_position_ticks(10 * SEC, now - 5 * SEC, now, 0.0);
        assert_eq!(out, 15 * SEC);
    }

    #[test]
    fn clock_regression_returns_reported_position() {
        let now = 1_000 * SEC;
        let out = projected_position_ticks(10 * SEC, now + 5 * SEC, now, 1.0);
        assert_eq!(out, 10 * SEC);
    }

    // --- resolve_position_ticks ---

    const T2026: i64 = 1_767_225_600; // 2026-01-01T00:00:00Z, unix seconds

    fn ft(unix_secs: i64) -> i64 {
        unix_secs * SEC + FILETIME_OFFSET_TICKS
    }

    #[test]
    fn resolve_projects_position_forward() {
        // Reported 20 s of a 218 s track, 3 s ago, playing -> 23 s.
        let out = resolve_position_ticks(20 * SEC, Some(218 * SEC), ft(T2026 - 3), ft(T2026), 1.0);
        assert_eq!(out, Some(23 * SEC));
    }

    #[test]
    fn resolve_tolerates_small_clock_skew() {
        // LastUpdatedTime 2 s in the future: trust the reported position.
        let out = resolve_position_ticks(20 * SEC, Some(218 * SEC), ft(T2026 + 2), ft(T2026), 1.0);
        assert_eq!(out, Some(20 * SEC));
    }

    #[test]
    fn resolve_rejects_negative_position() {
        let out = resolve_position_ticks(-SEC, Some(218 * SEC), ft(T2026), ft(T2026), 1.0);
        assert_eq!(out, None);
    }

    #[test]
    fn resolve_rejects_position_beyond_end() {
        let out = resolve_position_ticks(300 * SEC, Some(218 * SEC), ft(T2026 - 3), ft(T2026), 1.0);
        assert_eq!(out, None);
    }

    #[test]
    fn resolve_rejects_missing_last_updated() {
        let out = resolve_position_ticks(20 * SEC, Some(218 * SEC), 0, ft(T2026), 1.0);
        assert_eq!(out, None);
    }

    #[test]
    fn resolve_rejects_stale_last_updated() {
        // 25 hours old: not a live position.
        let out = resolve_position_ticks(
            20 * SEC,
            Some(218 * SEC),
            ft(T2026 - 25 * 3600),
            ft(T2026),
            1.0,
        );
        assert_eq!(out, None);
    }

    #[test]
    fn resolve_rejects_wrong_epoch_timestamp() {
        // A .NET-axis value (ticks from year 0001) sits ~1970 years in the
        // future on the FILETIME axis: the exact bug that produced
        // 50 491 123 220 "seconds".
        let net_axis_now = 621_355_968_000_000_000;
        let out = resolve_position_ticks(20 * SEC, Some(218 * SEC), net_axis_now, ft(T2026), 1.0);
        assert_eq!(out, None);
    }

    #[test]
    fn resolve_rejects_projection_past_end() {
        // 216 s + 10 s of playback = 226 s > 218 s end: cannot be true.
        let out =
            resolve_position_ticks(216 * SEC, Some(218 * SEC), ft(T2026 - 10), ft(T2026), 1.0);
        assert_eq!(out, None);
    }

    #[test]
    fn resolve_frozen_position_when_rate_zero() {
        // Paused (rate 0): the position must not advance.
        let out = resolve_position_ticks(20 * SEC, Some(218 * SEC), ft(T2026 - 3), ft(T2026), 0.0);
        assert_eq!(out, Some(20 * SEC));
    }

    #[test]
    fn resolve_allows_unknown_end() {
        let out = resolve_position_ticks(20 * SEC, None, ft(T2026 - 3), ft(T2026), 1.0);
        assert_eq!(out, Some(23 * SEC));
    }

    // --- duration_ticks_to_seconds ---

    #[test]
    fn duration_ticks_convert_to_seconds() {
        assert_eq!(duration_ticks_to_seconds(218 * SEC), Some(218));
        assert_eq!(duration_ticks_to_seconds(0), None);
        assert_eq!(duration_ticks_to_seconds(-5), None);
    }

    #[test]
    fn no_timestamps_when_paused() {
        assert_eq!(discord_timestamps(false, Some(1), Some(1), 100), None);
    }

    #[test]
    fn no_timestamps_without_position() {
        assert_eq!(discord_timestamps(true, None, Some(1), 100), None);
    }

    #[test]
    fn start_is_now_minus_position() {
        let out = discord_timestamps(true, Some(50), Some(100), 200);
        assert_eq!(out, Some((150, Some(250))));
    }

    #[test]
    fn unknown_duration_gives_only_start() {
        let out = discord_timestamps(true, Some(50), None, 200);
        assert_eq!(out, Some((150, None)));
    }

    #[test]
    fn zero_duration_is_ignored() {
        let out = discord_timestamps(true, Some(50), Some(0), 200);
        assert_eq!(out, Some((150, None)));
    }
}
