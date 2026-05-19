//! Build the tray dropdown menu from the current AppState.

use crate::app_state::{AppState, DataState};
use crate::bars::{render_bar, render_stacked_bar};
use crate::history::{short_name, Aggregates};
use crate::icons::Band;
use crate::time_fmt::resets_in;
use chrono::{Local, Utc};
use tray_icon::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};

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

    if let Some(history) = state.history.as_ref() {
        let _ = menu.append(&PredefinedMenuItem::separator());
        append_history_section(&menu, history);
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

const DAY_BAR_WIDTH: usize = 20;
const PROJECT_BAR_WIDTH: usize = 14;
const TOP_PROJECTS_N: usize = 8;

/// Render the History block: 7-day stacked bars + Top-projects submenus.
fn append_history_section(menu: &Menu, h: &Aggregates) {
    let today = Local::now().date_naive();
    let days = h.last_n_days(7, today);
    let max_total: u64 = days.iter().map(|(_, t)| t.sum()).max().unwrap_or(0);
    let week_total: u64 = days.iter().map(|(_, t)| t.sum()).sum();

    let header = MenuItem::new(
        format!("History — last 7d: {}", humanize_tokens(week_total)),
        false,
        None,
    );
    let _ = menu.append(&header);

    let legend = MenuItem::new(
        "  \u{1F7E6} in   \u{1F7E7} out   \u{1F7EA} cwrite   \u{1F7E9} cread",
        false,
        None,
    );
    let _ = menu.append(&legend);

    for (date, totals) in days {
        let row_total = totals.sum();
        let cells = if max_total == 0 {
            0
        } else {
            (DAY_BAR_WIDTH as f64 * row_total as f64 / max_total as f64).round() as usize
        };
        let bar = if cells == 0 {
            String::new()
        } else {
            render_stacked_bar(
                &[
                    (totals.input, '\u{1F7E6}'),
                    (totals.output, '\u{1F7E7}'),
                    (totals.cache_creation, '\u{1F7EA}'),
                    (totals.cache_read, '\u{1F7E9}'),
                ],
                cells,
            )
        };
        let label = format!(
            "  {}  {:>6}  {}",
            date.format("%a %m-%d"),
            humanize_tokens(row_total),
            bar
        );
        let _ = menu.append(&MenuItem::new(label, false, None));
    }

    let since_7d = today - chrono::Duration::days(6);
    let _ = menu.append(&top_projects_submenu("Top projects (7d) \u{25B8}", h, Some(since_7d)));
    let _ = menu.append(&top_projects_submenu(
        "Top projects (all-time) \u{25B8}",
        h,
        None,
    ));
}

fn top_projects_submenu(label: &str, h: &Aggregates, since: Option<chrono::NaiveDate>) -> Submenu {
    let sub = Submenu::new(label, true);
    let top = h.top_projects(TOP_PROJECTS_N, since);
    if top.is_empty() {
        let _ = sub.append(&MenuItem::new("(no data)", false, None));
        return sub;
    }
    let max_total = top.first().map(|(_, n)| *n).unwrap_or(0);
    let all_paths: Vec<&str> = h.by_project.keys().map(|s| s.as_str()).collect();
    for (path, total) in &top {
        let bar = render_stacked_bar(
            &[(*total, '\u{1F7E7}')],
            ((PROJECT_BAR_WIDTH as f64 * (*total as f64) / max_total as f64).round() as usize)
                .max(1),
        );
        let name = short_name(path, &all_paths);
        let label = format!("{:<24} {:>7}  {}", name, humanize_tokens(*total), bar);
        let _ = sub.append(&MenuItem::new(label, false, None));
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
