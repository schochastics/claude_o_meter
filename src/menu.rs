//! Build the tray dropdown menu from the current AppState.

use crate::app_state::{AppState, DataState};
use crate::bars::render_bar;
use crate::icons::Band;
use crate::time_fmt::resets_in;
use chrono::Utc;
use tray_icon::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};

pub struct MenuIds {
    pub menu: Menu,
    pub refresh: MenuItem,
    pub launch_at_login: CheckMenuItem,
    pub quit: MenuItem,
}

const BAR_WIDTH: usize = 16;

pub fn build_menu(state: &AppState) -> MenuIds {
    let menu = Menu::new();

    match &state.data {
        DataState::Loading => {
            let item = MenuItem::new("Loading…", false, None);
            let _ = menu.append(&item);
        }
        DataState::AuthRequired => {
            let title = MenuItem::new("Token expired — run `claude login`", false, None);
            let _ = menu.append(&title);
        }
        DataState::Error(msg) => {
            let item = MenuItem::new(format!("Error: {msg}"), false, None);
            let _ = menu.append(&item);
        }
        DataState::Ok { usage, fetched_at } => {
            let now = Utc::now();
            if let Some(w) = usage.five_hour.as_ref() {
                let pct = (w.utilization * 100.0).round() as i64;
                let title = MenuItem::new(format!("Session  {pct}%"), false, None);
                let bar = MenuItem::new(
                    format!("  {} {}", render_bar(w.utilization, BAR_WIDTH), resets_in(w.resets_at, now)),
                    false,
                    None,
                );
                let _ = menu.append(&title);
                let _ = menu.append(&bar);
            }
            if let Some(w) = usage.seven_day.as_ref() {
                let pct = (w.utilization * 100.0).round() as i64;
                let title = MenuItem::new(format!("Weekly  {pct}%"), false, None);
                let bar = MenuItem::new(
                    format!("  {} {}", render_bar(w.utilization, BAR_WIDTH), resets_in(w.resets_at, now)),
                    false,
                    None,
                );
                let _ = menu.append(&title);
                let _ = menu.append(&bar);
            }

            let per_model = usage.per_model();
            if !per_model.is_empty() {
                let _ = menu.append(&PredefinedMenuItem::separator());
                let header = MenuItem::new("Weekly by model", false, None);
                let _ = menu.append(&header);
                for (label, w) in per_model {
                    let pct = (w.utilization * 100.0).round() as i64;
                    let row = MenuItem::new(
                        format!("  {:<8} {}  {pct}%", label, render_bar(w.utilization, BAR_WIDTH - 4)),
                        false,
                        None,
                    );
                    let _ = menu.append(&row);
                }
            }

            let _ = menu.append(&PredefinedMenuItem::separator());
            let fetched =
                MenuItem::new(format!("Updated {}", fetched_at.format("%H:%M:%S")), false, None);
            let _ = menu.append(&fetched);
        }
    }

    let _ = menu.append(&PredefinedMenuItem::separator());
    let refresh = MenuItem::new("Refresh now", true, None);
    let launch_at_login =
        CheckMenuItem::new("Launch at Login", true, state.launch_at_login, None);
    let quit = MenuItem::new("Quit Claude-O-Meter", true, None);
    let _ = menu.append(&refresh);
    let _ = menu.append(&launch_at_login);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&quit);

    MenuIds { menu, refresh, launch_at_login, quit }
}

/// Menu bar title shown beside the icon. Empty when there's no data yet.
pub fn title_for(state: &AppState) -> String {
    match &state.data {
        DataState::Loading => "…".to_string(),
        DataState::AuthRequired => "?".to_string(),
        DataState::Error(_) => "!".to_string(),
        DataState::Ok { usage, .. } => {
            let pct = (usage.headline_fraction() * 100.0).round() as i64;
            format!(" {pct}%")
        }
    }
}

/// (left, right) bands for the split icon — left tinted by the 5h session
/// utilization, right by the 7d weekly utilization. Defaults to Blue while
/// loading and Red on auth/error.
pub fn bands_for(state: &AppState) -> (Band, Band) {
    match &state.data {
        DataState::Ok { usage, .. } => {
            let s = usage.five_hour.as_ref().map(|w| w.utilization).unwrap_or(0.0);
            let w = usage.seven_day.as_ref().map(|w| w.utilization).unwrap_or(0.0);
            (Band::from_fraction(s), Band::from_fraction(w))
        }
        DataState::AuthRequired | DataState::Error(_) => (Band::Red, Band::Red),
        DataState::Loading => (Band::Blue, Band::Blue),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{UsageResponse, Window};
    use chrono::TimeZone;
    use std::collections::BTreeMap;

    #[test]
    fn title_loading() {
        let s = AppState::new(false);
        assert_eq!(title_for(&s), "…");
    }

    #[test]
    fn title_auth_required() {
        let mut s = AppState::new(false);
        s.data = DataState::AuthRequired;
        assert_eq!(title_for(&s), "?");
    }

    #[test]
    fn title_ok_takes_max() {
        let r = Utc.with_ymd_and_hms(2026, 5, 19, 17, 0, 0).unwrap();
        let usage = UsageResponse {
            five_hour: Some(Window { utilization: 0.30, resets_at: r }),
            seven_day: Some(Window { utilization: 0.74, resets_at: r }),
            extra: BTreeMap::new(),
        };
        let mut s = AppState::new(false);
        s.data = DataState::Ok { usage, fetched_at: Utc::now() };
        assert_eq!(title_for(&s), " 74%");
    }

    #[test]
    fn bands_ok_match_fractions() {
        let r = Utc.with_ymd_and_hms(2026, 5, 19, 17, 0, 0).unwrap();
        let usage = UsageResponse {
            five_hour: Some(Window { utilization: 0.95, resets_at: r }),
            seven_day: Some(Window { utilization: 0.20, resets_at: r }),
            extra: BTreeMap::new(),
        };
        let mut s = AppState::new(false);
        s.data = DataState::Ok { usage, fetched_at: Utc::now() };
        assert_eq!(bands_for(&s), (Band::Red, Band::Blue));
    }

    #[test]
    fn bands_loading_is_blue() {
        let s = AppState::new(false);
        assert_eq!(bands_for(&s), (Band::Blue, Band::Blue));
    }

    #[test]
    fn bands_auth_required_is_red() {
        let mut s = AppState::new(false);
        s.data = DataState::AuthRequired;
        assert_eq!(bands_for(&s), (Band::Red, Band::Red));
    }

    #[test]
    fn bands_missing_window_falls_back_to_zero() {
        let r = Utc.with_ymd_and_hms(2026, 5, 19, 17, 0, 0).unwrap();
        let usage = UsageResponse {
            five_hour: Some(Window { utilization: 0.80, resets_at: r }),
            seven_day: None,
            extra: BTreeMap::new(),
        };
        let mut s = AppState::new(false);
        s.data = DataState::Ok { usage, fetched_at: Utc::now() };
        assert_eq!(bands_for(&s), (Band::Orange, Band::Blue));
    }
}
