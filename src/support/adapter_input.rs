use crate::use_cases::request::{DumpModeRequest, LaunchTargetRequest, SyntaxExtensionScope};
use crate::use_cases::result::{UseCaseError, UseCaseErrorKind};

/// Accepted launch-mode alias set for a specific transport boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchModeAliases {
    Cli,
    Mcp,
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
pub fn parse_launch_target(
    raw: &str,
    field_name: &'static str,
    aliases: LaunchModeAliases,
) -> Result<LaunchTargetRequest, UseCaseError> {
    let normalized = normalize_required_string(raw, field_name)?.to_lowercase();
    let mode = match aliases {
        LaunchModeAliases::Cli => match normalized.as_str() {
            "designer" => Some(LaunchTargetRequest::designer()),
            "thin" => Some(LaunchTargetRequest::thin_client()),
            "thick" => Some(LaunchTargetRequest::thick_client()),
            "ordinary" => Some(LaunchTargetRequest::ordinary_application()),
            _ => None,
        },
        LaunchModeAliases::Mcp => match normalized.as_str() {
            "designer" | "configurator" | "1cv8" | "конфигуратор" => {
                Some(LaunchTargetRequest::designer())
            }
            "thin"
            | "thin-client"
            | "thin client"
            | "thin_client"
            | "tc"
            | "1cv8c"
            | "тонкий клиент"
            | "тонкий" => Some(LaunchTargetRequest::thin_client()),
            "thick"
            | "thick-client"
            | "thick client"
            | "thick_client"
            | "толстый клиент"
            | "толстый" => Some(LaunchTargetRequest::thick_client()),
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
) -> SyntaxExtensionScope {
    let extension = normalize_optional_string(extension);
    let all_extensions = all_extensions.unwrap_or(extension.is_none());
    SyntaxExtensionScope::new(extension, all_extensions)
}

/// Normalizes a single optional EDT project name into the use-case request list.
pub fn normalize_edt_projects(project_name: Option<&str>) -> Vec<String> {
    normalize_optional_string(project_name).map_or_else(Vec::new, |project| vec![project])
}

#[cfg(test)]
mod tests {
    use super::{
        normalize_edt_projects, normalize_extension_scope, parse_launch_target,
        parse_optional_dump_mode, parse_required_dump_mode, LaunchModeAliases,
    };
    use crate::use_cases::request::{DumpModeRequest, LaunchTargetRequest, SyntaxExtensionScope};
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
            parse_launch_target("ordinary", "mode", LaunchModeAliases::Cli).expect("cli ordinary"),
            LaunchTargetRequest::ordinary_application()
        );
        assert_eq!(
            parse_launch_target("Тонкий клиент", "utility_type", LaunchModeAliases::Mcp)
                .expect("mcp alias"),
            LaunchTargetRequest::thin_client()
        );
        let error = parse_launch_target("ordinary", "utility_type", LaunchModeAliases::Mcp)
            .expect_err("ordinary is not published for MCP");
        assert_eq!(error.kind(), UseCaseErrorKind::Validation);
        assert_eq!(error.message(), "unsupported launch utility_type: ordinary");
    }

    #[test]
    fn normalizes_extension_scope_and_edt_project_name() {
        assert_eq!(
            normalize_extension_scope(Some(" Ext "), None),
            SyntaxExtensionScope::SingleExtension {
                name: "Ext".to_owned(),
            }
        );
        assert_eq!(
            normalize_extension_scope(None, Some(false)),
            SyntaxExtensionScope::MainConfiguration
        );
        assert_eq!(normalize_edt_projects(Some(" Project ")), vec!["Project"]);
        assert!(normalize_edt_projects(Some("   ")).is_empty());
    }
}
