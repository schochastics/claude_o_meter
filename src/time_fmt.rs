//! Humanize durations as "2h 14m", "47m", "12s", "now".

use chrono::{DateTime, Utc};
use std::time::Duration;

pub fn humanize(d: Duration) -> String {
    let total = d.as_secs();
    if total == 0 {
        return "now".to_string();
    }
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{h}h {m:02}m")
    } else if m > 0 {
        format!("{m}m")
    } else {
        format!("{s}s")
    }
}

/// "resets in 2h 14m" or "resets soon" if already past.
pub fn resets_in(target: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let delta = target.signed_duration_since(now);
    if delta.num_seconds() <= 0 {
        return "resets soon".to_string();
    }
    let secs = delta.num_seconds() as u64;
    format!("resets in {}", humanize(Duration::from_secs(secs)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn hours_and_minutes() {
        assert_eq!(humanize(Duration::from_secs(2 * 3600 + 14 * 60)), "2h 14m");
    }

    #[test]
    fn minutes_only() {
        assert_eq!(humanize(Duration::from_secs(47 * 60)), "47m");
    }

    #[test]
    fn seconds_only() {
        assert_eq!(humanize(Duration::from_secs(12)), "12s");
    }

    #[test]
    fn zero_is_now() {
        assert_eq!(humanize(Duration::from_secs(0)), "now");
    }

    #[test]
    fn resets_in_future() {
        let now = Utc.with_ymd_and_hms(2026, 5, 19, 12, 0, 0).unwrap();
        let target = Utc.with_ymd_and_hms(2026, 5, 19, 14, 14, 0).unwrap();
        assert_eq!(resets_in(target, now), "resets in 2h 14m");
    }

    #[test]
    fn resets_in_past() {
        let now = Utc.with_ymd_and_hms(2026, 5, 19, 12, 0, 0).unwrap();
        let target = Utc.with_ymd_and_hms(2026, 5, 19, 11, 0, 0).unwrap();
        assert_eq!(resets_in(target, now), "resets soon");
    }
}
