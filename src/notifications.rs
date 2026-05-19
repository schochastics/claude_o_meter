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
    reset_at: DateTime<Utc>,
    label: &str,
    out: &mut Vec<Notification>,
) {
    if state.reset_at != Some(reset_at) {
        state.reset_at = Some(reset_at);
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
                resets_at: reset,
            }),
            seven_day: weekly.map(|u| Window {
                utilization: u,
                resets_at: reset,
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
}
