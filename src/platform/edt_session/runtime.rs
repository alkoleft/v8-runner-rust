use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::config::model::AppConfig;
use crate::platform::interactive::{
    InteractiveCommandOutput, InteractiveProcessError, InteractiveProcessExecutor,
    InteractiveProcessRequest, ShutdownOutcome,
};
use crate::platform::locator::UtilityType;
use crate::platform::utilities::PlatformUtilities;

use super::{
    EdtDrainReason, EdtRestartReason, EdtSessionDrainReason, EdtSessionError,
    EdtSessionHostOptions, EdtSessionManagerInner, EdtSessionRequest, EdtSessionResponse,
};

pub(super) trait ManagedSession: Send {
    fn pid(&self) -> Option<u32>;

    fn execute(
        &mut self,
        command: &str,
        timeout: Duration,
    ) -> Result<InteractiveCommandOutput, InteractiveProcessError>;

    fn shutdown(&mut self, timeout: Duration) -> Result<ShutdownOutcome, InteractiveProcessError>;

    fn kill(&mut self) -> Result<(), InteractiveProcessError>;
}

impl ManagedSession for InteractiveProcessExecutor {
    fn pid(&self) -> Option<u32> {
        Self::pid(self)
    }

    fn execute(
        &mut self,
        command: &str,
        timeout: Duration,
    ) -> Result<InteractiveCommandOutput, InteractiveProcessError> {
        Self::execute(self, command, timeout)
    }

    fn shutdown(&mut self, timeout: Duration) -> Result<ShutdownOutcome, InteractiveProcessError> {
        Self::shutdown(self, timeout)
    }

    fn kill(&mut self) -> Result<(), InteractiveProcessError> {
        Self::kill(self)
    }
}

pub(super) trait SessionFactory: Send + Sync {
    fn spawn_session(&self) -> Result<Box<dyn ManagedSession>, EdtSessionError>;

    fn pre_dispatch(
        &self,
        _session: &mut dyn ManagedSession,
        _request: &EdtSessionRequest,
    ) -> Result<(), EdtSessionError> {
        Ok(())
    }

    #[cfg(test)]
    fn post_mark_running(&self, _request: &EdtSessionRequest) {}
}

#[derive(Clone)]
pub(super) struct DefaultSessionFactory {
    config: AppConfig,
    options: EdtSessionHostOptions,
}

impl DefaultSessionFactory {
    pub(super) fn new(config: AppConfig, options: EdtSessionHostOptions) -> Self {
        Self { config, options }
    }
}

impl SessionFactory for DefaultSessionFactory {
    fn spawn_session(&self) -> Result<Box<dyn ManagedSession>, EdtSessionError> {
        let mut utilities = PlatformUtilities::from_config(&self.config);
        let location = utilities.locate(UtilityType::EdtCli).map_err(|error| {
            EdtSessionError::StartupFailed {
                message: error.to_string(),
            }
        })?;
        let request = InteractiveProcessRequest::new(location.path).with_args([
            "-data".to_owned(),
            self.options.workspace.display().to_string(),
        ]);
        InteractiveProcessExecutor::spawn(request, self.options.startup_timeout)
            .map(|session| Box::new(session) as Box<dyn ManagedSession>)
            .map_err(|error| EdtSessionError::StartupFailed {
                message: error.to_string(),
            })
    }

    fn pre_dispatch(
        &self,
        session: &mut dyn ManagedSession,
        request: &EdtSessionRequest,
    ) -> Result<(), EdtSessionError> {
        run_baseline_reset(
            session,
            &self.options.workspace,
            request,
            BASELINE_RESET_TIMEOUT_CAP,
        )
    }
}

pub(super) fn run_worker(
    inner: Arc<EdtSessionManagerInner>,
    factory: Arc<dyn SessionFactory>,
    prewarm: bool,
) {
    let mut session: Option<Box<dyn ManagedSession>> = None;
    if prewarm && !inner.shutdown_token.is_cancelled() {
        match factory.spawn_session() {
            Ok(new_session) => {
                inner
                    .active_pid
                    .store(new_session.pid().unwrap_or(0), Ordering::SeqCst);
                session = Some(new_session);
            }
            Err(_) => {
                inner.observer.record_startup_failure();
                inner.active_pid.store(0, Ordering::SeqCst);
            }
        }
    }
    while let Some(queued) = inner.next_request() {
        if inner.shutdown_token.is_cancelled() {
            queued.state.release_queued();
            queued.reply(Err(EdtSessionError::DrainedByRestartOrShutdown {
                reason: EdtSessionDrainReason::Shutdown,
            }));
            inner.observer.record_drain(EdtDrainReason::Shutdown, 1);
            inner.drain_pending(
                EdtSessionError::DrainedByRestartOrShutdown {
                    reason: EdtSessionDrainReason::Shutdown,
                },
                EdtSessionDrainReason::Shutdown,
            );
            break;
        }

        if queued.request.cancellation.is_cancelled() {
            queued.state.release_queued();
            queued.reply(Err(EdtSessionError::QueuedCancelled));
            continue;
        }
        if is_deadline_exhausted(queued.request.deadline) {
            queued.state.release_queued();
            queued.reply(Err(EdtSessionError::QueuedTimeout));
            continue;
        }

        if session.is_none() {
            match factory.spawn_session() {
                Ok(new_session) => {
                    inner
                        .active_pid
                        .store(new_session.pid().unwrap_or(0), Ordering::SeqCst);
                    session = Some(new_session);
                }
                Err(error) => {
                    queued.state.release_queued();
                    queued.reply(Err(error));
                    inner.observer.record_startup_failure();
                    inner.drain_pending(
                        EdtSessionError::DrainedByRestartOrShutdown {
                            reason: EdtSessionDrainReason::Restart,
                        },
                        EdtSessionDrainReason::Restart,
                    );
                    inner.active_pid.store(0, Ordering::SeqCst);
                    continue;
                }
            }
        }

        let Some(active_session) = session.as_mut() else {
            queued.reply(Err(EdtSessionError::InternalFailure {
                message: "shared EDT worker lost session after startup".to_owned(),
            }));
            continue;
        };

        if let Err(error) = factory.pre_dispatch(active_session.as_mut(), &queued.request) {
            queued.state.release_queued();
            queued.reply(Err(error.clone()));
            if !matches!(
                error,
                EdtSessionError::QueuedCancelled | EdtSessionError::QueuedTimeout
            ) {
                if kill_and_drop_session(&mut session, inner.active_pid.as_ref()) {
                    inner
                        .observer
                        .record_restart(EdtRestartReason::BaselineFailure);
                }
                inner.drain_pending(
                    EdtSessionError::DrainedByRestartOrShutdown {
                        reason: EdtSessionDrainReason::Restart,
                    },
                    EdtSessionDrainReason::Restart,
                );
            }
            continue;
        }

        if queued.request.cancellation.is_cancelled() {
            queued.state.release_queued();
            queued.reply(Err(EdtSessionError::QueuedCancelled));
            continue;
        }
        let remaining = queued
            .request
            .deadline
            .saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            queued.state.release_queued();
            queued.reply(Err(EdtSessionError::QueuedTimeout));
            continue;
        }
        if !queued.state.try_mark_running() {
            continue;
        }
        #[cfg(test)]
        factory.post_mark_running(&queued.request);
        if queued.request.cancellation.is_cancelled() {
            queued.state.finish();
            queued.reply(Err(EdtSessionError::RunningCancelled));
            continue;
        }
        let execution = active_session.execute(&queued.request.command, remaining);
        match execution {
            Ok(output) => {
                queued.state.finish();
                queued.reply(Ok(EdtSessionResponse {
                    stdout: output.stdout,
                    stderr: output.stderr,
                }));
            }
            Err(InteractiveProcessError::CommandTimeout { .. }) => {
                if kill_and_drop_session(&mut session, inner.active_pid.as_ref()) {
                    inner
                        .observer
                        .record_restart(EdtRestartReason::CommandTimeout);
                }
                inner.drain_pending(
                    EdtSessionError::DrainedByRestartOrShutdown {
                        reason: EdtSessionDrainReason::Restart,
                    },
                    EdtSessionDrainReason::Restart,
                );
                queued.state.finish();
                queued.reply(Err(EdtSessionError::RunningTimeout));
            }
            Err(error) => {
                if kill_and_drop_session(&mut session, inner.active_pid.as_ref()) {
                    inner
                        .observer
                        .record_restart(EdtRestartReason::SessionFailure);
                }
                inner.drain_pending(
                    EdtSessionError::DrainedByRestartOrShutdown {
                        reason: EdtSessionDrainReason::Restart,
                    },
                    EdtSessionDrainReason::Restart,
                );
                queued.state.finish();
                queued.reply(Err(EdtSessionError::SessionFailed {
                    message: error.to_string(),
                }));
            }
        }
    }

    shutdown_session(
        &mut session,
        inner.shutdown_timeout,
        inner.active_pid.as_ref(),
    );
    inner.active_pid.store(0, Ordering::SeqCst);
}

pub(super) fn kill_and_drop_session(
    session: &mut Option<Box<dyn ManagedSession>>,
    active_pid: &AtomicU32,
) -> bool {
    let had_live_session = session.is_some() || active_pid.load(Ordering::SeqCst) != 0;
    if let Some(mut session) = session.take() {
        if session.kill().is_err() {
            let pid = active_pid.load(Ordering::SeqCst);
            let _ = super::kill_process_group_by_pid(pid);
        }
    } else {
        let pid = active_pid.load(Ordering::SeqCst);
        let _ = super::kill_process_group_by_pid(pid);
    }
    active_pid.store(0, Ordering::SeqCst);
    had_live_session
}

pub(super) fn shutdown_session(
    session: &mut Option<Box<dyn ManagedSession>>,
    timeout: Duration,
    active_pid: &AtomicU32,
) {
    if let Some(mut session) = session.take() {
        if session.shutdown(timeout).is_err() {
            if session.kill().is_err() {
                let pid = active_pid.load(Ordering::SeqCst);
                let _ = super::kill_process_group_by_pid(pid);
            }
        }
    } else {
        let pid = active_pid.load(Ordering::SeqCst);
        let _ = super::kill_process_group_by_pid(pid);
    }
    active_pid.store(0, Ordering::SeqCst);
}

fn is_deadline_exhausted(deadline: Instant) -> bool {
    deadline <= Instant::now()
}

pub(super) fn wait_for_worker(worker: &JoinHandle<()>, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if worker.is_finished() {
            return true;
        }
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        thread::sleep(JOIN_POLL_INTERVAL.min(deadline.saturating_duration_since(now)));
    }
}

pub(super) fn run_baseline_reset(
    session: &mut dyn ManagedSession,
    workspace: &Path,
    request: &EdtSessionRequest,
    timeout_cap: Duration,
) -> Result<(), EdtSessionError> {
    if request.cancellation.is_cancelled() {
        return Err(EdtSessionError::QueuedCancelled);
    }

    let reset_timeout = baseline_timeout_budget(request.deadline, timeout_cap)?;
    let reset_output = session
        .execute(
            &super::render_interactive_change_dir_command(workspace),
            reset_timeout.duration,
        )
        .map_err(|error| baseline_error("reset", error, reset_timeout.clamped_by_budget))?;
    if !reset_output.stderr.trim().is_empty() {
        return Err(EdtSessionError::SessionFailed {
            message: format!(
                "shared EDT baseline reset produced stderr: {}",
                reset_output.stderr.trim()
            ),
        });
    }

    if request.cancellation.is_cancelled() {
        return Err(EdtSessionError::QueuedCancelled);
    }

    let probe_timeout = baseline_timeout_budget(request.deadline, timeout_cap)?;
    let probe_output = session
        .execute(
            &super::render_interactive_probe_workdir_command(),
            probe_timeout.duration,
        )
        .map_err(|error| baseline_error("probe", error, probe_timeout.clamped_by_budget))?;
    if !probe_output.stderr.trim().is_empty() {
        return Err(EdtSessionError::SessionFailed {
            message: format!(
                "shared EDT baseline probe produced stderr: {}",
                probe_output.stderr.trim()
            ),
        });
    }

    if !workspace_paths_match(probe_output.stdout.trim(), workspace) {
        return Err(EdtSessionError::SessionFailed {
            message: format!(
                "shared EDT baseline probe returned '{}' instead of '{}'",
                probe_output.stdout.trim(),
                workspace.display()
            ),
        });
    }

    Ok(())
}

fn baseline_error(
    step: &str,
    error: InteractiveProcessError,
    clamped_by_budget: bool,
) -> EdtSessionError {
    match error {
        InteractiveProcessError::CommandTimeout { .. } if clamped_by_budget => {
            EdtSessionError::QueuedTimeout
        }
        other => EdtSessionError::SessionFailed {
            message: format!("shared EDT baseline {step} failed: {other}"),
        },
    }
}

fn baseline_timeout_budget(
    deadline: Instant,
    timeout_cap: Duration,
) -> Result<BaselineTimeoutBudget, EdtSessionError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(EdtSessionError::QueuedTimeout);
    }

    Ok(BaselineTimeoutBudget {
        duration: remaining.min(timeout_cap),
        clamped_by_budget: remaining <= timeout_cap,
    })
}

struct BaselineTimeoutBudget {
    duration: Duration,
    clamped_by_budget: bool,
}

fn workspace_paths_match(actual: &str, expected: &Path) -> bool {
    let trimmed = actual.trim();
    if trimmed.is_empty() {
        return false;
    }

    let actual_path = Path::new(trimmed);
    actual_path.components().eq(expected.components())
        || std::fs::canonicalize(actual_path)
            .ok()
            .zip(std::fs::canonicalize(expected).ok())
            .is_some_and(|(actual, expected)| actual == expected)
}

const JOIN_POLL_INTERVAL: Duration = Duration::from_millis(10);
const BASELINE_RESET_TIMEOUT_CAP: Duration = Duration::from_secs(1);
