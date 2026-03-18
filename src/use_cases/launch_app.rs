use std::time::Instant;

use crate::cli::args::LaunchArgs;
use crate::config::model::AppConfig;
use crate::domain::launch::{LaunchMode, LaunchResult};
use crate::output::json::Envelope;
use crate::output::presenter::Presenter;
use crate::platform::connection::V8Connection;
use crate::platform::locator::UtilityType;
use crate::platform::process::ProcessRequest;
use crate::platform::utilities::PlatformUtilities;
use crate::support::error::AppError;

pub fn execute(
    config: &AppConfig,
    args: &LaunchArgs,
    presenter: &Presenter,
) -> Result<(), AppError> {
    let started = Instant::now();
    let (mode, utility, command_mode) = match args.mode.as_str() {
        "designer" => (LaunchMode::Designer, UtilityType::V8, "DESIGNER"),
        "thin" => (LaunchMode::Thin, UtilityType::V8C, "ENTERPRISE"),
        "thick" => (LaunchMode::Thick, UtilityType::V8, "ENTERPRISE"),
        other => {
            return Err(AppError::Validation(format!(
                "unsupported launch mode: {other}"
            )));
        }
    };

    let mut utilities = PlatformUtilities::from_config(config);
    let location = utilities
        .locate(utility)
        .map_err(|e| AppError::Platform(e.to_string()))?;

    let mut process_args = vec![command_mode.to_owned()];
    process_args.extend(V8Connection::from_connection_string(&config.connection).args());

    let spawned = utilities
        .runner_for(utility)
        .spawn(&ProcessRequest {
            program: location.path.clone(),
            args: process_args,
            workdir: None,
            stdout_log_path: None,
            stderr_log_path: None,
        })
        .map_err(|e| AppError::Platform(e.to_string()))?;

    let result = LaunchResult {
        ok: true,
        mode,
        pid: Some(spawned.pid),
        binary: spawned.binary.clone(),
        message: Some(format!(
            "Launched {} via {} (pid {})",
            args.mode,
            spawned.binary.display(),
            spawned.pid
        )),
    };

    let duration_ms = started.elapsed().as_millis() as u64;
    if presenter.is_json() {
        presenter.print_envelope(&Envelope::ok("launch", duration_ms, result));
    } else {
        presenter.print_ok(
            result
                .message
                .as_deref()
                .unwrap_or("Launched application successfully"),
        );
    }

    Ok(())
}
