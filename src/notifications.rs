//! Threshold-based notifications.
//!
//! Tracks which thresholds have already fired for each rolling window and
//! re-arms them when the window's `resets_at` changes.

use crate::api::UsageResponse;
use chrono::{DateTime, Utc};

const DEFAULT_THRESHOLDS: &[f64] = &[0.75, 0.90, 0.95];

#[derive(Debug, Clone, PartialEq)]
pub struct Notification {
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone)]
struct WindowState {
    fired: Vec<bool>,
    reset_at: Option<DateTime<Utc>>,
}

impl WindowState {
    fn new(n: usize) -> Self {
        Self {
            fired: vec![false; n],
            reset_at: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ThresholdTracker {
    thresholds: Vec<f64>,
    session: WindowState,
    weekly: WindowState,
}

impl Default for ThresholdTracker {
    fn default() -> Self {
        Self::new(DEFAULT_THRESHOLDS.to_vec())
    }
}

impl ThresholdTracker {
    pub fn new(thresholds: Vec<f64>) -> Self {
        let n = thresholds.len();
        Self {
            thresholds,
            session: WindowState::new(n),
            weekly: WindowState::new(n),
        }
    }

    pub fn observe(&mut self, u: &UsageResponse) -> Vec<Notification> {
        let mut out = Vec::new();
        if let Some(w) = u.five_hour.as_ref() {
            crossings(
                &self.thresholds,
                &mut self.session,
                w.utilization,
                w.resets_at,
                "Session",
                &mut out,
            );
        }
        if let Some(w) = u.seven_day.as_ref() {
            crossings(
                &self.thresholds,
                &mut self.weekly,
                w.utilization,
                w.resets_at,
                "Weekly",
                &mut out,
            );
        }
        out
    }
}

fn crossings(
    thresholds: &[f64],
    state: &mut WindowState,
    utilization: f64,
    reset_at: Option<DateTime<Utc>>,
    label: &str,
    out: &mut Vec<Notification>,
) {
    if state.reset_at != reset_at {
        state.reset_at = reset_at;
        for slot in state.fired.iter_mut() {
            *slot = false;
        }
    }
    for (i, &t) in thresholds.iter().enumerate() {
        if utilization >= t && !state.fired[i] {
            state.fired[i] = true;
            out.push(Notification {
                title: format!("Claude {label} quota at {}%", (t * 100.0).round() as u32),
                body: format!("Current usage: {:.0}%", (utilization * 100.0).round()),
            });
        }
    }
}

/// Minimum elapsed time (minutes) between two samples for a rate to be trusted.
/// Guards against zero/negative/duplicate timestamps producing infinite rates.
const SPIKE_MIN_DT_MIN: f64 = 0.5;

/// Re-arm once the climb rate falls below `threshold * REARM_FACTOR`, so a
/// sustained-but-decelerating climb doesn't re-fire every poll.
const SPIKE_REARM_FACTOR: f64 = 0.5;

#[derive(Debug, Clone)]
struct SpikeSample {
    utilization: f64,
    at: DateTime<Utc>,
    reset_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default)]
struct SpikeWindow {
    prev: Option<SpikeSample>,
    armed: bool,
    last_rate: f64,
}

impl SpikeWindow {
    fn new() -> Self {
        Self {
            prev: None,
            armed: true,
            last_rate: 0.0,
        }
    }
}

/// Velocity detector: warns when usage climbs abnormally fast within a short
/// time — the symptom of a runaway agent or a costly infinite loop. Distinct
/// from [`ThresholdTracker`], which fires on absolute levels, not rate of
/// change. Tracks the session window (and optionally the 7-day window).
#[derive(Debug, Clone)]
pub struct SpikeTracker {
    threshold_per_min: f64,
    rearm_factor: f64,
    track_weekly: bool,
    session: SpikeWindow,
    weekly: SpikeWindow,
}

impl SpikeTracker {
    pub fn new(threshold_per_min: f64, track_weekly: bool) -> Self {
        Self {
            threshold_per_min,
            rearm_factor: SPIKE_REARM_FACTOR,
            track_weekly,
            session: SpikeWindow::new(),
            weekly: SpikeWindow::new(),
        }
    }

    /// Observe a sample at wall-clock `now`. Returns notifications to dispatch.
    pub fn observe(&mut self, u: &UsageResponse, now: DateTime<Utc>) -> Vec<Notification> {
        let mut out = Vec::new();
        if let Some(w) = u.five_hour.as_ref() {
            check_spike(
                self.threshold_per_min,
                self.rearm_factor,
                &mut self.session,
                w.utilization,
                w.resets_at,
                now,
                "session",
                &mut out,
            );
        }
        if self.track_weekly
            && let Some(w) = u.seven_day.as_ref()
        {
            check_spike(
                self.threshold_per_min,
                self.rearm_factor,
                &mut self.weekly,
                w.utilization,
                w.resets_at,
                now,
                "weekly",
                &mut out,
            );
        }
        out
    }

    /// Whether the session window is currently climbing at or above the spike
    /// threshold. Drives the tray-icon alarm badge; independent of the
    /// fire-once notification cooldown, so the badge persists while the climb
    /// continues and clears once it decelerates.
    pub fn session_spiking(&self) -> bool {
        self.session.last_rate >= self.threshold_per_min
    }
}

#[allow(clippy::too_many_arguments)]
fn check_spike(
    threshold: f64,
    rearm_factor: f64,
    state: &mut SpikeWindow,
    utilization: f64,
    reset_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    label: &str,
    out: &mut Vec<Notification>,
) {
    // A NaN/inf sample must never become a baseline or it poisons the next delta.
    if !utilization.is_finite() {
        state.last_rate = 0.0;
        return;
    }

    let sample = SpikeSample {
        utilization,
        at: now,
        reset_at,
    };

    let Some(prev) = state.prev.as_ref() else {
        // First sample: establish a baseline, never fire.
        state.last_rate = 0.0;
        state.prev = Some(sample);
        return;
    };

    // Window reset: never alert across the boundary; re-arm with a fresh baseline.
    if prev.reset_at != reset_at {
        state.last_rate = 0.0;
        state.armed = true;
        state.prev = Some(sample);
        return;
    }

    let dt_min = (now - prev.at).num_seconds() as f64 / 60.0;
    if dt_min <= SPIKE_MIN_DT_MIN {
        // Too close together (or non-monotonic clock): advance baseline, no rate.
        state.prev = Some(sample);
        return;
    }

    let delta = utilization - prev.utilization;
    if delta <= 0.0 {
        // Flat or falling usage is never a spike; re-arm.
        state.last_rate = 0.0;
        state.armed = true;
        state.prev = Some(sample);
        return;
    }

    let rate = delta / dt_min;
    if !rate.is_finite() {
        state.prev = Some(sample);
        return;
    }
    state.last_rate = rate;
    state.prev = Some(sample);

    if rate < threshold * rearm_factor {
        // Decelerated below the hysteresis floor: re-arm for the next episode.
        state.armed = true;
    }

    if state.armed && rate >= threshold {
        state.armed = false;
        out.push(Notification {
            title: format!("Claude {label} usage spiking"),
            body: format!("+{:.1}%/min — possible runaway", rate * 100.0),
        });
    }
}

/// Dispatch a notification via mac-notification-sys. No-op (logged) on failure;
/// notifications without an ad-hoc-signed bundle silently drop on macOS 14+.
pub fn dispatch(n: &Notification) {
    if let Err(e) = mac_notification_sys::send_notification(&n.title, None, &n.body, None) {
        tracing::warn!(error = %e, "notification dispatch failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Window;
    use chrono::TimeZone;
    use std::collections::BTreeMap;

    fn resp(session: Option<f64>, weekly: Option<f64>, reset: DateTime<Utc>) -> UsageResponse {
        UsageResponse {
            five_hour: session.map(|u| Window {
                utilization: u,
                resets_at: Some(reset),
            }),
            seven_day: weekly.map(|u| Window {
                utilization: u,
                resets_at: Some(reset),
            }),
            extra: BTreeMap::new(),
        }
    }

    #[test]
    fn fires_each_threshold_once_per_window() {
        let mut t = ThresholdTracker::default();
        let r = Utc.with_ymd_and_hms(2026, 5, 19, 17, 0, 0).unwrap();

        assert_eq!(t.observe(&resp(Some(0.50), None, r)).len(), 0);
        let n1 = t.observe(&resp(Some(0.76), None, r));
        assert_eq!(n1.len(), 1);
        assert!(n1[0].title.contains("75"));

        // Same window, same level — no re-fire.
        let n2 = t.observe(&resp(Some(0.80), None, r));
        assert_eq!(n2.len(), 0);

        let n3 = t.observe(&resp(Some(0.91), None, r));
        assert_eq!(n3.len(), 1);
        assert!(n3[0].title.contains("90"));

        let n4 = t.observe(&resp(Some(0.97), None, r));
        assert_eq!(n4.len(), 1);
        assert!(n4[0].title.contains("95"));
    }

    #[test]
    fn rearms_after_reset() {
        let mut t = ThresholdTracker::default();
        let r1 = Utc.with_ymd_and_hms(2026, 5, 19, 17, 0, 0).unwrap();
        let r2 = Utc.with_ymd_and_hms(2026, 5, 19, 22, 0, 0).unwrap();

        let _ = t.observe(&resp(Some(0.99), None, r1));
        let n = t.observe(&resp(Some(0.99), None, r2));
        assert_eq!(
            n.len(),
            3,
            "all three thresholds should re-arm at new window"
        );
    }

    #[test]
    fn jump_past_multiple_thresholds() {
        let mut t = ThresholdTracker::default();
        let r = Utc.with_ymd_and_hms(2026, 5, 19, 17, 0, 0).unwrap();
        let n = t.observe(&resp(Some(0.99), None, r));
        assert_eq!(n.len(), 3);
    }

    #[test]
    fn session_and_weekly_tracked_independently() {
        let mut t = ThresholdTracker::default();
        let r = Utc.with_ymd_and_hms(2026, 5, 19, 17, 0, 0).unwrap();
        let n = t.observe(&resp(Some(0.80), Some(0.80), r));
        assert_eq!(n.len(), 2);
        let session_titles: Vec<_> = n.iter().filter(|x| x.title.contains("Session")).collect();
        let weekly_titles: Vec<_> = n.iter().filter(|x| x.title.contains("Weekly")).collect();
        assert_eq!(session_titles.len(), 1);
        assert_eq!(weekly_titles.len(), 1);
    }

    #[test]
    fn empty_thresholds_never_fires() {
        let mut t = ThresholdTracker::new(Vec::new());
        let r = Utc.with_ymd_and_hms(2026, 5, 19, 17, 0, 0).unwrap();
        assert_eq!(t.observe(&resp(Some(0.99), Some(0.99), r)).len(), 0);
    }

    #[test]
    fn unsorted_thresholds_each_fire_once() {
        let mut t = ThresholdTracker::new(vec![0.95, 0.75, 0.90]);
        let r = Utc.with_ymd_and_hms(2026, 5, 19, 17, 0, 0).unwrap();
        // Jumping straight to 0.99 should fire all three exactly once.
        let n = t.observe(&resp(Some(0.99), None, r));
        assert_eq!(n.len(), 3);
        // No re-fire on subsequent observations within the same window.
        assert_eq!(t.observe(&resp(Some(1.0), None, r)).len(), 0);
    }

    #[test]
    fn nan_utilization_never_fires() {
        let mut t = ThresholdTracker::default();
        let r = Utc.with_ymd_and_hms(2026, 5, 19, 17, 0, 0).unwrap();
        // NaN >= t is always false, so no notifications should be produced.
        assert_eq!(t.observe(&resp(Some(f64::NAN), None, r)).len(), 0);
    }

    #[test]
    fn exact_threshold_value_fires() {
        let mut t = ThresholdTracker::default();
        let r = Utc.with_ymd_and_hms(2026, 5, 19, 17, 0, 0).unwrap();
        // Utilization exactly equal to a threshold (0.75) should fire.
        let n = t.observe(&resp(Some(0.75), None, r));
        assert_eq!(n.len(), 1);
        assert!(n[0].title.contains("75"));
    }

    // --- SpikeTracker ---

    const SPIKE_T: f64 = 0.02; // 2%/min

    fn ts(minute: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, 19, 12, 0, 0).unwrap() + chrono::Duration::minutes(minute)
    }

    /// Session-only response with a fixed reset time.
    fn sresp(util: f64, reset: DateTime<Utc>) -> UsageResponse {
        resp(Some(util), None, reset)
    }

    #[test]
    fn spike_first_sample_never_fires() {
        let mut s = SpikeTracker::new(SPIKE_T, false);
        let r = ts(300);
        assert_eq!(s.observe(&sresp(0.10, r), ts(0)).len(), 0);
        assert!(!s.session_spiking());
    }

    #[test]
    fn spike_rate_below_threshold_no_fire() {
        let mut s = SpikeTracker::new(SPIKE_T, false);
        let r = ts(300);
        let _ = s.observe(&sresp(0.10, r), ts(0));
        // +1% over 7 min ≈ 0.14%/min, well below 2%/min.
        let n = s.observe(&sresp(0.11, r), ts(7));
        assert_eq!(n.len(), 0);
        assert!(!s.session_spiking());
    }

    #[test]
    fn spike_rate_above_threshold_fires_once() {
        let mut s = SpikeTracker::new(SPIKE_T, false);
        let r = ts(300);
        let _ = s.observe(&sresp(0.10, r), ts(0));
        // +20% over 7 min ≈ 2.86%/min.
        let n = s.observe(&sresp(0.30, r), ts(7));
        assert_eq!(n.len(), 1);
        assert!(n[0].body.contains("/min"));
        assert!(s.session_spiking());
    }

    #[test]
    fn spike_cooldown_prevents_spam() {
        let mut s = SpikeTracker::new(SPIKE_T, false);
        let r = ts(300);
        let _ = s.observe(&sresp(0.10, r), ts(0));
        assert_eq!(s.observe(&sresp(0.30, r), ts(7)).len(), 1);
        // Still climbing fast on the next two polls — no re-fire.
        assert_eq!(s.observe(&sresp(0.55, r), ts(14)).len(), 0);
        assert_eq!(s.observe(&sresp(0.80, r), ts(21)).len(), 0);
        assert!(s.session_spiking(), "badge stays lit while still climbing");
    }

    #[test]
    fn spike_rearms_after_rate_falls() {
        let mut s = SpikeTracker::new(SPIKE_T, false);
        let r = ts(300);
        let _ = s.observe(&sresp(0.10, r), ts(0));
        assert_eq!(s.observe(&sresp(0.30, r), ts(7)).len(), 1);
        // Plateau: +0.1% over 7 min ≈ 0.014%/min, below the 1%/min re-arm floor.
        assert_eq!(s.observe(&sresp(0.301, r), ts(14)).len(), 0);
        assert!(!s.session_spiking());
        // A second spike should fire again.
        assert_eq!(s.observe(&sresp(0.50, r), ts(21)).len(), 1);
    }

    #[test]
    fn spike_reset_rearms_baseline() {
        let mut s = SpikeTracker::new(SPIKE_T, false);
        let r1 = ts(300);
        let r2 = ts(600);
        let _ = s.observe(&sresp(0.10, r1), ts(0));
        assert_eq!(s.observe(&sresp(0.30, r1), ts(7)).len(), 1);
        // Window resets: utilization drops, resets_at changes — must not fire.
        let n = s.observe(&sresp(0.0, r2), ts(14));
        assert_eq!(n.len(), 0);
        assert!(!s.session_spiking());
        // Fresh baseline in the new window — a spike there fires.
        assert_eq!(s.observe(&sresp(0.25, r2), ts(21)).len(), 1);
    }

    #[test]
    fn spike_decrease_never_fires() {
        let mut s = SpikeTracker::new(SPIKE_T, false);
        let r = ts(300);
        let _ = s.observe(&sresp(0.50, r), ts(0));
        let n = s.observe(&sresp(0.20, r), ts(7));
        assert_eq!(n.len(), 0);
        assert_eq!(s.session.last_rate, 0.0);
        assert!(!s.session_spiking());
    }

    #[test]
    fn spike_nan_never_fires_and_is_not_a_baseline() {
        let mut s = SpikeTracker::new(SPIKE_T, false);
        let r = ts(300);
        // A NaN sample is ignored entirely.
        assert_eq!(s.observe(&sresp(f64::NAN, r), ts(0)).len(), 0);
        // The next valid sample is therefore the first baseline — still no fire.
        assert_eq!(s.observe(&sresp(0.10, r), ts(7)).len(), 0);
        // And a real spike after that fires normally.
        assert_eq!(s.observe(&sresp(0.40, r), ts(14)).len(), 1);
    }

    #[test]
    fn spike_zero_or_negative_dt_no_fire() {
        let mut s = SpikeTracker::new(SPIKE_T, false);
        let r = ts(300);
        let _ = s.observe(&sresp(0.10, r), ts(0));
        // Same timestamp, big jump: guarded by SPIKE_MIN_DT_MIN, no inf rate.
        let n = s.observe(&sresp(0.90, r), ts(0));
        assert_eq!(n.len(), 0);
        assert!(s.session.last_rate.is_finite());
    }

    #[test]
    fn spike_weekly_ignored_when_track_weekly_false() {
        let mut s = SpikeTracker::new(SPIKE_T, false);
        let r = ts(300);
        let mk = |w: f64| UsageResponse {
            five_hour: None,
            seven_day: Some(Window {
                utilization: w,
                resets_at: Some(r),
            }),
            extra: BTreeMap::new(),
        };
        let _ = s.observe(&mk(0.10), ts(0));
        assert_eq!(s.observe(&mk(0.40), ts(7)).len(), 0);
    }

    #[test]
    fn spike_weekly_fires_when_enabled() {
        let mut s = SpikeTracker::new(SPIKE_T, true);
        let r = ts(300);
        let mk = |w: f64| UsageResponse {
            five_hour: None,
            seven_day: Some(Window {
                utilization: w,
                resets_at: Some(r),
            }),
            extra: BTreeMap::new(),
        };
        let _ = s.observe(&mk(0.10), ts(0));
        let n = s.observe(&mk(0.40), ts(7));
        assert_eq!(n.len(), 1);
        assert!(n[0].title.contains("weekly"));
    }
}
