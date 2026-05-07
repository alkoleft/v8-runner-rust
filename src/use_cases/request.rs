use crate::domain::artifacts::{CFE_RUNNER_ID, CF_RUNNER_ID, EPF_RUNNER_ID, ERF_RUNNER_ID};
use crate::domain::execution::ExecutionTimeouts;
use crate::domain::load::LoadMode;
use crate::domain::runner::{
    ExecutionPolicy, LaunchClientModeRequest, LaunchOptions, RunnerKind, RunnerOutputFormat,
    RunnerProfile, ScenarioExecutionRequest,
};
use crate::domain::test::TEST_RUNNER_ID;
use crate::use_cases::result::{UseCaseError, UseCaseErrorKind};

/// Transport-neutral request for the `build` use case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildRequest {
    /// Forces a full rebuild instead of change-based execution.
    pub full_rebuild: bool,
    /// Optional source-set selector. When absent, all configured source-sets are built.
    pub source_set: Option<String>,
}

/// Transport-neutral request for the `load` use case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadRequest {
    /// Requested mode for artifact application.
    pub mode: LoadMode,
    /// Path to artifact file.
    pub artifact_path: String,
    /// Optional merge settings file.
    pub settings_path: Option<String>,
    /// Optional extension target.
    pub extension: Option<String>,
}

/// Transport-neutral request for the `test` use case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestRequest {
    /// Shared runner execution block reused by future test/package scenarios.
    pub execution: ScenarioExecutionRequest,
    /// When `true`, the use case may request a full build before test execution.
    pub full: bool,
    /// Selected test scope. Module targets require a non-empty module name.
    pub scope: TestScopeRequest,
    /// Optional WS-mode (session-manager) overrides shared with `launch mcp`.
    pub mcp_ws: McpClientWsRequest,
}

impl TestRequest {
    /// Default YaXUnit execution contract for the current test flow.
    /// Note: only `timeouts.total_ms` is wired into runtime today; the policy
    /// flags are retained as part of the shared OCP-friendly contract.
    pub fn default_execution() -> ScenarioExecutionRequest {
        ScenarioExecutionRequest {
            profile: RunnerProfile {
                id: TEST_RUNNER_ID.to_owned(),
                kind: RunnerKind::YaXUnit,
                output_formats: vec![
                    RunnerOutputFormat::JunitXml,
                    RunnerOutputFormat::PlainTextLog,
                ],
                backend_hint: Some("enterprise".to_owned()),
            },
            client_mode: Some(LaunchClientModeRequest::Thin),
            timeouts: ExecutionTimeouts::default(),
            policy: ExecutionPolicy {
                retain_artifacts_on_failure: true,
                retain_artifacts_on_success: false,
            },
            launch: LaunchOptions::default(),
        }
    }
}

/// Transport-neutral test scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestScopeRequest {
    All,
    /// Runs a single module test target.
    Module {
        name: String,
    },
}

/// Transport-neutral dump mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DumpModeRequest {
    Full,
    Incremental,
    Partial,
}

/// Transport-neutral request for the `dump` use case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DumpRequest {
    /// Requested dump mode. `Partial` requires at least one object selector.
    pub mode: DumpModeRequest,
    /// Optional source-set selector. Required when multiple candidates are available.
    pub source_set: Option<String>,
    /// Optional extension selector for extension dumps.
    pub extension: Option<String>,
    /// Requested object filters for `Partial` dump mode.
    pub objects: Vec<String>,
}

/// Transport-neutral convert scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConvertScopeRequest {
    All,
    SourceSet { name: String },
}

/// Transport-neutral request for the `convert` use case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConvertRequest {
    /// Requested convert scope.
    pub scope: ConvertScopeRequest,
    /// Optional user-facing target root for converted source-set layout.
    pub output_root: Option<String>,
}

/// Transport-neutral artifact export mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactsModeRequest {
    ConfigurationCf,
    ExtensionCfe,
    ExternalDataProcessorEpf,
    ExternalReportErf,
}

/// Transport-neutral request for the `artifacts` use case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactsRequest {
    /// Shared runner execution block for packaging-like scenarios.
    pub execution: ScenarioExecutionRequest,
    /// Requested artifact export mode.
    pub mode: ArtifactsModeRequest,
    /// Final output file path provided by the caller.
    pub output_path: String,
    /// Optional source-set selector used to disambiguate repo context.
    pub source_set: Option<String>,
    /// Requested extension name in the infobase for `-Extension`.
    pub extension: Option<String>,
}

impl ArtifactsRequest {
    pub fn default_execution(mode: ArtifactsModeRequest) -> ScenarioExecutionRequest {
        let (id, kind) = match mode {
            ArtifactsModeRequest::ConfigurationCf => (CF_RUNNER_ID, RunnerKind::Cf),
            ArtifactsModeRequest::ExtensionCfe => (CFE_RUNNER_ID, RunnerKind::Cfe),
            ArtifactsModeRequest::ExternalDataProcessorEpf => (EPF_RUNNER_ID, RunnerKind::Epf),
            ArtifactsModeRequest::ExternalReportErf => (ERF_RUNNER_ID, RunnerKind::Erf),
        };

        ScenarioExecutionRequest {
            profile: RunnerProfile {
                id: id.to_owned(),
                kind,
                output_formats: vec![RunnerOutputFormat::Binary, RunnerOutputFormat::PlainTextLog],
                backend_hint: Some("designer".to_owned()),
            },
            client_mode: Some(LaunchClientModeRequest::Designer),
            timeouts: ExecutionTimeouts::default(),
            policy: ExecutionPolicy {
                retain_artifacts_on_failure: true,
                retain_artifacts_on_success: true,
            },
            launch: LaunchOptions::default(),
        }
    }
}

/// Transport-neutral request for the `syntax` use case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxRequest {
    /// Selected syntax target and validation flags.
    pub target: SyntaxTargetRequest,
}

/// Transport-neutral syntax target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyntaxTargetRequest {
    DesignerConfig(DesignerConfigSyntaxRequest),
    DesignerModules(DesignerModulesSyntaxRequest),
    /// Runs EDT validation for selected projects or all EDT projects when empty.
    Edt {
        projects: Vec<String>,
    },
}

/// Supported Designer client-mode scopes for syntax checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesignerClientScope {
    ThinClient,
    WebClient,
    MobileClient,
    Server,
    ExternalConnection,
    ExternalConnectionServer,
    MobileAppClient,
    MobileAppServer,
    ThickClientManagedApplication,
    ThickClientServerManagedApplication,
    ThickClientOrdinaryApplication,
    ThickClientServerOrdinaryApplication,
}

impl DesignerClientScope {
    pub const fn flag(self) -> &'static str {
        match self {
            Self::ThinClient => "-ThinClient",
            Self::WebClient => "-WebClient",
            Self::MobileClient => "-MobileClient",
            Self::Server => "-Server",
            Self::ExternalConnection => "-ExternalConnection",
            Self::ExternalConnectionServer => "-ExternalConnectionServer",
            Self::MobileAppClient => "-MobileAppClient",
            Self::MobileAppServer => "-MobileAppServer",
            Self::ThickClientManagedApplication => "-ThickClientManagedApplication",
            Self::ThickClientServerManagedApplication => "-ThickClientServerManagedApplication",
            Self::ThickClientOrdinaryApplication => "-ThickClientOrdinaryApplication",
            Self::ThickClientServerOrdinaryApplication => "-ThickClientServerOrdinaryApplication",
        }
    }
}

/// Deduplicated client-scope set emitted in stable flag order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DesignerClientScopes {
    scopes: Vec<DesignerClientScope>,
}

impl DesignerClientScopes {
    pub fn new(scopes: impl IntoIterator<Item = DesignerClientScope>) -> Self {
        let mut unique = Vec::new();
        for scope in scopes {
            if !unique.contains(&scope) {
                unique.push(scope);
            }
        }
        Self { scopes: unique }
    }

    pub fn contains(&self, scope: DesignerClientScope) -> bool {
        self.scopes.contains(&scope)
    }

    pub fn is_empty(&self) -> bool {
        self.scopes.is_empty()
    }
}

/// Supported non-client Designer configuration checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesignerConfigCheck {
    ConfigLogIntegrity,
    IncorrectReferences,
    MobileClientDigiSign,
    DistributiveModules,
    UnreferenceProcedures,
    HandlersExistence,
    EmptyHandlers,
    UnsupportedFunctional,
}

impl DesignerConfigCheck {
    pub const fn flag(self) -> &'static str {
        match self {
            Self::ConfigLogIntegrity => "-ConfigLogIntegrity",
            Self::IncorrectReferences => "-IncorrectReferences",
            Self::MobileClientDigiSign => "-MobileClientDigiSign",
            Self::DistributiveModules => "-DistributiveModules",
            Self::UnreferenceProcedures => "-UnreferenceProcedures",
            Self::HandlersExistence => "-HandlersExistence",
            Self::EmptyHandlers => "-EmptyHandlers",
            Self::UnsupportedFunctional => "-UnsupportedFunctional",
        }
    }
}

/// Deduplicated config-check set emitted in stable flag order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DesignerConfigChecks {
    checks: Vec<DesignerConfigCheck>,
}

impl DesignerConfigChecks {
    pub fn new(checks: impl IntoIterator<Item = DesignerConfigCheck>) -> Self {
        let mut unique = Vec::new();
        for check in checks {
            if !unique.contains(&check) {
                unique.push(check);
            }
        }
        Self { checks: unique }
    }

    pub fn contains(&self, check: DesignerConfigCheck) -> bool {
        self.checks.contains(&check)
    }
}

/// Extension-targeting strategy for Designer syntax commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyntaxExtensionScope {
    MainConfiguration,
    AllExtensions,
    SingleExtension { name: String },
    SingleExtensionAndAll { name: String },
}

impl SyntaxExtensionScope {
    pub fn new(extension: Option<String>, all_extensions: bool) -> Self {
        match (extension, all_extensions) {
            (Some(name), true) => Self::SingleExtensionAndAll { name },
            (Some(name), false) => Self::SingleExtension { name },
            (None, true) => Self::AllExtensions,
            (None, false) => Self::MainConfiguration,
        }
    }

    pub fn extension(&self) -> Option<&str> {
        match self {
            Self::SingleExtension { name } | Self::SingleExtensionAndAll { name } => {
                Some(name.as_str())
            }
            Self::MainConfiguration | Self::AllExtensions => None,
        }
    }

    pub const fn includes_all_extensions(&self) -> bool {
        matches!(
            self,
            Self::AllExtensions | Self::SingleExtensionAndAll { .. }
        )
    }
}

/// Fine-grained extra checks that can be enabled for extended modules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtendedModulesDetail {
    Basic,
    SynchronousCalls,
    Modality,
    SynchronousCallsAndModality,
}

/// Typed policy for extended-modules validation instead of separate bool flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtendedModulesPolicy {
    Disabled,
    Enabled(ExtendedModulesDetail),
}

impl ExtendedModulesPolicy {
    pub fn from_cli_flags(
        enabled: bool,
        check_use_synchronous_calls: bool,
        check_use_modality: bool,
    ) -> Result<Self, UseCaseError> {
        Self::from_flags(
            enabled,
            check_use_synchronous_calls,
            check_use_modality,
            "check-use-synchronous-calls",
            "check-use-modality",
            "extended-modules-check",
        )
    }

    pub fn from_mcp_flags(
        enabled: Option<bool>,
        check_use_synchronous_calls: Option<bool>,
        check_use_modality: Option<bool>,
    ) -> Result<Self, UseCaseError> {
        Self::from_flags(
            enabled != Some(false),
            check_use_synchronous_calls == Some(true),
            check_use_modality == Some(true),
            "checkUseSynchronousCalls",
            "checkUseModality",
            "extendedModulesCheck",
        )
    }

    pub const fn basic(enabled: bool) -> Self {
        if enabled {
            Self::Enabled(ExtendedModulesDetail::Basic)
        } else {
            Self::Disabled
        }
    }

    fn from_flags(
        enabled: bool,
        check_use_synchronous_calls: bool,
        check_use_modality: bool,
        sync_calls_field: &'static str,
        modality_field: &'static str,
        enabled_field: &'static str,
    ) -> Result<Self, UseCaseError> {
        if !enabled && check_use_synchronous_calls {
            return Err(UseCaseError::new(
                UseCaseErrorKind::Validation,
                format!("{sync_calls_field} requires {enabled_field}=true"),
            ));
        }

        if !enabled && check_use_modality {
            return Err(UseCaseError::new(
                UseCaseErrorKind::Validation,
                format!("{modality_field} requires {enabled_field}=true"),
            ));
        }

        Ok(
            match (enabled, check_use_synchronous_calls, check_use_modality) {
                (false, false, false) => Self::Disabled,
                (true, false, false) => Self::Enabled(ExtendedModulesDetail::Basic),
                (true, true, false) => Self::Enabled(ExtendedModulesDetail::SynchronousCalls),
                (true, false, true) => Self::Enabled(ExtendedModulesDetail::Modality),
                (true, true, true) => {
                    Self::Enabled(ExtendedModulesDetail::SynchronousCallsAndModality)
                }
                (false, _, _) => unreachable!("dependency checks reject disabled extra checks"),
            },
        )
    }

    pub const fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled(_))
    }

    pub const fn checks_synchronous_calls(self) -> bool {
        matches!(
            self,
            Self::Enabled(ExtendedModulesDetail::SynchronousCalls)
                | Self::Enabled(ExtendedModulesDetail::SynchronousCallsAndModality)
        )
    }

    pub const fn checks_modality(self) -> bool {
        matches!(
            self,
            Self::Enabled(ExtendedModulesDetail::Modality)
                | Self::Enabled(ExtendedModulesDetail::SynchronousCallsAndModality)
        )
    }
}

/// Transport-neutral request for Designer configuration checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesignerConfigSyntaxRequest {
    /// Non-client config checks selected for this request.
    checks: DesignerConfigChecks,
    /// Client scopes selected for Designer syntax analysis.
    client_scopes: DesignerClientScopes,
    /// Typed extended-modules policy, including dependent extra checks.
    extended_modules: ExtendedModulesPolicy,
    /// Extension-targeting strategy for the syntax run.
    extension_scope: SyntaxExtensionScope,
}

impl DesignerConfigSyntaxRequest {
    pub fn new(
        checks: DesignerConfigChecks,
        client_scopes: DesignerClientScopes,
        extended_modules: ExtendedModulesPolicy,
        extension_scope: SyntaxExtensionScope,
    ) -> Self {
        Self {
            checks,
            client_scopes,
            extended_modules,
            extension_scope,
        }
    }

    pub fn has_check(&self, check: DesignerConfigCheck) -> bool {
        self.checks.contains(check)
    }

    pub fn has_client_scope(&self, scope: DesignerClientScope) -> bool {
        self.client_scopes.contains(scope)
    }

    pub const fn extended_modules(&self) -> ExtendedModulesPolicy {
        self.extended_modules
    }

    pub const fn extension_scope(&self) -> &SyntaxExtensionScope {
        &self.extension_scope
    }
}

/// Transport-neutral request for Designer module checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesignerModulesSyntaxRequest {
    client_scopes: DesignerClientScopes,
    extended_modules: ExtendedModulesPolicy,
    extension_scope: SyntaxExtensionScope,
}

impl DesignerModulesSyntaxRequest {
    pub fn new(
        client_scopes: DesignerClientScopes,
        extended_modules: ExtendedModulesPolicy,
        extension_scope: SyntaxExtensionScope,
    ) -> Result<Self, UseCaseError> {
        if client_scopes.is_empty() && !extended_modules.is_enabled() {
            return Err(UseCaseError::new(
                UseCaseErrorKind::Validation,
                "syntax designer-modules requires at least one mode flag",
            ));
        }

        Ok(Self {
            client_scopes,
            extended_modules,
            extension_scope,
        })
    }

    pub fn has_client_scope(&self, scope: DesignerClientScope) -> bool {
        self.client_scopes.contains(scope)
    }

    pub fn has_modes(&self) -> bool {
        !self.client_scopes.is_empty() || self.extended_modules.is_enabled()
    }

    pub const fn extended_modules(&self) -> ExtendedModulesPolicy {
        self.extended_modules
    }

    pub const fn extension_scope(&self) -> &SyntaxExtensionScope {
        &self.extension_scope
    }
}

/// Enterprise-mode launch targets grouped under the shared enterprise launcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnterpriseLaunchTarget {
    ThinClient,
    ThickClient,
    OrdinaryApplication,
    ClientMcp { mode: ClientMcpMode },
}

/// Client mode used by the 1C client-side MCP launcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientMcpMode {
    Thin,
    Thick,
    Ordinary,
}

/// Transport-neutral launch target grouped by launcher family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchTargetRequest {
    Designer,
    Enterprise(EnterpriseLaunchTarget),
}

impl LaunchTargetRequest {
    pub const fn designer() -> Self {
        Self::Designer
    }

    pub const fn thin_client() -> Self {
        Self::Enterprise(EnterpriseLaunchTarget::ThinClient)
    }

    pub const fn thick_client() -> Self {
        Self::Enterprise(EnterpriseLaunchTarget::ThickClient)
    }

    pub const fn ordinary_application() -> Self {
        Self::Enterprise(EnterpriseLaunchTarget::OrdinaryApplication)
    }

    pub const fn client_mcp() -> Self {
        Self::Enterprise(EnterpriseLaunchTarget::ClientMcp {
            mode: ClientMcpMode::Thin,
        })
    }

    pub const fn client_mcp_with_mode(mode: ClientMcpMode) -> Self {
        Self::Enterprise(EnterpriseLaunchTarget::ClientMcp { mode })
    }
}

/// Optional scenario launched alongside the client-side MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientMcpAddonRequest {
    VanessaAutomation,
}

/// Transport-neutral options for `launch mcp`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClientMcpOptionsRequest {
    pub config_path: Option<String>,
    pub port: Option<u16>,
    pub addon: Option<ClientMcpAddonRequest>,
}

/// Transport-neutral request for the `launch` use case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchRequest {
    /// Requested launch target.
    pub target: LaunchTargetRequest,
    /// Shared launch options mapped from CLI/test scenarios.
    pub launch: LaunchOptions,
    /// Client-side MCP launch options. Present only for `LaunchTargetRequest::client_mcp*`.
    pub client_mcp: Option<ClientMcpOptionsRequest>,
    /// Optional WS-mode (session-manager) overrides shared between
    /// `launch mcp` and `test` flows.
    pub mcp_ws: McpClientWsRequest,
}

/// Transport-neutral selector for MCP client transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpClientTransportRequest {
    Ws,
    Legacy,
    Auto,
}

/// Transport-neutral overrides for the WS-mode `/C` snippet. All fields are
/// optional; the use case fills missing values from project config and
/// internal defaults at execution time.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpClientWsRequest {
    pub transport: Option<McpClientTransportRequest>,
    pub manager_url: Option<String>,
    pub client_uid: Option<String>,
    pub corr_id: Option<String>,
    pub log_level: Option<String>,
    pub ws_timeout_ms: Option<u64>,
}

/// Transport-neutral request for the `init` use case.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InitRequest;

/// Transport-neutral request for extension property updates.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConfigureExtensionsRequest {
    /// Optional source-set names to update. Empty means all extension source-sets.
    pub names: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::{
        DesignerClientScope, DesignerClientScopes, DesignerConfigCheck, DesignerConfigChecks,
        DesignerConfigSyntaxRequest, DesignerModulesSyntaxRequest, ExtendedModulesDetail,
        ExtendedModulesPolicy, SyntaxExtensionScope,
    };
    use crate::use_cases::result::UseCaseErrorKind;

    #[test]
    fn config_request_uses_typed_policy_objects() {
        let request = DesignerConfigSyntaxRequest::new(
            DesignerConfigChecks::new([
                DesignerConfigCheck::ConfigLogIntegrity,
                DesignerConfigCheck::ConfigLogIntegrity,
                DesignerConfigCheck::UnsupportedFunctional,
            ]),
            DesignerClientScopes::new([
                DesignerClientScope::ThinClient,
                DesignerClientScope::ThinClient,
                DesignerClientScope::Server,
            ]),
            ExtendedModulesPolicy::from_cli_flags(true, true, false).expect("policy"),
            SyntaxExtensionScope::new(Some("Ext".to_owned()), true),
        );

        assert!(request.has_check(DesignerConfigCheck::ConfigLogIntegrity));
        assert!(request.has_check(DesignerConfigCheck::UnsupportedFunctional));
        assert!(request.has_client_scope(DesignerClientScope::ThinClient));
        assert!(request.has_client_scope(DesignerClientScope::Server));
        assert_eq!(
            request.extended_modules(),
            ExtendedModulesPolicy::Enabled(ExtendedModulesDetail::SynchronousCalls)
        );
        assert_eq!(request.extension_scope().extension(), Some("Ext"));
        assert!(request.extension_scope().includes_all_extensions());
    }

    #[test]
    fn extended_modules_policy_rejects_invalid_dependency_combinations() {
        let error =
            ExtendedModulesPolicy::from_mcp_flags(Some(false), Some(true), None).expect_err("err");

        assert_eq!(error.kind(), UseCaseErrorKind::Validation);
        assert_eq!(
            error.message(),
            "checkUseSynchronousCalls requires extendedModulesCheck=true"
        );
    }

    #[test]
    fn modules_request_requires_at_least_one_mode_or_extended_modules() {
        let error = DesignerModulesSyntaxRequest::new(
            DesignerClientScopes::default(),
            ExtendedModulesPolicy::basic(false),
            SyntaxExtensionScope::new(None, true),
        )
        .expect_err("err");

        assert_eq!(error.kind(), UseCaseErrorKind::Validation);
        assert_eq!(
            error.message(),
            "syntax designer-modules requires at least one mode flag"
        );
    }
}
