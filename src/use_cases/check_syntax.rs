use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::cli::args::{
    DesignerConfigSyntaxArgs, DesignerModulesSyntaxArgs, SyntaxArgs, SyntaxTarget,
};
use crate::config::model::{AppConfig, BuilderBackend, SourceFormat};
use crate::domain::issue::{Issue, IssueSeverity, ObjectIssue};
use crate::domain::syntax::{SyntaxCheckResult, SyntaxCheckStatus, SyntaxIssueSummary};
use crate::output::json::Envelope;
use crate::output::presenter::Presenter;
use crate::parsers::designer_validation;
use crate::platform::connection::V8Connection;
use crate::platform::designer::DesignerDsl;
use crate::platform::locator::UtilityType;
use crate::platform::result::PlatformCommandResult;
use crate::platform::utilities::PlatformUtilities;
use crate::support::error::AppError;
use crate::support::temp::platform_logs_dir;

const SYNTAX_COMMAND: &str = "syntax";
const SUPPORTED_SYNTAX_ERROR: &str =
    "syntax currently supports only builder=DESIGNER and format=DESIGNER";
const EDT_DEFERRED_ERROR: &str = "syntax edt is deferred to a later stage and is not implemented";
static LOG_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub fn execute(
    config: &AppConfig,
    args: &SyntaxArgs,
    presenter: &Presenter,
) -> Result<(), AppError> {
    let result = match run_syntax(config, args) {
        Ok(result) => result,
        Err(failure) => {
            if presenter.is_json() {
                presenter.print_envelope(&Envelope::err(
                    SYNTAX_COMMAND,
                    failure.result.duration_ms,
                    failure.result.clone(),
                ));
            } else {
                render_text_result(&failure.result, presenter);
                presenter.print_error(&failure.error.to_string());
            }
            return Err(failure.error);
        }
    };

    if presenter.is_json() {
        presenter.print_envelope(&Envelope::ok(
            SYNTAX_COMMAND,
            result.duration_ms,
            result,
        ));
    } else {
        render_text_result(&result, presenter);
    }

    Ok(())
}

#[derive(Debug)]
struct SyntaxExecutionFailure {
    error: AppError,
    result: SyntaxCheckResult,
}

#[derive(Debug)]
struct DesignerInvocation {
    kind: DesignerCommandKind,
    flags: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
enum DesignerCommandKind {
    Config,
    Modules,
    Edt,
}

impl DesignerCommandKind {
    fn check_name(self) -> &'static str {
        match self {
            Self::Config => "designer-config",
            Self::Modules => "designer-modules",
            Self::Edt => "edt",
        }
    }
}

fn run_syntax(
    config: &AppConfig,
    args: &SyntaxArgs,
) -> Result<SyntaxCheckResult, SyntaxExecutionFailure> {
    let started = Instant::now();
    let invocation = match normalize_invocation(args) {
        Ok(invocation) => invocation,
        Err((kind, error)) => {
            let error_message = error.to_string();
            return Err(SyntaxExecutionFailure {
                result: failed_result(
                    kind.check_name(),
                    SyntaxCheckStatus::ToolFailed,
                    -1,
                    started,
                    vec![],
                    None,
                    Some(error_message),
                    None,
                ),
                error,
            });
        }
    };

    if let Some(error) = validate_supported_matrix(config) {
        return Err(SyntaxExecutionFailure {
            result: failed_result(
                invocation.kind.check_name(),
                SyntaxCheckStatus::ToolFailed,
                -1,
                started,
                vec![],
                None,
                Some(error.to_string()),
                None,
            ),
            error,
        });
    }

    let log_dir = match platform_logs_dir(&config.work_path) {
        Ok(dir) => dir,
        Err(error) => {
            let app_error = AppError::Runtime(format!(
                "failed to prepare syntax platform logs directory '{}': {error}",
                config.work_path.display()
            ));
            return Err(SyntaxExecutionFailure {
                result: failed_result(
                    invocation.kind.check_name(),
                    SyntaxCheckStatus::ToolFailed,
                    -1,
                    started,
                    vec![],
                    None,
                    Some(app_error.to_string()),
                    None,
                ),
                error: app_error,
            });
        }
    };

    let log_path = unique_log_path(&log_dir, invocation.kind.check_name());

    let mut utilities = PlatformUtilities::from_config(config);
    let location = match utilities.locate(UtilityType::V8) {
        Ok(location) => location,
        Err(error) => {
            let message = error.to_string();
            let app_error = AppError::Platform(message.clone());
            return Err(SyntaxExecutionFailure {
                result: failed_result(
                    invocation.kind.check_name(),
                    SyntaxCheckStatus::ToolFailed,
                    -1,
                    started,
                    vec![],
                    None,
                    Some(message),
                    Some(log_path),
                ),
                error: app_error,
            });
        }
    };

    let runner = utilities.runner_for(UtilityType::V8);
    let dsl = DesignerDsl::new(
        location.path,
        V8Connection::from_connection_string(&config.connection),
        runner,
        Some(log_path.clone()),
    );

    let flags: Vec<&str> = invocation.flags.iter().map(String::as_str).collect();
    let platform_result = match invocation.kind {
        DesignerCommandKind::Config => dsl.check_config(&flags),
        DesignerCommandKind::Modules => dsl.check_modules(&flags),
        DesignerCommandKind::Edt => unreachable!("EDT syntax is rejected before execution"),
    };

    let platform_result = match platform_result {
        Ok(result) => result,
        Err(error) => {
            let message = error.to_string();
            let app_error = AppError::Platform(message.clone());
            return Err(SyntaxExecutionFailure {
                result: failed_result(
                    invocation.kind.check_name(),
                    SyntaxCheckStatus::ToolFailed,
                    -1,
                    started,
                    vec![],
                    None,
                    Some(message),
                    Some(log_path),
                ),
                error: app_error,
            });
        }
    };

    let result = build_result(invocation.kind.check_name(), platform_result, started);
    match result.status {
        SyntaxCheckStatus::Clean => Ok(result),
        SyntaxCheckStatus::IssuesFound | SyntaxCheckStatus::ToolFailed => Err(SyntaxExecutionFailure {
            error: AppError::Runtime(format!(
                "syntax check '{}' finished with status {:?} (designer exit code {})",
                result.check_name, result.status, result.exit_code
            )),
            result,
        }),
    }
}

fn normalize_invocation(
    args: &SyntaxArgs,
) -> Result<DesignerInvocation, (DesignerCommandKind, AppError)> {
    match &args.target {
        SyntaxTarget::DesignerConfig(config_args) => Ok(DesignerInvocation {
            kind: DesignerCommandKind::Config,
            flags: normalize_config_flags(config_args),
        }),
        SyntaxTarget::DesignerModules(module_args) => {
            if !modules_has_modes(module_args) {
                return Err((
                    DesignerCommandKind::Modules,
                    AppError::Validation(
                        "syntax designer-modules requires at least one mode flag".to_owned(),
                    ),
                ));
            }

            Ok(DesignerInvocation {
                kind: DesignerCommandKind::Modules,
                flags: normalize_modules_flags(module_args),
            })
        }
        SyntaxTarget::Edt { .. } => Err((
            DesignerCommandKind::Edt,
            AppError::Validation(EDT_DEFERRED_ERROR.to_owned()),
        )),
    }
}

fn normalize_config_flags(args: &DesignerConfigSyntaxArgs) -> Vec<String> {
    let mut flags = Vec::new();
    push_flag(&mut flags, args.config_log_integrity, "-ConfigLogIntegrity");
    push_flag(&mut flags, args.incorrect_references, "-IncorrectReferences");
    push_flag(&mut flags, args.thin_client, "-ThinClient");
    push_flag(&mut flags, args.web_client, "-WebClient");
    push_flag(&mut flags, args.mobile_client, "-MobileClient");
    push_flag(&mut flags, args.server, "-Server");
    push_flag(&mut flags, args.external_connection, "-ExternalConnection");
    push_flag(
        &mut flags,
        args.external_connection_server,
        "-ExternalConnectionServer",
    );
    push_flag(&mut flags, args.mobile_app_client, "-MobileAppClient");
    push_flag(&mut flags, args.mobile_app_server, "-MobileAppServer");
    push_flag(
        &mut flags,
        args.thick_client_managed_application,
        "-ThickClientManagedApplication",
    );
    push_flag(
        &mut flags,
        args.thick_client_server_managed_application,
        "-ThickClientServerManagedApplication",
    );
    push_flag(
        &mut flags,
        args.thick_client_ordinary_application,
        "-ThickClientOrdinaryApplication",
    );
    push_flag(
        &mut flags,
        args.thick_client_server_ordinary_application,
        "-ThickClientServerOrdinaryApplication",
    );
    push_flag(&mut flags, args.mobile_client_digi_sign, "-MobileClientDigiSign");
    push_flag(&mut flags, args.distributive_modules, "-DistributiveModules");
    push_flag(&mut flags, args.unreference_procedures, "-UnreferenceProcedures");
    push_flag(&mut flags, args.handlers_existence, "-HandlersExistence");
    push_flag(&mut flags, args.empty_handlers, "-EmptyHandlers");
    push_flag(&mut flags, args.extended_modules_check, "-ExtendedModulesCheck");
    push_flag(
        &mut flags,
        args.check_use_synchronous_calls,
        "-CheckUseSynchronousCalls",
    );
    push_flag(&mut flags, args.check_use_modality, "-CheckUseModality");
    push_flag(
        &mut flags,
        args.unsupported_functional,
        "-UnsupportedFunctional",
    );
    push_extension_scope(&mut flags, args.extension.as_deref(), args.all_extensions);
    flags
}

fn normalize_modules_flags(args: &DesignerModulesSyntaxArgs) -> Vec<String> {
    let mut flags = Vec::new();
    push_flag(&mut flags, args.thin_client, "-ThinClient");
    push_flag(&mut flags, args.web_client, "-WebClient");
    push_flag(&mut flags, args.server, "-Server");
    push_flag(&mut flags, args.external_connection, "-ExternalConnection");
    push_flag(
        &mut flags,
        args.thick_client_ordinary_application,
        "-ThickClientOrdinaryApplication",
    );
    push_flag(&mut flags, args.mobile_app_client, "-MobileAppClient");
    push_flag(&mut flags, args.mobile_app_server, "-MobileAppServer");
    push_flag(&mut flags, args.mobile_client, "-MobileClient");
    push_flag(&mut flags, args.extended_modules_check, "-ExtendedModulesCheck");
    push_extension_scope(&mut flags, args.extension.as_deref(), args.all_extensions);
    flags
}

fn push_flag(flags: &mut Vec<String>, enabled: bool, flag: &str) {
    if enabled {
        flags.push(flag.to_owned());
    }
}

fn push_extension_scope(flags: &mut Vec<String>, extension: Option<&str>, all_extensions: bool) {
    if let Some(extension) = extension {
        flags.push("-Extension".to_owned());
        flags.push(extension.to_owned());
    }
    if all_extensions {
        flags.push("-AllExtensions".to_owned());
    }
}

fn modules_has_modes(args: &DesignerModulesSyntaxArgs) -> bool {
    args.thin_client
        || args.web_client
        || args.server
        || args.external_connection
        || args.thick_client_ordinary_application
        || args.mobile_app_client
        || args.mobile_app_server
        || args.mobile_client
        || args.extended_modules_check
}

fn validate_supported_matrix(config: &AppConfig) -> Option<AppError> {
    if config.builder != BuilderBackend::Designer || config.format != SourceFormat::Designer {
        Some(AppError::Validation(SUPPORTED_SYNTAX_ERROR.to_owned()))
    } else {
        None
    }
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

fn build_result(
    check_name: &str,
    platform_result: PlatformCommandResult,
    started: Instant,
) -> SyntaxCheckResult {
    let PlatformCommandResult {
        process,
        platform_log_path,
        platform_log,
        platform_log_read_error,
    } = platform_result;
    let exit_code = process.exit_code;
    let stderr = (!process.stderr.trim().is_empty()).then_some(process.stderr);
    let mut issues = platform_log
        .as_deref()
        .map(designer_validation::parse)
        .unwrap_or_default();
    let log_read_warning = platform_log_read_error;
    let status = status_from_exit_code(exit_code);

    if exit_code != 0 && issues.is_empty() {
        issues.push(fallback_issue(
            exit_code,
            stderr.as_deref(),
            log_read_warning.as_deref(),
            platform_log_path.as_deref(),
        ));
    }

    SyntaxCheckResult {
        status,
        exit_code,
        check_name: check_name.to_owned(),
        summary: summarize_issues(&issues),
        issues,
        duration_ms: started.elapsed().as_millis() as u64,
        platform_log_path,
        stderr,
        log_read_warning,
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
        duration_ms: started.elapsed().as_millis() as u64,
        platform_log_path,
        stderr,
        log_read_warning,
    }
}

fn status_from_exit_code(exit_code: i32) -> SyntaxCheckStatus {
    match exit_code {
        0 => SyntaxCheckStatus::Clean,
        101 => SyntaxCheckStatus::IssuesFound,
        _ => SyntaxCheckStatus::ToolFailed,
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

fn fallback_issue(
    exit_code: i32,
    stderr: Option<&str>,
    log_read_warning: Option<&str>,
    platform_log_path: Option<&Path>,
) -> Issue {
    let message = if let Some(log_read_warning) = log_read_warning {
        format!(
            "Designer exited with code {exit_code}; no parseable issues found; /Out log unreadable: {log_read_warning}"
        )
    } else if let Some(stderr) = stderr.filter(|stderr| !stderr.trim().is_empty()) {
        format!(
            "Designer exited with code {exit_code}; no parseable issues found; stderr: {}",
            stderr.trim()
        )
    } else if let Some(path) = platform_log_path {
        format!(
            "Designer exited with code {exit_code}; no parseable issues found in /Out log '{}'",
            path.display()
        )
    } else {
        format!("Designer exited with code {exit_code}; no parseable issues found")
    };

    Issue::Object(ObjectIssue {
        object: "Designer".to_owned(),
        message,
        severity: IssueSeverity::Error,
    })
}

fn render_text_result(result: &SyntaxCheckResult, presenter: &Presenter) {
    let summary_line = format!(
        "{}: {:?} (designer exit {}, errors {}, warnings {}, info {}, duration {} ms)",
        result.check_name,
        result.status,
        result.exit_code,
        result.summary.errors,
        result.summary.warnings,
        result.summary.info,
        result.duration_ms
    );

    match result.status {
        SyntaxCheckStatus::Clean => presenter.print_ok(&summary_line),
        SyntaxCheckStatus::IssuesFound | SyntaxCheckStatus::ToolFailed => {
            presenter.print_info(&summary_line)
        }
    }

    for issue in &result.issues {
        presenter.print_info(&render_issue(issue));
    }

    if let Some(log_read_warning) = &result.log_read_warning {
        presenter.print_info(&format!("log warning: {log_read_warning}"));
    }

    if matches!(result.status, SyntaxCheckStatus::ToolFailed) {
        if let Some(stderr) = &result.stderr {
            presenter.print_info(&format!("stderr: {}", stderr.trim()));
        }
    }
}

fn render_issue(issue: &Issue) -> String {
    match issue {
        Issue::Module(issue) => {
            let location = match (issue.line, issue.column) {
                (Some(line), Some(column)) => format!("{}:{}:{}", issue.path, line, column),
                (Some(line), None) => format!("{}:{}", issue.path, line),
                _ => issue.path.clone(),
            };
            format!(
                "{} {} {}",
                render_severity(&issue.severity),
                location,
                issue.message
            )
        }
        Issue::Object(issue) => format!(
            "{} {} {}",
            render_severity(&issue.severity),
            issue.object,
            issue.message
        ),
        Issue::Edt(issue) => {
            let location = match (issue.line, issue.column) {
                (Some(line), Some(column)) => format!("{}:{}:{}", issue.path, line, column),
                (Some(line), None) => format!("{}:{}", issue.path, line),
                _ => issue.path.clone(),
            };
            format!(
                "{} {} {}",
                render_severity(&issue.severity),
                location,
                issue.message
            )
        }
    }
}

fn render_severity(severity: &IssueSeverity) -> &'static str {
    match severity {
        IssueSeverity::Error => "ERROR",
        IssueSeverity::Warning => "WARNING",
        IssueSeverity::Info => "INFO",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        modules_has_modes, normalize_config_flags, normalize_modules_flags, run_syntax,
        status_from_exit_code,
    };
    use crate::cli::args::{
        DesignerConfigSyntaxArgs, DesignerModulesSyntaxArgs, SyntaxArgs, SyntaxTarget,
    };
    use crate::config::model::{
        AppConfig, BuildConfig, BuilderBackend, SourceFormat, SourceSetConfig, SourceSetPurpose,
        TestsConfig, ToolsConfig,
    };
    use crate::domain::issue::Issue;
    use crate::domain::syntax::SyntaxCheckStatus;
    use crate::support::error::AppError;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use tempfile::tempdir;

    fn make_executable(path: &Path) {
        let mut perms = fs::metadata(path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod");
    }

    fn write_script(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent");
        }
        fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("write");
        make_executable(path);
    }

    fn write_designer_script(path: &Path, log_body: Option<&str>, stderr: Option<&str>, exit_code: i32) {
        let log_branch = log_body.map(|body| {
            format!(
                "if [ -n \"$out\" ]; then cat <<'LOG' > \"$out\"\n{body}\nLOG\nfi"
            )
        }).unwrap_or_default();
        let stderr_branch = stderr
            .map(|stderr| format!("printf '%s\\n' '{}' >&2", stderr.replace('\'', "'\\''")))
            .unwrap_or_default();
        let body = format!(
            "out=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"/Out\" ]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\n{log_branch}\n{stderr_branch}\nexit {exit_code}"
        );
        write_script(path, &body);
    }

    fn sample_config(base_path: &Path, work_path: &Path, platform_path: &Path) -> AppConfig {
        AppConfig {
            base_path: base_path.to_path_buf(),
            work_path: work_path.to_path_buf(),
            format: SourceFormat::Designer,
            builder: BuilderBackend::Designer,
            connection: "File=/tmp/ib".to_owned(),
            source_sets: vec![SourceSetConfig {
                name: "main".to_owned(),
                purpose: SourceSetPurpose::Configuration,
                path: Path::new(".").to_path_buf(),
            }],
            build: BuildConfig::default(),
            tools: ToolsConfig {
                platform: crate::config::model::PlatformToolConfig {
                    path: Some(platform_path.to_path_buf()),
                    version: None,
                },
                edt_cli: Default::default(),
            },
            tests: TestsConfig::default(),
        }
    }

    #[test]
    fn status_mapping_matches_designer_exit_codes() {
        assert_eq!(status_from_exit_code(0), SyntaxCheckStatus::Clean);
        assert_eq!(status_from_exit_code(101), SyntaxCheckStatus::IssuesFound);
        assert_eq!(status_from_exit_code(1), SyntaxCheckStatus::ToolFailed);
    }

    #[test]
    fn normalizes_config_flags() {
        let flags = normalize_config_flags(&DesignerConfigSyntaxArgs {
            thin_client: true,
            server: true,
            extension: Some("Ext".to_owned()),
            ..default_config_args()
        });

        assert_eq!(flags, vec!["-ThinClient", "-Server", "-Extension", "Ext"]);
    }

    #[test]
    fn normalizes_modules_flags() {
        let flags = normalize_modules_flags(&DesignerModulesSyntaxArgs {
            server: true,
            all_extensions: true,
            ..default_modules_args()
        });

        assert_eq!(flags, vec!["-Server", "-AllExtensions"]);
    }

    #[test]
    fn modules_without_modes_are_rejected() {
        assert!(!modules_has_modes(&default_modules_args()));
        let args = SyntaxArgs {
            target: SyntaxTarget::DesignerModules(default_modules_args()),
        };
        let dir = tempdir().expect("tempdir");
        let config = sample_config(dir.path(), dir.path(), dir.path());

        let error = run_syntax(&config, &args).expect_err("expected failure");

        assert!(error.error.to_string().contains("requires at least one mode"));
        assert!(error.result.issues.is_empty());
    }

    #[test]
    fn unsupported_matrix_returns_validation_failure_without_fake_issue() {
        let dir = tempdir().expect("tempdir");
        let mut config = sample_config(dir.path(), dir.path(), dir.path());
        config.format = SourceFormat::Edt;
        let args = SyntaxArgs {
            target: SyntaxTarget::DesignerConfig(default_config_args()),
        };

        let error = run_syntax(&config, &args).expect_err("expected failure");

        assert!(matches!(error.error, AppError::Validation(_)));
        assert!(error.result.issues.is_empty());
    }

    #[test]
    fn clean_exit_returns_clean_status() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let binary = dir.path().join("platform").join("bin").join("1cv8");
        fs::create_dir_all(&base).expect("base");
        fs::create_dir_all(&work).expect("work");
        write_designer_script(&binary, None, None, 0);
        let config = sample_config(&base, &work, &dir.path().join("platform"));
        let args = SyntaxArgs {
            target: SyntaxTarget::DesignerConfig(default_config_args()),
        };

        let result = run_syntax(&config, &args).expect("clean run");

        assert_eq!(result.status, SyntaxCheckStatus::Clean);
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn validation_exit_preserves_parsed_issues() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let binary = dir.path().join("platform").join("bin").join("1cv8");
        fs::create_dir_all(&base).expect("base");
        fs::create_dir_all(&work).expect("work");
        write_designer_script(
            &binary,
            Some("{CommonModules.TestModule(7,2)}: Ошибка компиляции\n{1}: context"),
            None,
            101,
        );
        let config = sample_config(&base, &work, &dir.path().join("platform"));
        let args = SyntaxArgs {
            target: SyntaxTarget::DesignerModules(DesignerModulesSyntaxArgs {
                server: true,
                ..default_modules_args()
            }),
        };

        let failure = run_syntax(&config, &args).expect_err("expected validation failure");

        assert_eq!(failure.result.status, SyntaxCheckStatus::IssuesFound);
        assert_eq!(failure.result.exit_code, 101);
        assert_eq!(failure.result.issues.len(), 1);
        match &failure.result.issues[0] {
            Issue::Module(issue) => assert_eq!(issue.path, "CommonModules.TestModule"),
            _ => panic!("expected module issue"),
        }
    }

    #[test]
    fn tool_failure_preserves_stderr_and_fallback_issue() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let binary = dir.path().join("platform").join("bin").join("1cv8");
        fs::create_dir_all(&base).expect("base");
        fs::create_dir_all(&work).expect("work");
        write_designer_script(&binary, None, Some("license error"), 1);
        let config = sample_config(&base, &work, &dir.path().join("platform"));
        let args = SyntaxArgs {
            target: SyntaxTarget::DesignerModules(DesignerModulesSyntaxArgs {
                server: true,
                ..default_modules_args()
            }),
        };

        let failure = run_syntax(&config, &args).expect_err("expected tool failure");

        assert_eq!(failure.result.status, SyntaxCheckStatus::ToolFailed);
        assert_eq!(failure.result.exit_code, 1);
        assert_eq!(failure.result.issues.len(), 1);
        assert!(failure
            .result
            .stderr
            .as_deref()
            .expect("stderr")
            .contains("license error"));
    }

    #[test]
    fn unreadable_out_log_keeps_structured_failure() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        let binary = dir.path().join("platform").join("bin").join("1cv8");
        fs::create_dir_all(&base).expect("base");
        fs::create_dir_all(&work).expect("work");
        write_script(&binary, "exit 101");
        let config = sample_config(&base, &work, &dir.path().join("platform"));
        let args = SyntaxArgs {
            target: SyntaxTarget::DesignerModules(DesignerModulesSyntaxArgs {
                server: true,
                ..default_modules_args()
            }),
        };

        let failure = run_syntax(&config, &args).expect_err("expected failure");

        assert_eq!(failure.result.status, SyntaxCheckStatus::IssuesFound);
        assert!(failure.result.log_read_warning.is_some());
        assert_eq!(failure.result.issues.len(), 1);
    }

    #[test]
    fn log_directory_creation_failure_is_reported_before_spawn() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let work_file = dir.path().join("work-file");
        let binary = dir.path().join("platform").join("bin").join("1cv8");
        fs::create_dir_all(&base).expect("base");
        fs::write(&work_file, "not a directory").expect("work file");
        write_designer_script(&binary, None, None, 0);
        let config = sample_config(&base, &work_file, &dir.path().join("platform"));
        let args = SyntaxArgs {
            target: SyntaxTarget::DesignerConfig(default_config_args()),
        };

        let failure = run_syntax(&config, &args).expect_err("expected failure");

        assert_eq!(failure.result.status, SyntaxCheckStatus::ToolFailed);
        assert!(failure
            .error
            .to_string()
            .contains("failed to prepare syntax platform logs directory"));
    }

    fn default_config_args() -> DesignerConfigSyntaxArgs {
        DesignerConfigSyntaxArgs {
            config_log_integrity: false,
            incorrect_references: false,
            thin_client: false,
            web_client: false,
            mobile_client: false,
            server: false,
            external_connection: false,
            external_connection_server: false,
            mobile_app_client: false,
            mobile_app_server: false,
            thick_client_managed_application: false,
            thick_client_server_managed_application: false,
            thick_client_ordinary_application: false,
            thick_client_server_ordinary_application: false,
            mobile_client_digi_sign: false,
            distributive_modules: false,
            unreference_procedures: false,
            handlers_existence: false,
            empty_handlers: false,
            extended_modules_check: false,
            check_use_synchronous_calls: false,
            check_use_modality: false,
            unsupported_functional: false,
            extension: None,
            all_extensions: false,
        }
    }

    fn default_modules_args() -> DesignerModulesSyntaxArgs {
        DesignerModulesSyntaxArgs {
            thin_client: false,
            web_client: false,
            server: false,
            external_connection: false,
            thick_client_ordinary_application: false,
            mobile_app_client: false,
            mobile_app_server: false,
            mobile_client: false,
            extended_modules_check: false,
            extension: None,
            all_extensions: false,
        }
    }
}
