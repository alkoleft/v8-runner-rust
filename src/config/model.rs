use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::de::Error as _;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::domain::execution::ExecutionTimeouts;
use crate::platform::connection::V8Connection;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    /// Root path of the project sources
    pub base_path: PathBuf,

    /// Working directory for temp files and hash storages
    pub work_path: PathBuf,

    /// Global execution budget for public CLI and MCP commands in milliseconds.
    #[serde(rename = "execution_timeout", default = "default_execution_timeout_ms")]
    pub execution_timeout: u64,

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

    /// Returns the global execution timeout as a duration.
    pub fn execution_timeout_duration(&self) -> Duration {
        Duration::from_millis(self.execution_timeout.max(1))
    }
}

fn default_format() -> SourceFormat {
    SourceFormat::Designer
}

fn default_builder() -> BuilderBackend {
    BuilderBackend::Designer
}

fn default_execution_timeout_ms() -> u64 {
    300_000
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

    #[serde(rename = "edt_cli", default)]
    pub edt_cli: EdtCliConfig,

    #[serde(default)]
    pub client_mcp: ClientMcpToolConfig,

    #[serde(default)]
    pub va: VanessaToolConfig,
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

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(default, rename_all = "snake_case")]
pub struct ClientMcpToolConfig {
    /// Default port passed to onec-client-mcp-devkit via `/C"...;mcpPort=<PORT>"`.
    pub port: Option<u16>,

    /// Optional tool extension prepared by `build` for client MCP launches.
    pub extension: Option<ToolExtensionConfig>,

    /// Default transport for the MCP client side: `ws`, `legacy` or `auto`.
    /// When omitted, runtime treats it as `auto` (probe manager, fall back
    /// to legacy local HTTP MCP).
    pub transport: Option<String>,

    /// Default WS endpoint for the session-manager
    /// (e.g. `ws://127.0.0.1:4000/sessions`).
    pub manager_url: Option<String>,

    /// Default `mcp_log_level` value forwarded into the `/C` payload.
    pub log_level: Option<String>,

    /// Default `mcp_ws_timeout_ms` value forwarded into the `/C` payload.
    pub ws_timeout_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct ToolExtensionConfig {
    /// Extension name in the target infobase.
    pub name: String,

    /// Mutually exclusive extension input.
    pub input: ToolExtensionInput,
}

impl ToolExtensionConfig {
    pub fn source(&self) -> Option<&ToolExtensionSourceConfig> {
        match &self.input {
            ToolExtensionInput::Source(source) => Some(source),
            ToolExtensionInput::Artifact(_) => None,
        }
    }

    pub fn source_mut(&mut self) -> Option<&mut ToolExtensionSourceConfig> {
        match &mut self.input {
            ToolExtensionInput::Source(source) => Some(source),
            ToolExtensionInput::Artifact(_) => None,
        }
    }

    pub fn artifact_mut(&mut self) -> Option<&mut ToolExtensionArtifactConfig> {
        match &mut self.input {
            ToolExtensionInput::Source(_) => None,
            ToolExtensionInput::Artifact(artifact) => Some(artifact),
        }
    }
}

impl<'de> Deserialize<'de> for ToolExtensionConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize, Default)]
        #[serde(default, rename_all = "snake_case")]
        struct RawToolExtensionConfig {
            name: String,
            source: Option<ToolExtensionSourceConfig>,
            artifact: Option<ToolExtensionArtifactConfig>,
        }

        let raw = RawToolExtensionConfig::deserialize(deserializer)?;
        let input = match (raw.source, raw.artifact) {
            (Some(source), None) => ToolExtensionInput::Source(source),
            (None, Some(artifact)) => ToolExtensionInput::Artifact(artifact),
            (Some(_), Some(_)) => {
                return Err(D::Error::custom(
                    "tools.client_mcp.extension must specify exactly one of source or artifact",
                ))
            }
            (None, None) => {
                return Err(D::Error::custom(
                    "tools.client_mcp.extension must specify exactly one of source or artifact",
                ))
            }
        };

        Ok(Self {
            name: raw.name,
            input,
        })
    }
}

impl Serialize for ToolExtensionConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("ToolExtensionConfig", 2)?;
        state.serialize_field("name", &self.name)?;
        match &self.input {
            ToolExtensionInput::Source(source) => state.serialize_field("source", source)?,
            ToolExtensionInput::Artifact(artifact) => {
                state.serialize_field("artifact", artifact)?
            }
        }
        state.end()
    }
}

#[derive(Debug, Clone)]
pub enum ToolExtensionInput {
    Source(ToolExtensionSourceConfig),
    Artifact(ToolExtensionArtifactConfig),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, rename_all = "snake_case")]
pub struct ToolExtensionSourceConfig {
    /// Path to extension sources.
    pub path: PathBuf,

    /// Optional source format. When omitted, the project-level `format` is used.
    pub format: Option<SourceFormat>,
}

impl Default for ToolExtensionSourceConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::new(),
            format: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, rename_all = "snake_case")]
pub struct ToolExtensionArtifactConfig {
    /// Path to a `.cfe` artifact.
    pub path: PathBuf,
}

impl Default for ToolExtensionArtifactConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(default, rename_all = "snake_case")]
pub struct VanessaToolConfig {
    /// Path to the Vanessa Automation external data processor used by `test va` and `launch mcp va`.
    pub epf_path: Option<PathBuf>,
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
    pub params_path: Option<PathBuf>,
    pub profile: Option<String>,
    pub fail_fast: bool,
    pub timeouts: ExecutionTimeouts,
    pub profiles: BTreeMap<String, VanessaProfileConfig>,
}

impl VanessaTestConfig {
    pub fn is_configured(&self) -> bool {
        self.params_path.is_some() || self.profile.is_some() || !self.profiles.is_empty()
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
    #[serde(default)]
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
        rename = "startup_timeout_ms"
    )]
    pub startup_timeout_ms: u64,

    /// Default timeout for interactive EDT commands.
    #[serde(
        default = "default_edt_cli_command_timeout_ms",
        rename = "command_timeout_ms"
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
