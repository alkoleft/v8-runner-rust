use crate::domain::artifacts::{CFE_RUNNER_ID, CF_RUNNER_ID, EPF_RUNNER_ID, ERF_RUNNER_ID};
use crate::domain::execution::ExecutionTimeouts;
use crate::domain::load::LoadMode;
use crate::domain::runner::{
    ExecutionPolicy, LaunchClientModeRequest, LaunchOptions, RunnerKind, RunnerOutputFormat,
    RunnerProfile,
    ScenarioExecutionRequest,
};
use crate::domain::test::TEST_RUNNER_ID;

/// Transport-neutral request for the `build` use case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildRequest {
    /// Forces a full rebuild instead of change-based execution.
    pub full_rebuild: bool,
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

/// Transport-neutral request for Designer configuration checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesignerConfigSyntaxRequest {
    /// Enables Designer config-log integrity checks.
    pub config_log_integrity: bool,
    pub incorrect_references: bool,
    pub thin_client: bool,
    pub web_client: bool,
    pub mobile_client: bool,
    pub server: bool,
    pub external_connection: bool,
    pub external_connection_server: bool,
    pub mobile_app_client: bool,
    pub mobile_app_server: bool,
    pub thick_client_managed_application: bool,
    pub thick_client_server_managed_application: bool,
    pub thick_client_ordinary_application: bool,
    pub thick_client_server_ordinary_application: bool,
    pub mobile_client_digi_sign: bool,
    pub distributive_modules: bool,
    pub unreference_procedures: bool,
    pub handlers_existence: bool,
    pub empty_handlers: bool,
    pub extended_modules_check: bool,
    pub check_use_synchronous_calls: bool,
    pub check_use_modality: bool,
    pub unsupported_functional: bool,
    pub extension: Option<String>,
    pub all_extensions: bool,
}

/// Transport-neutral request for Designer module checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesignerModulesSyntaxRequest {
    pub thin_client: bool,
    pub web_client: bool,
    pub server: bool,
    pub external_connection: bool,
    pub thick_client_ordinary_application: bool,
    pub mobile_app_client: bool,
    pub mobile_app_server: bool,
    pub mobile_client: bool,
    pub extended_modules_check: bool,
    pub extension: Option<String>,
    pub all_extensions: bool,
}

/// Transport-neutral launch mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchModeRequest {
    Designer,
    Thin,
    Thick,
    Ordinary,
}

/// Transport-neutral request for the `launch` use case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchRequest {
    /// Requested launch target.
    pub mode: LaunchModeRequest,
    /// Shared launch options mapped from CLI/test scenarios.
    pub launch: LaunchOptions,
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
