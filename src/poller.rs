//! Background polling task: read credentials, call the API, dispatch events.

use crate::api::{ApiClient, FetchError, UsageResponse};
use crate::credentials::{self, CredError};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Notify};
use tokio::time::Instant;

#[derive(Debug, Clone)]
pub enum PollEvent {
    Updated(UsageResponse),
    AuthRequired,
    Error(String),
}

pub struct PollerHandle {
    pub refresh_now: Arc<Notify>,
}

pub fn spawn(
    runtime: &tokio::runtime::Handle,
    tx: mpsc::UnboundedSender<PollEvent>,
    refresh_secs: u64,
) -> PollerHandle {
    let refresh_now = Arc::new(Notify::new());
    let refresh_now_task = refresh_now.clone();

    runtime.spawn(async move {
        run(tx, refresh_now_task, Duration::from_secs(refresh_secs)).await;
    });

    PollerHandle { refresh_now }
}

async fn run(
    tx: mpsc::UnboundedSender<PollEvent>,
    refresh_now: Arc<Notify>,
    base_interval: Duration,
) {
    const MIN_DELAY: Duration = Duration::from_secs(60);
    const MAX_DELAY: Duration = Duration::from_secs(30 * 60);

    let mut consecutive_429: u32 = 0;
    let mut next_delay = Duration::from_secs(0);

    loop {
        let deadline = Instant::now() + next_delay;
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => {}
            _ = refresh_now.notified() => {
                tracing::debug!("manual refresh");
            }
        }

        let outcome = poll_once().await;
        match outcome {
            Ok(usage) => {
                consecutive_429 = 0;
                if tx.send(PollEvent::Updated(usage)).is_err() {
                    return;
                }
                next_delay = base_interval;
            }
            Err(PollError::AuthRequired) => {
                consecutive_429 = 0;
                if tx.send(PollEvent::AuthRequired).is_err() {
                    return;
                }
                next_delay = base_interval.max(MIN_DELAY);
            }
            Err(PollError::RateLimited { retry_after }) => {
                consecutive_429 += 1;
                let backoff = base_interval.saturating_mul(1u32 << consecutive_429.min(5));
                let server = retry_after.unwrap_or(Duration::ZERO);
                next_delay = MIN_DELAY.max(server).max(backoff).min(MAX_DELAY);
                if tx.send(PollEvent::Error("rate limited".into())).is_err() {
                    return;
                }
            }
            Err(PollError::Transient(msg)) => {
                consecutive_429 = 0;
                next_delay = base_interval.max(MIN_DELAY);
                if tx.send(PollEvent::Error(msg)).is_err() {
                    return;
                }
            }
        }
    }
}

#[derive(Debug)]
enum PollError {
    AuthRequired,
    RateLimited { retry_after: Option<Duration> },
    Transient(String),
}

async fn poll_once() -> Result<UsageResponse, PollError> {
    let creds = credentials::read_credentials().map_err(|e| match e {
        CredError::NotFound | CredError::AccessDenied => PollError::AuthRequired,
        other => PollError::Transient(other.to_string()),
    })?;
    if creds.is_expired(chrono::Utc::now()) {
        return Err(PollError::AuthRequired);
    }
    let client = ApiClient::new(creds.access_token);
    client.fetch().await.map_err(|e| match e {
        FetchError::Unauthorized => PollError::AuthRequired,
        FetchError::RateLimited { retry_after } => PollError::RateLimited { retry_after },
        other => PollError::Transient(other.to_string()),
    })
}
