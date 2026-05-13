use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// MCP request for `build_project`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct McpBuildProjectRequest {
    /// Optional full-rebuild flag from the MCP tool surface.
    #[schemars(description = "Run a full rebuild instead of an incremental build.")]
    pub full_rebuild: Option<bool>,
    /// Optional source-set selector from v8project.yaml.
    #[schemars(description = "Source-set name to build. When omitted, all source-sets are built.")]
    pub source_set: Option<String>,
    /// Optional one-shot override for `/UpdateDBCfg -Dynamic+`.
    #[schemars(
        description = "Override build.dynamicUpdate for this call: true applies changes without exclusive lock."
    )]
    pub dynamic_update: Option<bool>,
}

/// MCP request for `run_all_tests`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct McpRunAllTestsRequest {
    /// Optional full-report flag from the MCP tool surface.
    #[schemars(description = "Return the full test report instead of compact summary data.")]
    pub full: Option<bool>,
}

/// MCP request for `run_module_tests`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct McpRunModuleTestsRequest {
    /// Module name to execute.
    #[schemars(description = "Module name to execute tests for.")]
    pub module_name: String,
    /// Optional full-report flag from the MCP tool surface.
    #[serde(default)]
    #[schemars(description = "Return the full test report instead of compact summary data.")]
    pub full: Option<bool>,
}

/// MCP request for `dump_config`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct McpDumpConfigRequest {
    /// Optional raw dump mode. Null/blank defaults to `INCREMENTAL` in service mappers.
    #[schemars(description = "Dump mode, for example FULL or INCREMENTAL.")]
    pub mode: Option<String>,
    /// Optional extension name.
    #[schemars(
        description = "Extension name to dump. When omitted, the main configuration is dumped."
    )]
    pub extension: Option<String>,
    /// Requested object list for partial dump.
    #[serde(default)]
    #[schemars(description = "Specific metadata objects to dump.")]
    pub objects: Vec<String>,
}

/// MCP request for `launch_app`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct McpLaunchAppRequest {
    /// Raw utility alias from the MCP tool surface.
    #[schemars(
        description = "Client type to launch. Supported aliases: thin-client, тонкий клиент, тонкий, thin client, thin, tc; thick-client, толстый клиент, толстый, thick client, thick; designer, конфигуратор, configurator."
    )]
    pub utility_type: String,
}

/// MCP request for `check_syntax_edt`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct McpCheckSyntaxEdtRequest {
    /// Optional project name; when absent, all EDT projects are checked.
    #[schemars(
        description = "EDT project name to check. When omitted, all EDT projects are checked."
    )]
    pub project_name: Option<String>,
}

/// MCP request for `check_syntax_designer_config`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct McpCheckSyntaxDesignerConfigRequest {
    #[schemars(description = "Enable Designer configuration log integrity checks.")]
    pub config_log_integrity: Option<bool>,
    #[schemars(description = "Enable incorrect references checks.")]
    pub incorrect_references: Option<bool>,
    #[schemars(description = "Run checks for thin client context.")]
    pub thin_client: Option<bool>,
    #[schemars(description = "Run checks for web client context.")]
    pub web_client: Option<bool>,
    #[schemars(description = "Run checks for mobile client context.")]
    pub mobile_client: Option<bool>,
    #[schemars(description = "Run checks for server context.")]
    pub server: Option<bool>,
    #[schemars(description = "Run checks for external connection context.")]
    pub external_connection: Option<bool>,
    #[schemars(description = "Run checks for external connection server context.")]
    pub external_connection_server: Option<bool>,
    #[schemars(description = "Run checks for mobile application client context.")]
    pub mobile_app_client: Option<bool>,
    #[schemars(description = "Run checks for mobile application server context.")]
    pub mobile_app_server: Option<bool>,
    #[schemars(description = "Run checks for thick client managed application context.")]
    pub thick_client_managed_application: Option<bool>,
    #[schemars(description = "Run checks for thick client server managed application context.")]
    pub thick_client_server_managed_application: Option<bool>,
    #[schemars(description = "Run checks for thick client ordinary application context.")]
    pub thick_client_ordinary_application: Option<bool>,
    #[schemars(description = "Run checks for thick client server ordinary application context.")]
    pub thick_client_server_ordinary_application: Option<bool>,
    #[schemars(description = "Enable mobile client digital signature checks.")]
    pub mobile_client_digi_sign: Option<bool>,
    #[schemars(description = "Enable distributive modules checks.")]
    pub distributive_modules: Option<bool>,
    #[schemars(description = "Enable unreferenced procedures checks.")]
    pub unreference_procedures: Option<bool>,
    #[schemars(description = "Enable event handler existence checks.")]
    pub handlers_existence: Option<bool>,
    #[schemars(description = "Enable empty handlers checks.")]
    pub empty_handlers: Option<bool>,
    #[schemars(description = "Enable extended modules checks.")]
    pub extended_modules_check: Option<bool>,
    #[schemars(description = "Enable synchronous calls usage checks.")]
    pub check_use_synchronous_calls: Option<bool>,
    #[schemars(description = "Enable modality usage checks.")]
    pub check_use_modality: Option<bool>,
    #[schemars(description = "Enable unsupported functionality checks.")]
    pub unsupported_functional: Option<bool>,
    /// Optional extension selector. Blank values are treated as absent.
    #[schemars(description = "Extension name to check. Blank values are treated as absent.")]
    pub extension: Option<String>,
    /// Optional tri-state all-extensions flag. When omitted, service mappers infer the default
    /// from `extension`.
    #[schemars(description = "Check all extensions instead of a specific extension.")]
    pub all_extensions: Option<bool>,
}

/// MCP request for `check_syntax_designer_modules`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct McpCheckSyntaxDesignerModulesRequest {
    #[schemars(description = "Run checks for thin client context.")]
    pub thin_client: Option<bool>,
    #[schemars(description = "Run checks for web client context.")]
    pub web_client: Option<bool>,
    #[schemars(description = "Run checks for server context.")]
    pub server: Option<bool>,
    #[schemars(description = "Run checks for external connection context.")]
    pub external_connection: Option<bool>,
    #[schemars(description = "Run checks for thick client ordinary application context.")]
    pub thick_client_ordinary_application: Option<bool>,
    #[schemars(description = "Run checks for mobile application client context.")]
    pub mobile_app_client: Option<bool>,
    #[schemars(description = "Run checks for mobile application server context.")]
    pub mobile_app_server: Option<bool>,
    #[schemars(description = "Run checks for mobile client context.")]
    pub mobile_client: Option<bool>,
    #[schemars(description = "Enable extended modules checks.")]
    pub extended_modules_check: Option<bool>,
    /// Optional extension selector. Blank values are treated as absent.
    #[schemars(description = "Extension name to check. Blank values are treated as absent.")]
    pub extension: Option<String>,
    /// Optional tri-state all-extensions flag. When omitted, service mappers infer the default
    /// from `extension`.
    #[schemars(description = "Check all extensions instead of a specific extension.")]
    pub all_extensions: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::{
        McpCheckSyntaxDesignerConfigRequest, McpLaunchAppRequest, McpRunModuleTestsRequest,
    };

    #[test]
    fn generated_schema_includes_parameter_descriptions() {
        let run_module = schemars::schema_for!(McpRunModuleTestsRequest);
        let run_module_json = serde_json::to_value(run_module).expect("schema json");
        assert_eq!(
            run_module_json["properties"]["moduleName"]["description"],
            "Module name to execute tests for."
        );

        let launch = schemars::schema_for!(McpLaunchAppRequest);
        let launch_json = serde_json::to_value(launch).expect("schema json");
        assert_eq!(
            launch_json["properties"]["utilityType"]["description"],
            "Client type to launch. Supported aliases: thin-client, тонкий клиент, тонкий, thin client, thin, tc; thick-client, толстый клиент, толстый, thick client, thick; designer, конфигуратор, configurator."
        );

        let syntax = schemars::schema_for!(McpCheckSyntaxDesignerConfigRequest);
        let syntax_json = serde_json::to_value(syntax).expect("schema json");
        assert_eq!(
            syntax_json["properties"]["allExtensions"]["description"],
            "Check all extensions instead of a specific extension."
        );
    }
}
