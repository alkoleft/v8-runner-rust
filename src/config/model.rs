use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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

    /// Connection string to the infobase
    pub connection: String,

    /// Source sets (configuration + extensions)
    #[serde(rename = "source-set")]
    pub source_sets: Vec<SourceSetConfig>,

    /// Build pipeline configuration
    #[serde(default)]
    pub build: BuildConfig,

    /// Platform tools configuration
    #[serde(default)]
    pub tools: ToolsConfig,

    /// Test pipeline configuration
    #[serde(default)]
    pub tests: TestsConfig,
}

fn default_format() -> SourceFormat {
    SourceFormat::Designer
}

fn default_builder() -> BuilderBackend {
    BuilderBackend::Designer
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SourceFormat {
    Designer,
    Edt,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BuilderBackend {
    Designer,
    Ibcmd,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SourceSetConfig {
    pub name: String,

    /// CONFIGURATION or EXTENSION
    pub purpose: SourceSetPurpose,

    /// Path relative to basePath (for DESIGNER) or EDT project path
    pub path: PathBuf,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SourceSetPurpose {
    Configuration,
    Extension,
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

    #[serde(rename = "edt-cli", default)]
    pub edt_cli: EdtCliConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TestsConfig {
    #[serde(default = "default_test_execution_timeout_seconds")]
    pub execution_timeout_seconds: u64,
}

impl Default for TestsConfig {
    fn default() -> Self {
        Self {
            execution_timeout_seconds: default_test_execution_timeout_seconds(),
        }
    }
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

    /// Exact platform version in `major.minor.patch.build` format, for example `8.3.25.1234`.
    pub version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct EdtCliConfig {
    /// Path to 1cedtcli binary
    pub path: Option<PathBuf>,

    /// Auto-start interactive EDT session on startup
    #[serde(default)]
    pub auto_start: bool,
}
