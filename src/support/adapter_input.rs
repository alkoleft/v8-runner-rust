use crate::use_cases::request::{DumpModeRequest, LaunchModeRequest};
use crate::use_cases::result::{UseCaseError, UseCaseErrorKind};

/// Accepted launch-mode alias set for a specific transport boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchModeAliases {
    Cli,
    Mcp,
}

/// Normalized extension targeting shared by transport adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedExtensionScope {
    pub extension: Option<String>,
    pub all_extensions: bool,
}

/// Trims a raw optional string and drops blank values.
pub fn normalize_optional_string(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

/// Trims a required raw string and rejects blank values.
pub fn normalize_required_string(
    value: &str,
    field_name: &'static str,
) -> Result<String, UseCaseError> {
    normalize_optional_string(Some(value)).ok_or_else(|| {
        UseCaseError::new(
            UseCaseErrorKind::Validation,
            format!("{field_name} must not be blank"),
        )
    })
}

/// Parses a required CLI dump mode.
pub fn parse_required_dump_mode(raw: &str) -> Result<DumpModeRequest, UseCaseError> {
    let mode = normalize_required_string(raw, "dump mode")?;
    parse_normalized_dump_mode(&mode)
}

/// Parses an optional dump mode while preserving the caller-selected default when omitted.
pub fn parse_optional_dump_mode(
    raw: Option<&str>,
    default_mode: DumpModeRequest,
) -> Result<DumpModeRequest, UseCaseError> {
    match normalize_optional_string(raw) {
        Some(mode) => parse_normalized_dump_mode(&mode),
        None => Ok(default_mode),
    }
}

fn parse_normalized_dump_mode(mode: &str) -> Result<DumpModeRequest, UseCaseError> {
    match mode.to_ascii_uppercase().as_str() {
        "FULL" => Ok(DumpModeRequest::Full),
        "INCREMENTAL" => Ok(DumpModeRequest::Incremental),
        "PARTIAL" => Ok(DumpModeRequest::Partial),
        _ => Err(UseCaseError::new(
            UseCaseErrorKind::Validation,
            format!("unsupported dump mode: {mode}"),
        )),
    }
}

/// Parses a launch mode according to the current transport alias contract.
pub fn parse_launch_mode(
    raw: &str,
    field_name: &'static str,
    aliases: LaunchModeAliases,
) -> Result<LaunchModeRequest, UseCaseError> {
    let normalized = normalize_required_string(raw, field_name)?.to_lowercase();
    let mode = match aliases {
        LaunchModeAliases::Cli => match normalized.as_str() {
            "designer" => Some(LaunchModeRequest::Designer),
            "thin" => Some(LaunchModeRequest::Thin),
            "thick" => Some(LaunchModeRequest::Thick),
            "ordinary" => Some(LaunchModeRequest::Ordinary),
            _ => None,
        },
        LaunchModeAliases::Mcp => match normalized.as_str() {
            "designer" | "configurator" | "1cv8" | "конфигуратор" => {
                Some(LaunchModeRequest::Designer)
            }
            "thin"
            | "thin-client"
            | "thin client"
            | "thin_client"
            | "tc"
            | "1cv8c"
            | "тонкий клиент"
            | "тонкий" => Some(LaunchModeRequest::Thin),
            "thick"
            | "thick-client"
            | "thick client"
            | "thick_client"
            | "толстый клиент"
            | "толстый" => Some(LaunchModeRequest::Thick),
            _ => None,
        },
    };

    mode.ok_or_else(|| {
        UseCaseError::new(
            UseCaseErrorKind::Validation,
            format!("unsupported launch {field_name}: {raw}"),
        )
    })
}

/// Normalizes optional extension targeting for syntax-like requests.
pub fn normalize_extension_scope(
    extension: Option<&str>,
    all_extensions: Option<bool>,
) -> NormalizedExtensionScope {
    let extension = normalize_optional_string(extension);
    let all_extensions = all_extensions.unwrap_or(extension.is_none());

    NormalizedExtensionScope {
        extension,
        all_extensions,
    }
}

/// Validates MCP-specific syntax flag dependencies before use-case dispatch.
pub fn validate_extended_modules_dependencies(
    extended_modules_check: Option<bool>,
    check_use_synchronous_calls: Option<bool>,
    check_use_modality: Option<bool>,
) -> Result<(), UseCaseError> {
    if extended_modules_check == Some(false) && check_use_synchronous_calls == Some(true) {
        return Err(UseCaseError::new(
            UseCaseErrorKind::Validation,
            "checkUseSynchronousCalls requires extendedModulesCheck=true",
        ));
    }

    if extended_modules_check == Some(false) && check_use_modality == Some(true) {
        return Err(UseCaseError::new(
            UseCaseErrorKind::Validation,
            "checkUseModality requires extendedModulesCheck=true",
        ));
    }

    Ok(())
}

/// Normalizes a single optional EDT project name into the use-case request list.
pub fn normalize_edt_projects(project_name: Option<&str>) -> Vec<String> {
    normalize_optional_string(project_name).map_or_else(Vec::new, |project| vec![project])
}

#[cfg(test)]
mod tests {
    use super::{
        normalize_edt_projects, normalize_extension_scope, parse_launch_mode,
        parse_optional_dump_mode, parse_required_dump_mode, LaunchModeAliases,
        NormalizedExtensionScope,
    };
    use crate::use_cases::request::{DumpModeRequest, LaunchModeRequest};
    use crate::use_cases::result::UseCaseErrorKind;

    #[test]
    fn parses_dump_modes_for_cli_and_mcp_defaults() {
        assert_eq!(
            parse_required_dump_mode("incremental").expect("cli dump mode"),
            DumpModeRequest::Incremental
        );
        assert_eq!(
            parse_optional_dump_mode(None, DumpModeRequest::Incremental).expect("default mode"),
            DumpModeRequest::Incremental
        );
        assert_eq!(
            parse_optional_dump_mode(Some(" PARTIAL "), DumpModeRequest::Incremental)
                .expect("partial mode"),
            DumpModeRequest::Partial
        );
    }

    #[test]
    fn parses_launch_modes_for_cli_and_mcp_alias_sets() {
        assert_eq!(
            parse_launch_mode("ordinary", "mode", LaunchModeAliases::Cli).expect("cli ordinary"),
            LaunchModeRequest::Ordinary
        );
        assert_eq!(
            parse_launch_mode("Тонкий клиент", "utility_type", LaunchModeAliases::Mcp)
                .expect("mcp alias"),
            LaunchModeRequest::Thin
        );
        let error = parse_launch_mode("ordinary", "utility_type", LaunchModeAliases::Mcp)
            .expect_err("ordinary is not published for MCP");
        assert_eq!(error.kind(), UseCaseErrorKind::Validation);
        assert_eq!(error.message(), "unsupported launch utility_type: ordinary");
    }

    #[test]
    fn normalizes_extension_scope_and_edt_project_name() {
        assert_eq!(
            normalize_extension_scope(Some(" Ext "), None),
            NormalizedExtensionScope {
                extension: Some("Ext".to_owned()),
                all_extensions: false,
            }
        );
        assert_eq!(
            normalize_extension_scope(None, Some(false)),
            NormalizedExtensionScope {
                extension: None,
                all_extensions: false,
            }
        );
        assert_eq!(normalize_edt_projects(Some(" Project ")), vec!["Project"]);
        assert!(normalize_edt_projects(Some("   ")).is_empty());
    }
}
