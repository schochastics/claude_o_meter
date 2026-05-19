//! Shared in-process state owned by the UI thread.

use crate::api::UsageResponse;
use crate::history::Aggregates;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone)]
pub enum DataState {
    Loading,
    Ok { usage: UsageResponse, fetched_at: DateTime<Utc> },
    AuthRequired,
    Error(String),
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub data: DataState,
    pub launch_at_login: bool,
    pub history: Option<Aggregates>,
}

impl AppState {
    pub fn new(launch_at_login: bool) -> Self {
        Self { data: DataState::Loading, launch_at_login, history: None }
    }
}
