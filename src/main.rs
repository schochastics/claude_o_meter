use chrono::Utc;
use claude_o_meter::app_state::{AppState, DataState};
use claude_o_meter::history::Aggregates;
use claude_o_meter::icons::icon_for_split;
use claude_o_meter::launch_at_login;
use claude_o_meter::menu::{bands_for, build_menu, title_for};
use claude_o_meter::notifications::{ThresholdTracker, dispatch};
use claude_o_meter::poller::{self, PollEvent};
use claude_o_meter::settings::Settings;
use directories::{BaseDirs, ProjectDirs};
use std::path::PathBuf;
use std::sync::mpsc as std_mpsc;
use std::time::Duration;
use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tokio::sync::mpsc as tokio_mpsc;
use tray_icon::menu::MenuEvent;
use tray_icon::{TrayIcon, TrayIconBuilder};

#[derive(Debug)]
enum UserEvent {
    PollerTick(PollEvent),
    HistoryTick(Aggregates),
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let bundle_id = "com.cynkra.claude-o-meter";
    if let Err(e) = mac_notification_sys::set_application(bundle_id) {
        tracing::debug!(error = %e, "set_application failed (run from .app bundle for notifications)");
    }

    let settings = Settings::load();
    let mut tracker = ThresholdTracker::new(settings.thresholds.clone());
    let mut state = AppState::new(launch_at_login::is_enabled());

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    let (tokio_tx, mut tokio_rx) = tokio_mpsc::unbounded_channel::<PollEvent>();
    let (history_tx, mut history_rx) = tokio_mpsc::unbounded_channel::<Aggregates>();
    let (forward_tx, forward_rx) = std_mpsc::channel::<UserEvent>();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let handle = rt.handle().clone();

    let forward_tx_clone = forward_tx.clone();
    std::thread::Builder::new()
        .name("tokio-rt".into())
        .spawn(move || {
            rt.block_on(async move {
                loop {
                    tokio::select! {
                        Some(ev) = tokio_rx.recv() => {
                            if forward_tx_clone.send(UserEvent::PollerTick(ev)).is_err() { break; }
                        }
                        Some(agg) = history_rx.recv() => {
                            if forward_tx_clone.send(UserEvent::HistoryTick(agg)).is_err() { break; }
                        }
                        else => break,
                    }
                }
            });
        })?;

    std::thread::Builder::new()
        .name("proxy-pump".into())
        .spawn(move || {
            while let Ok(ev) = forward_rx.recv() {
                if proxy.send_event(ev).is_err() {
                    break;
                }
            }
        })?;

    let poller_handle = poller::spawn(&handle, tokio_tx, settings.refresh_secs);
    spawn_history_loop(&handle, history_tx);
    drop(forward_tx);

    let initial = build_menu(&state);
    let mut refresh_id = initial.refresh.id().clone();
    let mut login_id = initial.launch_at_login.id().clone();
    let mut quit_id = initial.quit.id().clone();

    let (left, right) = bands_for(&state);
    let tray: TrayIcon = TrayIconBuilder::new()
        .with_menu(Box::new(initial.menu))
        .with_icon(icon_for_split(left, right))
        .with_tooltip(title_for(&state))
        .build()?;

    let menu_events = MenuEvent::receiver();

    event_loop.run(move |event, _target, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::UserEvent(UserEvent::PollerTick(pe)) => {
                match pe {
                    PollEvent::Updated(usage) => {
                        for n in tracker.observe(&usage) {
                            dispatch(&n);
                        }
                        state.data = DataState::Ok {
                            usage,
                            fetched_at: Utc::now(),
                        };
                    }
                    PollEvent::AuthRequired => {
                        state.data = DataState::AuthRequired;
                    }
                    PollEvent::Error(msg) => {
                        state.data = DataState::Error(msg);
                    }
                }
                let rebuilt = build_menu(&state);
                refresh_id = rebuilt.refresh.id().clone();
                login_id = rebuilt.launch_at_login.id().clone();
                quit_id = rebuilt.quit.id().clone();
                tray.set_menu(Some(Box::new(rebuilt.menu)));
                let (left, right) = bands_for(&state);
                tray.set_icon(Some(icon_for_split(left, right))).ok();
                tray.set_tooltip(Some(title_for(&state))).ok();
                tray.set_title(None::<&str>);
            }
            Event::UserEvent(UserEvent::HistoryTick(agg)) => {
                state.history = Some(agg);
                let rebuilt = build_menu(&state);
                refresh_id = rebuilt.refresh.id().clone();
                login_id = rebuilt.launch_at_login.id().clone();
                quit_id = rebuilt.quit.id().clone();
                tray.set_menu(Some(Box::new(rebuilt.menu)));
            }
            _ => {}
        }

        while let Ok(menu_event) = menu_events.try_recv() {
            if menu_event.id == refresh_id {
                poller_handle.refresh_now.notify_one();
            } else if menu_event.id == login_id {
                let want = !state.launch_at_login;
                match launch_at_login::set_enabled(want) {
                    Ok(()) => {
                        state.launch_at_login = launch_at_login::is_enabled();
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "launch-at-login toggle failed");
                    }
                }
                let rebuilt = build_menu(&state);
                refresh_id = rebuilt.refresh.id().clone();
                login_id = rebuilt.launch_at_login.id().clone();
                quit_id = rebuilt.quit.id().clone();
                tray.set_menu(Some(Box::new(rebuilt.menu)));
            } else if menu_event.id == quit_id {
                let _ = settings.save();
                *control_flow = ControlFlow::Exit;
            }
        }
    });
}

fn projects_dir() -> Option<PathBuf> {
    BaseDirs::new().map(|b| b.home_dir().join(".claude/projects"))
}

fn history_cache_path() -> Option<PathBuf> {
    ProjectDirs::from("com", "cynkra", "claude-o-meter")
        .map(|d| d.config_dir().join("history.json"))
}

fn spawn_history_loop(
    runtime: &tokio::runtime::Handle,
    tx: tokio_mpsc::UnboundedSender<Aggregates>,
) {
    let Some(proj_dir) = projects_dir() else {
        tracing::warn!("no home directory; history disabled");
        return;
    };
    let cache_path = history_cache_path();
    runtime.spawn(async move {
        let cache_path_owned = cache_path.clone();
        let proj_dir_owned = proj_dir.clone();

        // Load cache (if any) and emit immediately so the menu populates.
        let mut agg = if let Some(p) = cache_path_owned.as_ref() {
            tokio::task::spawn_blocking({
                let p = p.clone();
                move || Aggregates::load_or_default(&p)
            })
            .await
            .unwrap_or_default()
        } else {
            Aggregates::default()
        };
        if !agg.by_day.is_empty() || agg.scanned_files > 0 {
            let _ = tx.send(agg.clone());
        }

        loop {
            let proj = proj_dir_owned.clone();
            let cache = cache_path_owned.clone();
            let mut working = agg.clone();
            let scanned = tokio::task::spawn_blocking(move || {
                let changed = working.refresh(&proj).unwrap_or(false);
                if changed
                    && let Some(p) = cache.as_ref()
                    && let Err(e) = working.save(p)
                {
                    tracing::warn!(error = %e, "history save failed");
                }
                (changed, working)
            })
            .await;
            if let Ok((changed, updated)) = scanned
                && changed
            {
                agg = updated.clone();
                if tx.send(updated).is_err() {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_secs(5 * 60)).await;
        }
    });
}
