//! Build the tray dropdown menu from the current AppState.

use crate::app_state::{AppState, DataState};
use crate::bars::{CANVAS_H, CANVAS_W, Theme, render_bar_rgba, render_solid_bar_rgba};
use crate::history::{Aggregates, short_name};
use crate::icons::Band;
use crate::theme::Appearance;
use crate::time_fmt::resets_in;
use chrono::{Local, Utc};
use tray_icon::menu::{
    CheckMenuItem, Icon, IconMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu,
};

pub struct MenuIds {
    pub menu: Menu,
    pub refresh: MenuItem,
    pub purge_history: MenuItem,
    pub launch_at_login: CheckMenuItem,
    pub quit: MenuItem,
}

// Token-category colors for the stacked history bar.
const COLOR_INPUT: [u8; 3] = [0x4D, 0x9D, 0xE0]; // blue
const COLOR_OUTPUT: [u8; 3] = [0xF2, 0x9E, 0x4C]; // claude-orange
const COLOR_CACHE_CREATION: [u8; 3] = [0xB5, 0x7E, 0xDC]; // purple
const COLOR_CACHE_READ: [u8; 3] = [0x3A, 0xC0, 0x6E]; // green

const TOP_PROJECTS_N: usize = 8;
const TOKEN_COL_WIDTH: usize = 6;
const FIG_SPACE: char = '\u{2007}';

pub fn build_menu(state: &AppState) -> MenuIds {
    let menu = Menu::new();
    let theme = Appearance::detect().theme();

    match &state.data {
        DataState::Loading => {
            let _ = menu.append(&MenuItem::new("Loading…", false, None));
        }
        DataState::AuthRequired => {
            let _ = menu.append(&MenuItem::new(
                "Token expired — run `claude login`",
                false,
                None,
            ));
        }
        DataState::Error(msg) => {
            let _ = menu.append(&MenuItem::new(format!("Error: {msg}"), false, None));
        }
        DataState::Ok { usage, fetched_at } => {
            let now = Utc::now();

            if let Some(w) = usage.five_hour.as_ref() {
                let _ = menu.append(&window_row(
                    "Session",
                    w.utilization,
                    w.resets_at,
                    now,
                    &theme,
                ));
            }
            if let Some(w) = usage.seven_day.as_ref() {
                let _ = menu.append(&window_row(
                    "Weekly",
                    w.utilization,
                    w.resets_at,
                    now,
                    &theme,
                ));
                if let Some(line) = burn_rate_line(w.utilization, w.resets_at, now) {
                    let _ = menu.append(&MenuItem::new(line, false, None));
                }
            }

            let _ = menu.append(&PredefinedMenuItem::separator());
            let _ = menu.append(&MenuItem::new(
                format!(
                    "Updated {}",
                    fetched_at.with_timezone(&Local).format("%H:%M:%S")
                ),
                false,
                None,
            ));
        }
    }

    if let Some(history) = state.history.as_ref() {
        let _ = menu.append(&PredefinedMenuItem::separator());
        let today = Local::now().date_naive();
        let _ = menu.append(&history_submenu(history, today, &theme));
        if let Some(line) = monthly_line(history, today) {
            let _ = menu.append(&MenuItem::new(line, false, None));
        }
        let _ = menu.append(&top_projects_submenu(
            "Top projects (7d)",
            history,
            Some(today - chrono::Duration::days(6)),
            &theme,
        ));
        let _ = menu.append(&top_projects_submenu(
            "Top projects (all-time)",
            history,
            None,
            &theme,
        ));
    }

    let _ = menu.append(&PredefinedMenuItem::separator());
    let refresh = MenuItem::new("Refresh now", true, None);
    let tombstones = state
        .history
        .as_ref()
        .map(|h| h.tombstoned_count())
        .unwrap_or(0);
    let purge_label = if tombstones > 0 {
        format!("Purge removed sessions ({tombstones})")
    } else {
        "Purge removed sessions".to_string()
    };
    let purge_history = MenuItem::new(purge_label, tombstones > 0, None);
    let launch_at_login = CheckMenuItem::new("Launch at Login", true, state.launch_at_login, None);
    let quit = MenuItem::new("Quit Claude-O-Meter", true, None);
    let _ = menu.append(&refresh);
    let _ = menu.append(&purge_history);
    let _ = menu.append(&launch_at_login);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&quit);

    MenuIds {
        menu,
        refresh,
        purge_history,
        launch_at_login,
        quit,
    }
}

fn window_row(
    name: &str,
    utilization: f64,
    resets_at: Option<chrono::DateTime<Utc>>,
    now: chrono::DateTime<Utc>,
    theme: &Theme,
) -> IconMenuItem {
    let pct = (utilization * 100.0).round() as i64;
    let band = Band::from_fraction(utilization);
    let bar = render_solid_bar_rgba(utilization, band.rgb(), theme);
    let label = match resets_at {
        Some(t) => format!("{name}  {pct}%  {}", resets_in(t, now)),
        None => format!("{name}  {pct}%"),
    };
    IconMenuItem::new(label, false, Some(bar_icon(bar)), None)
}

fn bar_icon(rgba: Vec<u8>) -> Icon {
    Icon::from_rgba(rgba, CANVAS_W, CANVAS_H).expect("valid RGBA buffer")
}

/// Build the "History ▸" submenu — 7 day rows, today first, with stacked
/// per-category bars rendered as menu-item images.
fn history_submenu(h: &Aggregates, today: chrono::NaiveDate, theme: &Theme) -> Submenu {
    let mut days = h.last_n_days(7, today);
    days.reverse();
    let max_total: u64 = days.iter().map(|(_, t)| t.sum()).max().unwrap_or(0);
    let week_total: u64 = days.iter().map(|(_, t)| t.sum()).sum();

    let sub = Submenu::new(
        format!("History — last 7d: {}", humanize_tokens(week_total)),
        true,
    );

    let _ = sub.append(&MenuItem::new(
        "input · output · cache write · cache read",
        false,
        None,
    ));
    let _ = sub.append(&PredefinedMenuItem::separator());

    let scale_basis = max_total.max(1);
    for (date, totals) in &days {
        let row_total = totals.sum();
        let bar = render_bar_rgba(
            &[
                (totals.input, COLOR_INPUT),
                (totals.output, COLOR_OUTPUT),
                (totals.cache_creation, COLOR_CACHE_CREATION),
                (totals.cache_read, COLOR_CACHE_READ),
            ],
            scale_basis,
            theme,
        );
        let label = format!(
            "{}  {}",
            pad_left_figure(&humanize_tokens(row_total), TOKEN_COL_WIDTH),
            date.format("%a %m-%d"),
        );
        let _ = sub.append(&IconMenuItem::new(label, false, Some(bar_icon(bar)), None));
    }
    sub
}

fn top_projects_submenu(
    label: &str,
    h: &Aggregates,
    since: Option<chrono::NaiveDate>,
    theme: &Theme,
) -> Submenu {
    let sub = Submenu::new(label, true);
    let top = h.top_projects(TOP_PROJECTS_N, since);
    if top.is_empty() {
        let _ = sub.append(&MenuItem::new("(no data)", false, None));
        return sub;
    }
    let max_total = top.first().map(|(_, n)| *n).unwrap_or(0).max(1);
    let all_paths: Vec<&str> = h.by_project.keys().map(|s| s.as_str()).collect();
    for (path, total) in &top {
        let fraction = *total as f64 / max_total as f64;
        let bar = render_solid_bar_rgba(fraction, COLOR_OUTPUT, theme);
        let name = short_name(path, &all_paths);
        let label = format!(
            "{}  {}",
            pad_left_figure(&humanize_tokens(*total), TOKEN_COL_WIDTH),
            name,
        );
        let _ = sub.append(&IconMenuItem::new(label, false, Some(bar_icon(bar)), None));
    }
    sub
}

fn humanize_tokens(n: u64) -> String {
    if n >= 1_000_000_000 {
        format!("{:.1}B", n as f64 / 1_000_000_000.0)
    } else if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.0}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// Weekly burn-rate projection. Returns a short status string to render as a
/// disabled menu row beneath the Weekly window. `None` when the window is too
/// fresh to make a useful projection (need ≥ 6h elapsed and ≥ 5% utilized) or
/// when the API hasn't reported `resets_at` yet.
fn burn_rate_line(
    utilization: f64,
    resets_at: Option<chrono::DateTime<Utc>>,
    now: chrono::DateTime<Utc>,
) -> Option<String> {
    const WINDOW_HOURS: f64 = 7.0 * 24.0;
    if !utilization.is_finite() {
        return None;
    }
    if utilization >= 1.0 {
        return Some("Pace: at cap".to_string());
    }
    let resets_at = resets_at?;
    let hours_remaining = (resets_at - now).num_seconds() as f64 / 3600.0;
    let hours_elapsed = WINDOW_HOURS - hours_remaining;
    if hours_elapsed < 6.0 || utilization < 0.05 {
        return None;
    }
    let pace_per_hour = utilization / hours_elapsed;
    let hours_until_cap = (1.0 - utilization) / pace_per_hour;
    if hours_until_cap >= hours_remaining {
        Some("Pace: on track to reset under cap".to_string())
    } else {
        Some(format!("Pace: cap in {}", humanize_hours(hours_until_cap)))
    }
}

fn humanize_hours(h: f64) -> String {
    if h < 1.0 {
        let m = (h * 60.0).round().max(1.0) as i64;
        format!("{m}m")
    } else if h < 24.0 {
        format!("{}h", h.round() as i64)
    } else {
        let days = (h / 24.0).floor() as i64;
        let rem_h = (h - days as f64 * 24.0).round() as i64;
        if rem_h == 0 {
            format!("{days}d")
        } else {
            format!("{days}d {rem_h}h")
        }
    }
}

/// Tokens-this-calendar-month with linear projection to month-end. Returns
/// `None` if there's no usage data for the current month yet.
fn monthly_line(history: &Aggregates, today: chrono::NaiveDate) -> Option<String> {
    let (current, projected) = history.current_month_total_and_projection(today);
    if current == 0 {
        return None;
    }
    Some(format!(
        "Month: {} (proj {})",
        humanize_tokens(current),
        humanize_tokens(projected),
    ))
}

/// Left-pad `s` with figure-spaces (U+2007) to `width` chars. Figure spaces
/// match the width of a digit in proportional fonts, so they keep numeric
/// columns aligned in menu labels rendered in SF.
fn pad_left_figure(s: &str, width: usize) -> String {
    let chars = s.chars().count();
    if chars >= width {
        return s.to_string();
    }
    let pad: String = std::iter::repeat_n(FIG_SPACE, width - chars).collect();
    format!("{pad}{s}")
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
            let s = usage
                .five_hour
                .as_ref()
                .map(|w| w.utilization)
                .unwrap_or(0.0);
            let w = usage
                .seven_day
                .as_ref()
                .map(|w| w.utilization)
                .unwrap_or(0.0);
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
            five_hour: Some(Window {
                utilization: 0.30,
                resets_at: Some(r),
            }),
            seven_day: Some(Window {
                utilization: 0.74,
                resets_at: Some(r),
            }),
            extra: BTreeMap::new(),
        };
        let mut s = AppState::new(false);
        s.data = DataState::Ok {
            usage,
            fetched_at: Utc::now(),
        };
        assert_eq!(title_for(&s), " 74%");
    }

    #[test]
    fn bands_ok_match_fractions() {
        let r = Utc.with_ymd_and_hms(2026, 5, 19, 17, 0, 0).unwrap();
        let usage = UsageResponse {
            five_hour: Some(Window {
                utilization: 0.95,
                resets_at: Some(r),
            }),
            seven_day: Some(Window {
                utilization: 0.20,
                resets_at: Some(r),
            }),
            extra: BTreeMap::new(),
        };
        let mut s = AppState::new(false);
        s.data = DataState::Ok {
            usage,
            fetched_at: Utc::now(),
        };
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
    fn pad_left_figure_fills_with_figure_space() {
        let s = pad_left_figure("47", 5);
        assert_eq!(s.chars().count(), 5);
        assert_eq!(s.chars().filter(|c| *c == '\u{2007}').count(), 3);
        assert!(s.ends_with("47"));
    }

    #[test]
    fn pad_left_figure_leaves_overlong_alone() {
        assert_eq!(pad_left_figure("123456", 4), "123456");
    }

    #[test]
    fn humanize_tokens_scales() {
        assert_eq!(humanize_tokens(42), "42");
        assert_eq!(humanize_tokens(1500), "2K");
        assert_eq!(humanize_tokens(1_200_000), "1.2M");
        assert_eq!(humanize_tokens(2_500_000_000), "2.5B");
    }

    fn ts(year: i32, month: u32, day: u32, hour: u32) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, hour, 0, 0).unwrap()
    }

    #[test]
    fn burn_rate_none_when_window_too_fresh() {
        // Window just started (1h elapsed) — too noisy to project.
        let resets = ts(2026, 5, 26, 12);
        let now = resets - chrono::Duration::hours(167);
        assert_eq!(burn_rate_line(0.5, Some(resets), now), None);
    }

    #[test]
    fn burn_rate_none_when_utilization_too_low() {
        let resets = ts(2026, 5, 26, 12);
        let now = resets - chrono::Duration::hours(100);
        // 68h elapsed but only 1% used → too low to project.
        assert_eq!(burn_rate_line(0.01, Some(resets), now), None);
    }

    #[test]
    fn burn_rate_on_track_when_pace_slow() {
        let resets = ts(2026, 5, 26, 12);
        // 84h elapsed (halfway through window), 30% used → pace will reset under cap.
        let now = resets - chrono::Duration::hours(84);
        let line = burn_rate_line(0.30, Some(resets), now).unwrap();
        assert!(line.contains("on track"), "got: {line}");
    }

    #[test]
    fn burn_rate_warns_when_pace_will_exceed_cap() {
        let resets = ts(2026, 5, 26, 12);
        // 24h elapsed, 50% used → at this pace, cap in ~24h, well before reset.
        let now = resets - chrono::Duration::hours(144);
        let line = burn_rate_line(0.50, Some(resets), now).unwrap();
        assert!(line.contains("cap in"), "got: {line}");
    }

    #[test]
    fn burn_rate_at_cap() {
        let resets = ts(2026, 5, 26, 12);
        let now = resets - chrono::Duration::hours(50);
        assert_eq!(
            burn_rate_line(1.0, Some(resets), now),
            Some("Pace: at cap".to_string())
        );
    }

    #[test]
    fn burn_rate_nan_yields_none() {
        let resets = ts(2026, 5, 26, 12);
        let now = resets - chrono::Duration::hours(50);
        assert_eq!(burn_rate_line(f64::NAN, Some(resets), now), None);
    }

    #[test]
    fn burn_rate_none_when_resets_at_missing() {
        // Fresh login: API returns null for resets_at; skip the projection.
        let now = ts(2026, 5, 26, 12);
        assert_eq!(burn_rate_line(0.5, None, now), None);
    }

    #[test]
    fn humanize_hours_formats_buckets() {
        assert_eq!(humanize_hours(0.25), "15m");
        assert_eq!(humanize_hours(3.4), "3h");
        assert_eq!(humanize_hours(48.0), "2d");
        assert_eq!(humanize_hours(50.0), "2d 2h");
    }

    #[test]
    fn bands_missing_window_falls_back_to_zero() {
        let r = Utc.with_ymd_and_hms(2026, 5, 19, 17, 0, 0).unwrap();
        let usage = UsageResponse {
            five_hour: Some(Window {
                utilization: 0.80,
                resets_at: Some(r),
            }),
            seven_day: None,
            extra: BTreeMap::new(),
        };
        let mut s = AppState::new(false);
        s.data = DataState::Ok {
            usage,
            fetched_at: Utc::now(),
        };
        assert_eq!(bands_for(&s), (Band::Orange, Band::Blue));
    }
}
