use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::domain::execution::ExecutionTimeouts;
use crate::platform::connection::V8Connection;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    /// Root path of the project sources
    pub base_path: PathBuf,

    /// Working directory for temp files and hash storages
    pub work_path: PathBuf,

    /// Source format: DESIGNER or EDT
    #[serde(default = "default_format")]
    pub format: SourceFormat,

    /// Builder backend: DESIGNER or IBCMD
    #[serde(default = "default_builder")]
    pub builder: BuilderBackend,

    /// Infobase connection and credentials contract.
    pub infobase: InfobaseConfig,

    /// Source sets (configuration + extensions)
    #[serde(rename = "source-set")]
    pub source_sets: Vec<SourceSetConfig>,

    /// Build pipeline configuration
    #[serde(default)]
    pub build: BuildConfig,

    /// Platform tools configuration
    #[serde(default)]
    pub tools: ToolsConfig,

    /// MCP transport configuration
    #[serde(default)]
    pub mcp: McpConfig,

    /// Test pipeline configuration
    #[serde(default)]
    pub tests: TestsConfig,
}

/// Connection and credentials for the target infobase.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InfobaseConfig {
    /// Connection string to the infobase.
    pub connection: String,

    /// Optional infobase user name passed to platform utilities.
    pub user: Option<String>,

    /// Optional infobase password passed to platform utilities.
    pub password: Option<String>,

    /// Optional DBMS contract for server-based infobases.
    #[serde(default)]
    pub dbms: Option<InfobaseDbmsConfig>,
}

impl InfobaseConfig {
    /// Build a file-based infobase config.
    #[cfg(test)]
    pub fn file(connection: impl Into<String>) -> Self {
        Self {
            connection: connection.into(),
            user: None,
            password: None,
            dbms: None,
        }
    }

    /// Attach infobase credentials to an existing config.
    #[cfg(test)]
    pub fn with_credentials(mut self, user: Option<String>, password: Option<String>) -> Self {
        self.user = user;
        self.password = password;
        self
    }

    /// Build a server-based infobase config.
    #[cfg(test)]
    pub fn server(connection: impl Into<String>, dbms: InfobaseDbmsConfig) -> Self {
        Self {
            connection: connection.into(),
            user: None,
            password: None,
            dbms: Some(dbms),
        }
    }
}

/// DBMS-level contract used by `IBCMD` for server-based infobases.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct InfobaseDbmsConfig {
    /// DBMS kind passed as `--dbms`.
    #[serde(default)]
    pub kind: Option<String>,

    /// DBMS server passed as `--database-server`.
    #[serde(default)]
    pub server: Option<String>,

    /// Physical database name passed as `--database-name`.
    #[serde(default)]
    pub name: Option<String>,

    /// Optional DBMS user passed as `--database-user`.
    #[serde(default)]
    pub user: Option<String>,

    /// Optional DBMS password passed as `--database-password`.
    #[serde(default)]
    pub password: Option<String>,
}

impl InfobaseDbmsConfig {
    /// Build a DBMS contract with mandatory fields populated.
    #[cfg(test)]
    pub fn new(
        kind: impl Into<String>,
        server: impl Into<String>,
        name: impl Into<String>,
    ) -> Self {
        Self {
            kind: Some(kind.into()),
            server: Some(server.into()),
            name: Some(name.into()),
            user: None,
            password: None,
        }
    }

    /// Attach DBMS credentials to an existing contract.
    #[cfg(test)]
    pub fn with_credentials(mut self, user: Option<String>, password: Option<String>) -> Self {
        self.user = user;
        self.password = password;
        self
    }
}

impl AppConfig {
    /// Builds a platform-ready 1C connection with infobase credentials applied.
    pub fn v8_connection(&self) -> V8Connection {
        let mut conn = V8Connection::from_connection_string(&self.infobase.connection);
        conn.user = self.infobase.user.clone();
        conn.password = self.infobase.password.clone();
        conn
    }
}

fn default_format() -> SourceFormat {
    SourceFormat::Designer
}

fn default_builder() -> BuilderBackend {
    BuilderBackend::Designer
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SourceFormat {
    Designer,
    Edt,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BuilderBackend {
    Designer,
    Ibcmd,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SourceSetConfig {
    pub name: String,

    /// YAML `type`: CONFIGURATION, EXTENSION, EXTERNAL_DATA_PROCESSORS, or EXTERNAL_REPORTS.
    #[serde(rename = "type")]
    pub purpose: SourceSetPurpose,

    /// Path relative to basePath (for DESIGNER) or EDT project path
    pub path: PathBuf,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SourceSetPurpose {
    Configuration,
    Extension,
    ExternalDataProcessors,
    ExternalReports,
}

impl SourceSetPurpose {
    pub const fn is_external(self) -> bool {
        matches!(self, Self::ExternalDataProcessors | Self::ExternalReports)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildConfig {
    #[serde(default = "default_partial_load_threshold")]
    pub partial_load_threshold: usize,
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            partial_load_threshold: default_partial_load_threshold(),
        }
    }
}

fn default_partial_load_threshold() -> usize {
    20
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ToolsConfig {
    #[serde(default)]
    pub platform: PlatformToolConfig,

    #[serde(default)]
    pub enterprise: EnterpriseToolConfig,

    #[serde(rename = "edt_cli", alias = "edt-cli", default)]
    pub edt_cli: EdtCliConfig,
}

/// MCP transport-neutral runtime configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, rename_all = "snake_case")]
pub struct McpConfig {
    /// HTTP transport settings for the future MCP server.
    pub http: McpHttpConfig,

    /// Shared execution limits for MCP calls.
    pub execution: McpExecutionConfig,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            http: McpHttpConfig::default(),
            execution: McpExecutionConfig::default(),
        }
    }
}

/// HTTP-specific MCP configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, rename_all = "snake_case")]
pub struct McpHttpConfig {
    /// Socket address for the future HTTP transport listener.
    pub bind_address: String,

    /// URL path that serves MCP HTTP requests.
    pub path: String,

    /// Whether MCP HTTP sessions keep state across requests.
    pub stateful_sessions: bool,

    /// Maximum number of tracked HTTP sessions.
    pub max_sessions: usize,

    /// Idle session eviction timeout in seconds.
    pub idle_ttl_secs: u64,
}

impl Default for McpHttpConfig {
    fn default() -> Self {
        Self {
            bind_address: default_mcp_http_bind_address(),
            path: default_mcp_http_path(),
            stateful_sessions: default_mcp_http_stateful_sessions(),
            max_sessions: default_mcp_http_max_sessions(),
            idle_ttl_secs: default_mcp_http_idle_ttl_secs(),
        }
    }
}

/// Execution guardrails for MCP requests.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, rename_all = "snake_case")]
pub struct McpExecutionConfig {
    /// Maximum number of MCP calls allowed to execute concurrently.
    pub max_concurrent_calls: usize,

    /// Grace period for shutdown drain in seconds.
    pub shutdown_grace_period_secs: u64,
}

impl Default for McpExecutionConfig {
    fn default() -> Self {
        Self {
            max_concurrent_calls: default_mcp_execution_max_concurrent_calls(),
            shutdown_grace_period_secs: default_mcp_execution_shutdown_grace_period_secs(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, rename_all = "snake_case")]
pub struct TestsConfig {
    #[serde(default = "default_test_execution_timeout_seconds")]
    pub execution_timeout_seconds: u64,

    #[serde(default)]
    pub yaxunit: YaxunitTestConfig,

    #[serde(default)]
    pub va: VanessaTestConfig,
}

impl Default for TestsConfig {
    fn default() -> Self {
        Self {
            execution_timeout_seconds: default_test_execution_timeout_seconds(),
            yaxunit: YaxunitTestConfig::default(),
            va: VanessaTestConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(default, rename_all = "snake_case")]
pub struct YaxunitTestConfig {
    pub timeouts: ExecutionTimeouts,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(default, rename_all = "snake_case")]
pub struct VanessaTestConfig {
    pub epf_path: Option<PathBuf>,
    pub params_path: Option<PathBuf>,
    pub profile: Option<String>,
    pub fail_fast: bool,
    pub timeouts: ExecutionTimeouts,
    pub profiles: BTreeMap<String, VanessaProfileConfig>,
}

impl VanessaTestConfig {
    pub fn is_configured(&self) -> bool {
        self.epf_path.is_some()
            || self.params_path.is_some()
            || self.profile.is_some()
            || !self.profiles.is_empty()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(default, rename_all = "snake_case")]
pub struct VanessaProfileConfig {
    pub feature_path: Option<PathBuf>,
    pub features_to_run: Vec<String>,
    pub filter_tags: Vec<String>,
    pub ignore_tags: Vec<String>,
    pub scenario_filter: Vec<String>,
}

fn default_test_execution_timeout_seconds() -> u64 {
    300
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct PlatformToolConfig {
    /// Installation hint for platform utilities.
    ///
    /// May point to a concrete binary (`1cv8`, `1cv8c`, `ibcmd`), to an installation `bin`
    /// directory, or to a platform root that contains versioned subdirectories.
    pub path: Option<PathBuf>,

    /// Platform version requirement in `major.minor`, `major.minor.patch`, or
    /// `major.minor.patch.build` format.
    ///
    /// A 2- or 3-part value selects the highest matching version; a 4-part value
    /// selects an exact build.
    pub version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct EnterpriseToolConfig {
    /// Additional command-line keys appended to enterprise client launches.
    #[serde(
        default,
        alias = "additional_launch_keys",
        alias = "additionalLaunchKeys"
    )]
    pub additional_launch_keys: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct EdtCliConfig {
    /// Path to 1cedtcli binary, installation root, or version-like discovery hint.
    pub path: Option<PathBuf>,

    /// Optional EDT version hint used for auto-discovery, for example `1c-edt-2025.2.3`.
    pub version: Option<String>,

    /// Use long-lived interactive `1cedtcli` processes instead of one-shot invocations.
    #[serde(default)]
    pub interactive_mode: bool,

    /// Eagerly prewarm the shared EDT session on MCP server startup.
    ///
    /// Short-lived CLI commands ignore this flag and start EDT lazily on demand.
    #[serde(default)]
    pub auto_start: bool,

    /// Time limit for EDT startup until the prompt is ready.
    #[serde(
        default = "default_edt_cli_startup_timeout_ms",
        rename = "startup_timeout_ms",
        alias = "startup-timeout-ms"
    )]
    pub startup_timeout_ms: u64,

    /// Default timeout for interactive EDT commands.
    #[serde(
        default = "default_edt_cli_command_timeout_ms",
        rename = "command_timeout_ms",
        alias = "command-timeout-ms"
    )]
    pub command_timeout_ms: u64,
}

impl Default for EdtCliConfig {
    fn default() -> Self {
        Self {
            path: None,
            version: None,
            interactive_mode: false,
            auto_start: false,
            startup_timeout_ms: default_edt_cli_startup_timeout_ms(),
            command_timeout_ms: default_edt_cli_command_timeout_ms(),
        }
    }
}

fn default_mcp_http_bind_address() -> String {
    "127.0.0.1:3000".to_owned()
}

fn default_mcp_http_path() -> String {
    "/mcp".to_owned()
}

const fn default_mcp_http_stateful_sessions() -> bool {
    true
}

const fn default_mcp_http_max_sessions() -> usize {
    64
}

const fn default_mcp_http_idle_ttl_secs() -> u64 {
    900
}

const fn default_mcp_execution_max_concurrent_calls() -> usize {
    1
}

const fn default_mcp_execution_shutdown_grace_period_secs() -> u64 {
    30
}

const fn default_edt_cli_startup_timeout_ms() -> u64 {
    300_000
}

const fn default_edt_cli_command_timeout_ms() -> u64 {
    300_000
}
