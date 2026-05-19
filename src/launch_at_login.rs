//! Launch-at-login via SMAppService.
//!
//! SMAppService requires the binary to live inside an ad-hoc-signed (or
//! Developer-ID-signed) `.app` bundle. Calls return `Ok(false)` (or fall
//! through) when run from `cargo run`.

use smappservice_rs::{AppService, ServiceManagementError, ServiceStatus, ServiceType};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LaunchError {
    #[error("SMAppService register failed: {0:?}")]
    Register(ServiceManagementError),
    #[error("SMAppService unregister failed: {0:?}")]
    Unregister(ServiceManagementError),
}

fn service() -> AppService {
    AppService::new(ServiceType::MainApp)
}

pub fn is_enabled() -> bool {
    matches!(service().status(), ServiceStatus::Enabled)
}

pub fn set_enabled(enable: bool) -> Result<(), LaunchError> {
    let svc = service();
    if enable {
        svc.register().map_err(LaunchError::Register)
    } else {
        svc.unregister().map_err(LaunchError::Unregister)
    }
}
