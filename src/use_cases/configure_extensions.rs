use std::time::Instant;

use crate::config::model::{AppConfig, SourceFormat, SourceSetConfig, SourceSetPurpose};
use crate::domain::extensions::{ExtensionsResult, ExtensionsStep};
use crate::platform::ibcmd::{IbcmdConnection, IbcmdDsl, IbcmdError};
use crate::platform::locator::UtilityType;
use crate::platform::utilities::PlatformUtilities;
use crate::support::error::AppError;
use crate::use_cases::context::ExecutionContext;
use crate::use_cases::ibcmd_diagnostics::format_ibcmd_failure_details;
use crate::use_cases::request::ConfigureExtensionsRequest;
use crate::use_cases::result::{UseCaseFailure, UseCaseResult};
use tracing::{debug, info};

const DISABLE_SAFETY_ACTION: &str = "disable_safety";
const EXTENSIONS_SUCCESS_LABEL: &str = "Extension properties updated successfully";
const EXTENSIONS_FAILURE_LABEL: &str = "Extension property update failed";

pub fn execute(
    context: &ExecutionContext,
    config: &AppConfig,
    args: &ConfigureExtensionsRequest,
) -> UseCaseResult<ExtensionsResult> {
    debug!(
        command = context.command().as_str(),
        transport = ?context.transport(),
        "executing configure extensions use case"
    );
    let started = Instant::now();
    let targets = match resolve_targets(config, args) {
        Ok(targets) => targets,
        Err(error) => {
            return Err(UseCaseFailure::without_payload(error));
        }
    };

    let connection = match IbcmdConnection::from_infobase(&config.infobase) {
        Ok(connection) => connection,
        Err(error) => {
            let error = match error {
                IbcmdError::MissingServerDbmsField(_) => AppError::Validation(error.to_string()),
                IbcmdError::Spawn(_) => AppError::Platform(error.to_string()),
            };
            return Err(UseCaseFailure::without_payload(error));
        }
    };

    let mut utilities = PlatformUtilities::from_config(config);
    let binary = match utilities.locate(UtilityType::Ibcmd) {
        Ok(location) => location.path,
        Err(error) => {
            return Err(UseCaseFailure::without_payload(AppError::Platform(
                error.to_string(),
            )));
        }
    };
    let dsl = IbcmdDsl::new(binary, connection, utilities.runner_for(UtilityType::Ibcmd));

    let mut steps = Vec::new();
    for target in targets {
        let step_started = Instant::now();
        debug!(
            target = target.as_str(),
            "configuring extension safety flags"
        );
        log_extension_progress(
            &target,
            DISABLE_SAFETY_ACTION,
            "running",
            "updating extension properties",
        );
        match dsl.infobase_extension_update_properties(&target, false, false) {
            Ok(result) if result.process.exit_code == 0 => {
                let step = ExtensionsStep {
                    target,
                    action: DISABLE_SAFETY_ACTION.to_owned(),
                    ok: true,
                    message: Some(
                        "безопасный режим и защита от опасных действий отключены".to_owned(),
                    ),
                    duration_ms: step_started.elapsed().as_millis() as u64,
                };
                log_extension_step(&step);
                steps.push(step);
            }
            Ok(result) => {
                let message = format_ibcmd_failure_details(
                    "extension update",
                    "extension",
                    &target,
                    result.process.exit_code,
                    &result.process.stdout,
                    &result.process.stderr,
                    None,
                    None,
                );
                let step = ExtensionsStep {
                    target: target.clone(),
                    action: DISABLE_SAFETY_ACTION.to_owned(),
                    ok: false,
                    message: Some(message.clone()),
                    duration_ms: step_started.elapsed().as_millis() as u64,
                };
                log_extension_step(&step);
                steps.push(step);
                log_extensions_summary(false);
                let payload = ExtensionsResult {
                    ok: false,
                    steps,
                    duration_ms: started.elapsed().as_millis() as u64,
                };
                return Err(UseCaseFailure::with_payload(
                    AppError::Platform(message),
                    payload,
                ));
            }
            Err(error) => {
                let message =
                    format!("ibcmd extension update failed for extension '{target}': {error}");
                let step = ExtensionsStep {
                    target: target.clone(),
                    action: DISABLE_SAFETY_ACTION.to_owned(),
                    ok: false,
                    message: Some(message.clone()),
                    duration_ms: step_started.elapsed().as_millis() as u64,
                };
                log_extension_step(&step);
                steps.push(step);
                log_extensions_summary(false);
                let payload = ExtensionsResult {
                    ok: false,
                    steps,
                    duration_ms: started.elapsed().as_millis() as u64,
                };
                return Err(UseCaseFailure::with_payload(
                    AppError::Platform(message),
                    payload,
                ));
            }
        }
    }

    log_extensions_summary(true);
    Ok(ExtensionsResult {
        ok: true,
        steps,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

fn log_extension_step(step: &ExtensionsStep) {
    log_extension_progress(
        &step.target,
        &step.action,
        if step.ok { "succeeded" } else { "failed" },
        step.message.as_deref().unwrap_or("ok"),
    );
}

fn log_extension_progress(target: &str, action: &str, status: &str, detail: &str) {
    let label = format!("{target}: {action}");
    info!(
        timeline_status = status,
        timeline_label = label.as_str(),
        timeline_detail = detail
    );
}

fn log_extensions_summary(ok: bool) {
    info!(
        timeline_status = if ok { "succeeded" } else { "failed" },
        timeline_label = if ok {
            EXTENSIONS_SUCCESS_LABEL
        } else {
            EXTENSIONS_FAILURE_LABEL
        },
    );
}

fn resolve_targets(
    config: &AppConfig,
    args: &ConfigureExtensionsRequest,
) -> Result<Vec<String>, AppError> {
    let available = config
        .source_sets
        .iter()
        .filter(|source_set| source_set.purpose == SourceSetPurpose::Extension)
        .map(|source_set| {
            (
                source_set.name.as_str(),
                resolve_extension_name(config, source_set),
            )
        })
        .collect::<Vec<_>>();

    if args.names.is_empty() {
        return Ok(available.into_iter().map(|(_, name)| name).collect());
    }

    let mut targets = Vec::new();
    for requested in &args.names {
        let Some((_, resolved)) = available
            .iter()
            .find(|(name, _)| *name == requested.as_str())
        else {
            return Err(AppError::Validation(format!(
                "unknown extension source-set '{requested}'"
            )));
        };
        targets.push(resolved.clone());
    }
    Ok(targets)
}

fn resolve_extension_name(config: &AppConfig, source_set: &SourceSetConfig) -> String {
    if config.format != SourceFormat::Edt {
        return source_set.name.clone();
    }

    let project_file = config.base_path.join(&source_set.path).join(".project");
    std::fs::read_to_string(project_file)
        .ok()
        .and_then(|contents| extract_xml_tag_text(&contents, "name"))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| source_set.name.clone())
}

fn extract_xml_tag_text(contents: &str, tag_name: &str) -> Option<String> {
    let open_tag = format!("<{tag_name}>");
    let close_tag = format!("</{tag_name}>");
    let start = contents.find(&open_tag)? + open_tag.len();
    let rest = &contents[start..];
    let end = rest.find(&close_tag)?;
    Some(rest[..end].trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::{execute, resolve_targets};
    use crate::config::model::{
        AppConfig, BuildConfig, BuilderBackend, PlatformToolConfig, SourceFormat, SourceSetConfig,
        SourceSetPurpose, TestsConfig, ToolsConfig,
    };
    use crate::use_cases::context::{CommandName, ExecutionContext};
    use crate::use_cases::request::ConfigureExtensionsRequest;
    use crate::use_cases::result::UseCaseErrorKind;
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod");
    }

    #[cfg(unix)]
    fn write_script(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create dirs");
        }
        fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("write script");
        make_executable(path);
    }

    fn sample_config(base: &Path, work: &Path, ibcmd_path: &Path) -> AppConfig {
        AppConfig {
            base_path: base.to_path_buf(),
            work_path: work.to_path_buf(),
            format: SourceFormat::Edt,
            builder: BuilderBackend::Designer,
            infobase: crate::config::model::InfobaseConfig::file("File=/tmp/ib"),
            source_sets: vec![
                SourceSetConfig {
                    name: "configuration".to_owned(),
                    purpose: SourceSetPurpose::Configuration,
                    path: PathBuf::from("configuration"),
                },
                SourceSetConfig {
                    name: "client_mcp".to_owned(),
                    purpose: SourceSetPurpose::Extension,
                    path: PathBuf::from("exts/client-mcp"),
                },
            ],
            build: BuildConfig::default(),
            tools: ToolsConfig {
                platform: PlatformToolConfig {
                    path: Some(ibcmd_path.to_path_buf()),
                    version: None,
                },
                ..ToolsConfig::default()
            },
            mcp: Default::default(),
            tests: TestsConfig::default(),
        }
    }

    #[test]
    fn resolve_targets_prefers_dot_project_name_for_edt_extensions() {
        let dir = tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join("exts").join("client-mcp")).expect("ext dir");
        fs::write(
            dir.path().join("exts").join("client-mcp").join(".project"),
            "<projectDescription><name>client_mcp</name></projectDescription>",
        )
        .expect("project file");
        let config = sample_config(dir.path(), dir.path(), Path::new("/tmp/ibcmd"));

        let targets = resolve_targets(&config, &ConfigureExtensionsRequest { names: vec![] })
            .expect("targets");

        assert_eq!(targets, vec!["client_mcp"]);
    }

    #[cfg(unix)]
    #[test]
    fn execute_updates_extension_properties_via_ibcmd() {
        let dir = tempdir().expect("tempdir");
        let calls = dir.path().join("ibcmd.calls.log");
        let ibcmd = dir.path().join("ibcmd");
        fs::create_dir_all(dir.path().join("exts").join("client-mcp")).expect("ext dir");
        fs::write(
            dir.path().join("exts").join("client-mcp").join(".project"),
            "<projectDescription><name>client_mcp</name></projectDescription>",
        )
        .expect("project file");
        write_script(
            &ibcmd,
            &format!("printf '%s\\n' \"$*\" >> '{}'\nexit 0", calls.display()),
        );
        let config = sample_config(dir.path(), dir.path(), &ibcmd);

        let result = execute(
            &ExecutionContext::cli(CommandName::Extensions),
            &config,
            &ConfigureExtensionsRequest { names: vec![] },
        )
        .expect("execute");

        assert!(result.ok);
        let calls_text = fs::read_to_string(calls).expect("calls");
        assert!(calls_text.contains("extension update"));
        assert!(calls_text.contains("--name client_mcp"));
        assert!(calls_text.contains("--safe-mode no"));
        assert!(calls_text.contains("--unsafe-action-protection no"));
    }

    #[cfg(unix)]
    #[test]
    fn execute_updates_extension_properties_via_ibcmd_server_contract() {
        let dir = tempdir().expect("tempdir");
        let calls = dir.path().join("ibcmd.calls.log");
        let ibcmd = dir.path().join("ibcmd");
        fs::create_dir_all(dir.path().join("exts").join("client-mcp")).expect("ext dir");
        fs::write(
            dir.path().join("exts").join("client-mcp").join(".project"),
            "<projectDescription><name>client_mcp</name></projectDescription>",
        )
        .expect("project file");
        write_script(
            &ibcmd,
            &format!("printf '%s\\n' \"$*\" >> '{}'\nexit 0", calls.display()),
        );
        let mut config = sample_config(dir.path(), dir.path(), &ibcmd);
        config.infobase = crate::config::model::InfobaseConfig::server(
            "Srvr=cluster:1541;Ref=demo",
            crate::config::model::InfobaseDbmsConfig::new(
                "PostgreSQL",
                "localhost",
                "demo",
            )
            .with_credentials(Some("postgres".to_owned()), Some("pg-secret".to_owned())),
        )
        .with_credentials(Some("Admin".to_owned()), Some("secret".to_owned()));

        let result = execute(
            &ExecutionContext::cli(CommandName::Extensions),
            &config,
            &ConfigureExtensionsRequest { names: vec![] },
        )
        .expect("execute");

        assert!(result.ok);
        let calls_text = fs::read_to_string(calls).expect("calls");
        assert!(calls_text.contains("--dbms PostgreSQL"));
        assert!(calls_text.contains("--database-server localhost"));
        assert!(calls_text.contains("--database-name demo"));
        assert!(calls_text.contains("--database-user postgres"));
        assert!(calls_text.contains("--database-password pg-secret"));
        assert!(calls_text.contains("--user Admin"));
        assert!(calls_text.contains("--password secret"));
    }

    #[cfg(unix)]
    #[test]
    fn execute_extension_non_zero_exit_reports_operation_target_and_exit_code() {
        let dir = tempdir().expect("tempdir");
        let ibcmd = dir.path().join("ibcmd");
        fs::create_dir_all(dir.path().join("exts").join("client-mcp")).expect("ext dir");
        fs::write(
            dir.path().join("exts").join("client-mcp").join(".project"),
            "<projectDescription><name>client_mcp</name></projectDescription>",
        )
        .expect("project file");
        write_script(&ibcmd, "echo 'bad extension state' >&2\nexit 17");
        let config = sample_config(dir.path(), dir.path(), &ibcmd);

        let failure = execute(
            &ExecutionContext::cli(CommandName::Extensions),
            &config,
            &ConfigureExtensionsRequest { names: vec![] },
        )
        .expect_err("failure");

        assert_eq!(failure.error.kind(), UseCaseErrorKind::Platform);
        assert!(failure
            .error
            .message()
            .contains("extension update failed for extension 'client_mcp' with exit code 17"));
        assert!(failure
            .error
            .message()
            .contains("stderr: bad extension state"));
    }
}
