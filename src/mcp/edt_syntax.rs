use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio_util::sync::CancellationToken;

use crate::config::model::{AppConfig, BuilderBackend, SourceFormat, SourceSetConfig};
use crate::domain::issue::{EdtIssue, Issue, IssueSeverity};
use crate::domain::syntax::{SyntaxCheckResult, SyntaxCheckStatus, SyntaxIssueSummary};
use crate::parsers::edt_validation;
use crate::platform::edt::render_interactive_validate_command;
use crate::platform::edt_session::{EdtSessionError, EdtSessionManager, EdtSessionRequest};
use crate::support::error::AppError;
use crate::support::temp::platform_logs_dir;
use crate::use_cases::request::{SyntaxRequest, SyntaxTargetRequest};
use crate::use_cases::result::{UseCaseFailure, UseCaseResult};
use crate::use_cases::source_inventory::SourceSetInventory;

const SUPPORTED_EDT_SYNTAX_ERROR: &str =
    "syntax edt currently supports only builder=DESIGNER and format=EDT";
static LOG_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Executes MCP `check_syntax_edt` through the shared EDT session actor.
///
/// Transport-level queued cancellation and timeout are returned separately so the
/// MCP transport can preserve admission semantics. Running cancellation and timeout
/// wait for terminal state and are converted into the normal use-case payload contract.
pub async fn execute(
    manager: &EdtSessionManager,
    config: &AppConfig,
    request: &SyntaxRequest,
    timeout: Duration,
    cancellation: CancellationToken,
) -> Result<UseCaseResult<SyntaxCheckResult>, EdtSyntaxTransportError> {
    let started = Instant::now();
    let projects = match &request.target {
        SyntaxTargetRequest::Edt { projects, .. } => projects,
        _ => {
            let error = AppError::Validation(
                "shared EDT syntax executor requires an EDT syntax target".to_owned(),
            );
            let error_message = error.to_string();
            return Ok(Err(SyntaxExecutionFailure::with_payload(
                error,
                failed_result(
                    "edt",
                    SyntaxCheckStatus::ToolFailed,
                    -1,
                    started,
                    vec![],
                    None,
                    Some(error_message),
                    None,
                ),
            )));
        }
    };

    if config.builder != BuilderBackend::Designer || config.format != SourceFormat::Edt {
        let error = AppError::Validation(SUPPORTED_EDT_SYNTAX_ERROR.to_owned());
        return Ok(Err(SyntaxExecutionFailure::with_payload(
            error,
            failed_result(
                "edt",
                SyntaxCheckStatus::ToolFailed,
                -1,
                started,
                vec![],
                None,
                Some(format!("validation error: {SUPPORTED_EDT_SYNTAX_ERROR}")),
                None,
            ),
        )));
    }

    let inventory = SourceSetInventory::new(config);
    let source_sets = match resolve_edt_source_sets(&inventory, projects) {
        Ok(source_sets) => source_sets,
        Err(error) => {
            let error_message = error.to_string();
            return Ok(Err(SyntaxExecutionFailure::with_payload(
                error,
                failed_result(
                    "edt",
                    SyntaxCheckStatus::ToolFailed,
                    -1,
                    started,
                    vec![],
                    None,
                    Some(error_message),
                    None,
                ),
            )));
        }
    };

    let log_dir = match platform_logs_dir(&config.work_path) {
        Ok(dir) => dir,
        Err(error) => {
            let app_error = AppError::Runtime(format!(
                "failed to prepare syntax platform logs directory '{}': {error}",
                config.work_path.display()
            ));
            let error_message = app_error.to_string();
            return Ok(Err(SyntaxExecutionFailure::with_payload(
                app_error,
                failed_result(
                    "edt",
                    SyntaxCheckStatus::ToolFailed,
                    -1,
                    started,
                    vec![],
                    None,
                    Some(error_message),
                    None,
                ),
            )));
        }
    };

    let deadline = std::time::Instant::now() + timeout;
    let mut issues = Vec::new();
    let mut status = SyntaxCheckStatus::Clean;
    let mut exit_code = 0;
    let mut stderr_lines = Vec::new();
    let mut log_warnings = Vec::new();
    let mut single_platform_log_path = None;
    let single_source_set = source_sets.len() == 1;

    for source_set in source_sets {
        let source_path = inventory.source_path(source_set);
        let log_path = unique_log_path(
            &log_dir,
            &format!("edt_{}", source_set.name.replace(' ', "_")),
        );
        let command = render_interactive_validate_command(&source_path, &log_path);
        let execution = manager
            .execute_observed(
                EdtSessionRequest::new(command, deadline).with_cancellation(cancellation.clone()),
            )
            .await;
        let response = match execution.result {
            Ok(response) => response,
            Err(EdtSessionError::QueuedCancelled) => {
                return Err(EdtSyntaxTransportError::QueuedCancelled);
            }
            Err(EdtSessionError::QueuedTimeout) => {
                return Err(EdtSyntaxTransportError::QueuedTimeout);
            }
            Err(EdtSessionError::RunningCancelled) => {
                if let Some(completion) = execution.completion {
                    completion.wait().await;
                }
                let message = format!(
                    "execution cancelled for command 'syntax' while shared EDT command was running; terminal state was observed before returning the result"
                );
                return Ok(Err(SyntaxExecutionFailure::with_payload(
                    AppError::Runtime(message.clone()),
                    failed_result(
                        "edt",
                        SyntaxCheckStatus::ToolFailed,
                        -1,
                        started,
                        vec![],
                        None,
                        Some(message),
                        single_source_set.then_some(log_path.clone()),
                    ),
                )));
            }
            Err(EdtSessionError::RunningTimeout) => {
                if let Some(completion) = execution.completion {
                    completion.wait().await;
                }
                let message = format!(
                    "execution timeout expired for command 'syntax' while shared EDT command was running; terminal state was observed before returning the result"
                );
                return Ok(Err(SyntaxExecutionFailure::with_payload(
                    AppError::Runtime(message.clone()),
                    failed_result(
                        "edt",
                        SyntaxCheckStatus::ToolFailed,
                        -1,
                        started,
                        vec![],
                        None,
                        Some(message),
                        single_source_set.then_some(log_path.clone()),
                    ),
                )));
            }
            Err(error) => {
                let message = error.to_string();
                let app_error = AppError::Runtime(message.clone());
                return Ok(Err(SyntaxExecutionFailure::with_payload(
                    app_error,
                    failed_result(
                        "edt",
                        SyntaxCheckStatus::ToolFailed,
                        -1,
                        started,
                        vec![],
                        None,
                        Some(message),
                        Some(log_path),
                    ),
                )));
            }
        };

        if single_source_set {
            single_platform_log_path = Some(log_path.clone());
        }

        let mut project_output = Vec::new();
        if !response.stderr.trim().is_empty() {
            let line = format!("{}: {}", source_set.name, response.stderr.trim());
            stderr_lines.push(line.clone());
            project_output.push(line);
        }
        if !response.stdout.trim().is_empty() {
            project_output.push(format!(
                "{} stdout: {}",
                source_set.name,
                response.stdout.trim()
            ));
        }

        let (platform_log, log_read_warning) = match std::fs::read_to_string(&log_path) {
            Ok(contents) => (Some(contents), None),
            Err(error) => (
                None,
                Some(format!(
                    "failed to read edt --file log '{}': {error}",
                    log_path.display()
                )),
            ),
        };
        if let Some(log_warning) = &log_read_warning {
            log_warnings.push(format!("{}: {log_warning}", source_set.name));
        }

        let project_issues = platform_log
            .as_deref()
            .map(edt_validation::parse)
            .unwrap_or_default();
        let project_status = actor_status_from_result(
            response.stdout.trim(),
            response.stderr.trim(),
            &project_issues,
        );
        status = combine_status(status, project_status);
        let project_exit_code = actor_exit_code(project_status);

        if project_exit_code != 0
            && (project_status == SyntaxCheckStatus::ToolFailed || exit_code == 0)
        {
            exit_code = project_exit_code;
        }

        if project_status == SyntaxCheckStatus::ToolFailed && project_issues.is_empty() {
            issues.push(fallback_edt_issue(
                &source_set.name,
                project_exit_code,
                (!project_output.is_empty()).then(|| project_output.join("\n")),
                log_read_warning.as_deref(),
                Some(log_path.as_path()),
            ));
        } else {
            issues.extend(project_issues);
        }
    }

    let stderr = (!stderr_lines.is_empty()).then_some(stderr_lines.join("\n"));
    let log_read_warning = (!log_warnings.is_empty()).then_some(log_warnings.join("\n"));
    let result = SyntaxCheckResult {
        status,
        exit_code,
        check_name: "edt".to_owned(),
        summary: summarize_issues(&issues),
        issues,
        duration_ms: elapsed_millis(started),
        platform_log_path: single_platform_log_path,
        stderr,
        log_read_warning,
    };

    match result.status {
        SyntaxCheckStatus::Clean => Ok(Ok(result)),
        SyntaxCheckStatus::IssuesFound | SyntaxCheckStatus::ToolFailed => {
            Ok(Err(SyntaxExecutionFailure::with_payload(
                AppError::Runtime(format!(
                    "syntax check '{}' finished with status {:?} (exit code {})",
                    result.check_name, result.status, result.exit_code
                )),
                result,
            )))
        }
    }
}

type SyntaxExecutionFailure = UseCaseFailure<SyntaxCheckResult>;

fn resolve_edt_source_sets<'a>(
    inventory: &SourceSetInventory<'a>,
    projects: &[String],
) -> Result<Vec<&'a SourceSetConfig>, AppError> {
    if !inventory.has_edt_contexts() {
        return Err(AppError::Validation(
            "syntax edt requires at least one source-set".to_owned(),
        ));
    }

    if projects.is_empty() {
        return Ok(inventory.source_sets());
    }

    let mut selected = Vec::new();
    let mut unknown = Vec::new();

    for project in projects {
        if let Some(source_set) = inventory.source_set(project) {
            selected.push(source_set);
        } else {
            unknown.push(project.clone());
        }
    }

    if !unknown.is_empty() {
        return Err(AppError::Validation(format!(
            "unknown EDT source-set(s): {}",
            unknown.join(", ")
        )));
    }

    Ok(selected)
}

fn unique_log_path(dir: &Path, check_name: &str) -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let sequence = LOG_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    dir.join(format!(
        "syntax_{}_{}_{}_{}.log",
        check_name,
        timestamp,
        std::process::id(),
        sequence
    ))
}

fn actor_status_from_result(stdout: &str, stderr: &str, issues: &[Issue]) -> SyntaxCheckStatus {
    if !stderr.is_empty() {
        SyntaxCheckStatus::ToolFailed
    } else if !issues.is_empty() {
        SyntaxCheckStatus::IssuesFound
    } else if !stdout.is_empty() {
        SyntaxCheckStatus::ToolFailed
    } else {
        SyntaxCheckStatus::Clean
    }
}

fn actor_exit_code(status: SyntaxCheckStatus) -> i32 {
    match status {
        SyntaxCheckStatus::Clean => 0,
        SyntaxCheckStatus::IssuesFound => 101,
        SyntaxCheckStatus::ToolFailed => -1,
    }
}

fn combine_status(current: SyntaxCheckStatus, next: SyntaxCheckStatus) -> SyntaxCheckStatus {
    match (current, next) {
        (SyntaxCheckStatus::ToolFailed, _) | (_, SyntaxCheckStatus::ToolFailed) => {
            SyntaxCheckStatus::ToolFailed
        }
        (SyntaxCheckStatus::IssuesFound, _) | (_, SyntaxCheckStatus::IssuesFound) => {
            SyntaxCheckStatus::IssuesFound
        }
        _ => SyntaxCheckStatus::Clean,
    }
}

fn failed_result(
    check_name: &str,
    status: SyntaxCheckStatus,
    exit_code: i32,
    started: Instant,
    issues: Vec<Issue>,
    log_read_warning: Option<String>,
    stderr: Option<String>,
    platform_log_path: Option<PathBuf>,
) -> SyntaxCheckResult {
    SyntaxCheckResult {
        status,
        exit_code,
        check_name: check_name.to_owned(),
        summary: summarize_issues(&issues),
        issues,
        duration_ms: elapsed_millis(started),
        platform_log_path,
        stderr,
        log_read_warning,
    }
}

fn summarize_issues(issues: &[Issue]) -> SyntaxIssueSummary {
    let mut summary = SyntaxIssueSummary {
        errors: 0,
        warnings: 0,
        info: 0,
    };

    for issue in issues {
        match issue_severity(issue) {
            IssueSeverity::Error => summary.errors += 1,
            IssueSeverity::Warning => summary.warnings += 1,
            IssueSeverity::Info => summary.info += 1,
        }
    }

    summary
}

fn issue_severity(issue: &Issue) -> &IssueSeverity {
    match issue {
        Issue::Module(issue) => &issue.severity,
        Issue::Object(issue) => &issue.severity,
        Issue::Edt(issue) => &issue.severity,
    }
}

fn fallback_edt_issue(
    project_name: &str,
    exit_code: i32,
    stderr: Option<String>,
    log_read_warning: Option<&str>,
    platform_log_path: Option<&Path>,
) -> Issue {
    let message = if let Some(log_read_warning) = log_read_warning {
        format!(
            "EDT check for project '{project_name}' exited with code {exit_code}; no parseable issues found; --file log unreadable: {log_read_warning}"
        )
    } else if let Some(stderr) = stderr.as_deref().filter(|stderr| !stderr.trim().is_empty()) {
        format!(
            "EDT check for project '{project_name}' exited with code {exit_code}; no parseable issues found; stderr: {}",
            stderr.trim()
        )
    } else if let Some(path) = platform_log_path {
        format!(
            "EDT check for project '{project_name}' exited with code {exit_code}; no parseable issues found in --file log '{}'",
            path.display()
        )
    } else {
        format!(
            "EDT check for project '{project_name}' exited with code {exit_code}; no parseable issues found"
        )
    };

    Issue::Edt(EdtIssue {
        path: project_name.to_owned(),
        line: None,
        column: None,
        message,
        severity: IssueSeverity::Error,
        check: None,
    })
}

fn elapsed_millis(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

pub(crate) enum EdtSyntaxTransportError {
    QueuedCancelled,
    QueuedTimeout,
}
