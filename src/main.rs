use chrono::Utc;
use claude_o_meter::app_state::{AppState, DataState};
use claude_o_meter::icons::icon_for_split;
use claude_o_meter::launch_at_login;
use claude_o_meter::menu::{bands_for, build_menu, title_for};
use claude_o_meter::notifications::{dispatch, ThresholdTracker};
use claude_o_meter::poller::{self, PollEvent};
use claude_o_meter::settings::Settings;
use std::sync::mpsc as std_mpsc;
use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tokio::sync::mpsc as tokio_mpsc;
use tray_icon::menu::MenuEvent;
use tray_icon::{TrayIcon, TrayIconBuilder};

#[derive(Debug)]
enum UserEvent {
    PollerTick(PollEvent),
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
    let (forward_tx, forward_rx) = std_mpsc::channel::<UserEvent>();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let handle = rt.handle().clone();

    std::thread::Builder::new()
        .name("tokio-rt".into())
        .spawn(move || {
            rt.block_on(async move {
                while let Some(ev) = tokio_rx.recv().await {
                    if forward_tx.send(UserEvent::PollerTick(ev)).is_err() {
                        break;
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

        if let Event::UserEvent(UserEvent::PollerTick(pe)) = event {
            match pe {
                PollEvent::Updated(usage) => {
                    for n in tracker.observe(&usage) {
                        dispatch(&n);
                    }
                    state.data = DataState::Ok { usage, fetched_at: Utc::now() };
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
