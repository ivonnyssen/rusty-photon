use filemonitor::run_server_loop;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use tokio::sync::Notify;
use tracing::{error, Level};
use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::{define_windows_service, service_dispatcher};

static SERVICE_CONFIG: OnceLock<(PathBuf, Level)> = OnceLock::new();

const SERVICE_NAME: &str = "filemonitor";

pub fn run(config_path: PathBuf, log_level: Level) -> Result<(), Box<dyn std::error::Error>> {
    SERVICE_CONFIG
        .set((config_path, log_level))
        .map_err(|_| "Service config already set")?;

    service_dispatcher::start(SERVICE_NAME, ffi_service_main)?;
    Ok(())
}

define_windows_service!(ffi_service_main, service_main);

fn service_main(_arguments: Vec<OsString>) {
    if let Err(e) = run_service() {
        error!("Service failed: {}", e);
    }
}

fn run_service() -> Result<(), Box<dyn std::error::Error>> {
    let (config_path, log_level) = SERVICE_CONFIG.get().expect("Service config not set");

    tracing_subscriber::fmt().with_max_level(*log_level).init();

    let stop_notify = Arc::new(Notify::new());
    let reload_notify = Arc::new(Notify::new());

    let stop_notify_clone = Arc::clone(&stop_notify);
    let reload_notify_clone = Arc::clone(&reload_notify);

    let status_handle =
        service_control_handler::register(
            SERVICE_NAME,
            move |control_event| match control_event {
                ServiceControl::Stop => {
                    stop_notify_clone.notify_one();
                    ServiceControlHandlerResult::NoError
                }
                ServiceControl::ParamChange => {
                    reload_notify_clone.notify_one();
                    ServiceControlHandlerResult::NoError
                }
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                _ => ServiceControlHandlerResult::NotImplemented,
            },
        )?;

    status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::PARAM_CHANGE,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: std::time::Duration::default(),
        process_id: None,
    })?;

    let rt = tokio::runtime::Runtime::new()?;
    let result = rt.block_on(async {
        run_server_loop(
            config_path,
            || {
                let n = Arc::clone(&stop_notify);
                Box::pin(async move { n.notified().await })
            },
            || {
                let n = Arc::clone(&reload_notify);
                Box::pin(async move { n.notified().await })
            },
        )
        .await
    });

    status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: std::time::Duration::default(),
        process_id: None,
    })?;

    result
}
