//! Launch-at-login via SMAppService.
//!
//! SMAppService requires the binary to live inside an ad-hoc-signed (or
//! Developer-ID-signed) `.app` bundle. Calls return `Ok(false)` (or fall
//! through) when run from `cargo run`.

use smappservice_rs::{AppService, ServiceStatus, ServiceType};

fn service() -> AppService {
    AppService::new(ServiceType::MainApp)
}

pub fn is_enabled() -> bool {
    matches!(service().status(), ServiceStatus::Enabled)
}

pub fn set_enabled(enable: bool) -> Result<(), String> {
    let svc = service();
    let result = if enable {
        svc.register()
    } else {
        svc.unregister()
    };
    result.map_err(|e| format!("{e:?}"))
}
