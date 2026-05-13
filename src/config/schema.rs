#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use schemars::{schema_for, JsonSchema};
use serde::de::Error as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const MAIN_CONFIG_SCHEMA_PATH: &str = "docs/schemas/v8project.schema.json";
pub const LOCAL_CONFIG_SCHEMA_PATH: &str = "docs/schemas/v8project.local.schema.json";

const REPOSITORY_RAW_SCHEMA_BASE: &str =
    "https://raw.githubusercontent.com/alkoleft/v8-runner-rust/master/docs/schemas";

pub fn main_config_schema_url() -> String {
    schema_url("v8project.schema.json")
}

pub fn local_config_schema_url() -> String {
    schema_url("v8project.local.schema.json")
}

pub fn main_config_schema_json() -> Value {
    let mut schema = serde_json::to_value(schema_for!(MainConfigSchema)).expect("schema json");
    set_schema_id(&mut schema, &main_config_schema_url());
    add_tool_extension_schema_constraints(&mut schema);
    add_numeric_runtime_bounds(&mut schema);
    schema
}

pub fn local_config_schema_json() -> Value {
    let mut schema =
        serde_json::to_value(schema_for!(LocalOverlayConfigSchema)).expect("schema json");
    set_schema_id(&mut schema, &local_config_schema_url());
    add_tool_extension_schema_constraints(&mut schema);
    add_numeric_runtime_bounds(&mut schema);
    schema
}

pub fn validate_main_config_schema_boundary(
    root: serde_yaml::Value,
) -> Result<(), serde_yaml::Error> {
    serde_yaml::from_value::<MainConfigSchema>(root).map(|_| ())
}

pub fn validate_local_overlay_schema_boundary(
    root: serde_yaml::Value,
) -> Result<(), serde_yaml::Error> {
    serde_yaml::from_value::<LocalOverlayConfigSchema>(root).map(|_| ())
}

pub fn schema_json_pretty(schema: &Value) -> String {
    let mut text = serde_json::to_string_pretty(schema).expect("schema json");
    text.push('\n');
    text
}

fn schema_url(file_name: &str) -> String {
    format!("{REPOSITORY_RAW_SCHEMA_BASE}/{file_name}")
}

fn set_schema_id(schema: &mut Value, id: &str) {
    let object = schema.as_object_mut().expect("root schema object");
    object.insert("$id".to_owned(), Value::String(id.to_owned()));
}

fn add_tool_extension_schema_constraints(schema: &mut Value) {
    if let Some(object) = schema_object_mut(schema, &["ToolExtensionSchema"]) {
        object.insert(
            "oneOf".to_owned(),
            json!([
                {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "name": { "type": "string" },
                        "source": { "$ref": "#/$defs/ToolExtensionSourceSchema" },
                        "artifact": { "type": "null" }
                    },
                    "required": ["name", "source"]
                },
                {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "name": { "type": "string" },
                        "source": { "type": "null" },
                        "artifact": { "$ref": "#/$defs/ToolExtensionArtifactSchema" }
                    },
                    "required": ["name", "artifact"]
                }
            ]),
        );
    }

    allow_null_property(schema, &["PartialClientMcpToolSchema"], "extension");
    allow_null_property(schema, &["ToolExtensionSchema"], "source");
    allow_null_property(schema, &["ToolExtensionSchema"], "artifact");
    allow_null_property(schema, &["PartialToolExtensionSchema"], "source");
    allow_null_property(schema, &["PartialToolExtensionSchema"], "artifact");
    reject_multiple_non_null_properties(
        schema,
        &["PartialToolExtensionSchema"],
        &["source", "artifact"],
    );
    reject_multiple_null_properties(
        schema,
        &["PartialToolExtensionSchema"],
        &["source", "artifact"],
    );
}

fn reject_multiple_non_null_properties(schema: &mut Value, def_path: &[&str], names: &[&str]) {
    let Some(object) = schema_object_mut(schema, def_path) else {
        return;
    };
    let all_of = object
        .entry("allOf")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .expect("schema allOf array");

    for i in 0..names.len() {
        for j in (i + 1)..names.len() {
            all_of.push(json!({
                "not": {
                    "allOf": [
                        {
                            "required": [names[i]],
                            "properties": {
                                names[i]: { "not": { "type": "null" } }
                            }
                        },
                        {
                            "required": [names[j]],
                            "properties": {
                                names[j]: { "not": { "type": "null" } }
                            }
                        }
                    ]
                }
            }));
        }
    }
}

fn reject_multiple_null_properties(schema: &mut Value, def_path: &[&str], names: &[&str]) {
    let Some(object) = schema_object_mut(schema, def_path) else {
        return;
    };
    let all_of = object
        .entry("allOf")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .expect("schema allOf array");

    for i in 0..names.len() {
        for j in (i + 1)..names.len() {
            all_of.push(json!({
                "not": {
                    "allOf": [
                        {
                            "required": [names[i]],
                            "properties": {
                                names[i]: { "type": "null" }
                            }
                        },
                        {
                            "required": [names[j]],
                            "properties": {
                                names[j]: { "type": "null" }
                            }
                        }
                    ]
                }
            }));
        }
    }
}

fn allow_null_property(schema: &mut Value, def_path: &[&str], property: &str) {
    let Some(object) = schema_object_mut(schema, def_path) else {
        return;
    };
    let Some(properties) = object.get_mut("properties").and_then(Value::as_object_mut) else {
        return;
    };
    let Some(current) = properties.remove(property) else {
        return;
    };
    let description = current.get("description").cloned();

    let mut replacement = json!({
            "anyOf": [
                current,
                { "type": "null" }
            ]
    });
    if let (Some(object), Some(description)) = (replacement.as_object_mut(), description) {
        object.insert("description".to_owned(), description);
    }

    properties.insert(property.to_owned(), replacement);
}

fn add_numeric_runtime_bounds(schema: &mut Value) {
    set_numeric_bounds(schema, &[], "execution_timeout", Some(1), Some(86_400_000));
    set_numeric_bounds(
        schema,
        &["BuildSchema"],
        "partialLoadThreshold",
        Some(1),
        None,
    );
    set_numeric_bounds(
        schema,
        &["EdtCliSchema"],
        "startup_timeout_ms",
        Some(1),
        None,
    );
    set_numeric_bounds(
        schema,
        &["EdtCliSchema"],
        "command_timeout_ms",
        Some(1),
        None,
    );
    for def in ["ClientMcpToolSchema", "PartialClientMcpToolSchema"] {
        set_numeric_bounds(schema, &[def], "port", Some(1), None);
    }
    for name in ["max_sessions", "idle_ttl_secs"] {
        set_numeric_bounds(schema, &["McpHttpSchema"], name, Some(1), None);
    }
    for name in ["max_concurrent_calls", "shutdown_grace_period_secs"] {
        set_numeric_bounds(schema, &["McpExecutionSchema"], name, Some(1), None);
    }
    set_numeric_bounds(
        schema,
        &["TestsSchema"],
        "execution_timeout_seconds",
        Some(1),
        Some(86_400),
    );
    for name in ["startup_ms", "run_ms", "total_ms"] {
        set_numeric_bounds(schema, &["ExecutionTimeoutsSchema"], name, Some(1), None);
    }
}

fn set_numeric_bounds(
    schema: &mut Value,
    def_path: &[&str],
    property: &str,
    minimum: Option<u64>,
    maximum: Option<u64>,
) {
    let Some(object) = schema_object_mut(schema, def_path) else {
        return;
    };
    let Some(property) = object
        .get_mut("properties")
        .and_then(Value::as_object_mut)
        .and_then(|properties| properties.get_mut(property))
        .and_then(Value::as_object_mut)
    else {
        return;
    };

    if let Some(minimum) = minimum {
        property.insert("minimum".to_owned(), Value::from(minimum));
    }
    if let Some(maximum) = maximum {
        property.insert("maximum".to_owned(), Value::from(maximum));
    }
}

fn schema_object_mut<'a>(
    schema: &'a mut Value,
    def_path: &[&str],
) -> Option<&'a mut serde_json::Map<String, Value>> {
    let mut object = schema.as_object_mut().expect("root schema object");
    for def in def_path {
        object = object
            .get_mut("$defs")
            .and_then(Value::as_object_mut)
            .and_then(|defs| defs.get_mut(*def))
            .and_then(Value::as_object_mut)?;
    }
    Some(object)
}

fn deserialize_non_null_optional<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    let value = Option::<serde_yaml::Value>::deserialize(deserializer)?
        .ok_or_else(|| D::Error::custom("null is not allowed for this field"))?;
    T::deserialize(value).map(Some).map_err(D::Error::custom)
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MainConfigSchema {
    /// Working directory for generated state, logs, temporary files, and hash storages.
    work_path: PathBuf,
    /// Global execution budget for public CLI and MCP commands in milliseconds.
    #[serde(
        rename = "execution_timeout",
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    execution_timeout: Option<u64>,
    /// Source format used by project source sets when a nested source does not override it.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "SourceFormatSchema")]
    format: Option<SourceFormatSchema>,
    /// Backend used for build/load operations.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "BuilderBackendSchema")]
    builder: Option<BuilderBackendSchema>,
    /// Target infobase connection, credentials, and optional DBMS settings.
    infobase: InfobaseSchema,
    /// Project source sets to build, test, dump, or materialize.
    #[serde(rename = "source-set")]
    source_sets: Vec<SourceSetSchema>,
    /// Build pipeline settings.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "BuildSchema")]
    build: Option<BuildSchema>,
    /// External tool discovery and launch settings.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "ToolsSchema")]
    tools: Option<ToolsSchema>,
    /// MCP server transport and execution settings.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "McpSchema")]
    mcp: Option<McpSchema>,
    /// Test runner defaults and profile settings.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "TestsSchema")]
    tests: Option<TestsSchema>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LocalOverlayConfigSchema {
    /// Machine-local working directory override.
    #[serde(
        rename = "workPath",
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "PathBuf")]
    work_path: Option<PathBuf>,
    /// Machine-local infobase credentials and connection overrides.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "PartialInfobaseSchema")]
    infobase: Option<PartialInfobaseSchema>,
    /// Machine-local tool discovery and launch overrides.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "PartialToolsSchema")]
    tools: Option<PartialToolsSchema>,
    /// Machine-local test runner overrides.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "PartialTestsSchema")]
    tests: Option<PartialTestsSchema>,
    /// Machine-local MCP runtime overrides.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "PartialMcpSchema")]
    mcp: Option<PartialMcpSchema>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum SourceFormatSchema {
    Designer,
    Edt,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum BuilderBackendSchema {
    Designer,
    Ibcmd,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct InfobaseSchema {
    /// 1C infobase connection string without embedded user or password.
    connection: String,
    /// Optional infobase user name passed to platform utilities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    user: Option<String>,
    /// Optional infobase password passed to platform utilities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    password: Option<String>,
    /// Optional unlock code. Non-empty value is propagated as `/UC <value>` to DESIGNER;
    /// empty string means no unlock code and `/UC` is not passed. Masked in command logs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    unlock_code: Option<String>,
    /// Optional DBMS settings for server-based infobases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dbms: Option<InfobaseDbmsSchema>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PartialInfobaseSchema {
    /// Optional local override for the 1C infobase connection string.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    connection: Option<String>,
    /// Optional local infobase user name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    user: Option<String>,
    /// Optional local infobase password.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    password: Option<String>,
    /// Optional local infobase unlock code. Non-empty value is propagated as `/UC <value>`;
    /// empty string means no unlock code and `/UC` is not passed. Masked in command logs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    unlock_code: Option<String>,
    /// Optional local DBMS settings override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dbms: Option<PartialInfobaseDbmsSchema>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct InfobaseDbmsSchema {
    /// DBMS kind passed to `ibcmd --dbms`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    /// DBMS server passed to `ibcmd --database-server`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    server: Option<String>,
    /// Physical database name passed to `ibcmd --database-name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    /// Optional DBMS user passed to `ibcmd --database-user`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    user: Option<String>,
    /// Optional DBMS password passed to `ibcmd --database-password`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    password: Option<String>,
}

type PartialInfobaseDbmsSchema = InfobaseDbmsSchema;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SourceSetSchema {
    /// Source-set name used by CLI filters and diagnostics.
    name: String,
    /// Source-set type: configuration, extension, external data processors, or external reports.
    #[serde(rename = "type")]
    purpose: SourceSetPurposeSchema,
    /// Source path relative to the primary config directory or an EDT project path.
    path: PathBuf,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum SourceSetPurposeSchema {
    Configuration,
    Extension,
    ExternalDataProcessors,
    ExternalReports,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BuildSchema {
    /// Maximum changed-file count for partial Designer load before falling back to full load.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "usize")]
    partial_load_threshold: Option<usize>,
    /// Default `/UpdateDBCfg -Dynamic+` toggle for `build`. CLI `--dynamic` overrides this.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool")]
    dynamic_update: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ToolsSchema {
    /// Platform executable discovery settings.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "PlatformToolSchema")]
    platform: Option<PlatformToolSchema>,
    /// Enterprise client launch settings.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "EnterpriseToolSchema")]
    enterprise: Option<EnterpriseToolSchema>,
    /// EDT CLI discovery and execution settings.
    #[serde(
        rename = "edt_cli",
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "EdtCliSchema")]
    edt_cli: Option<EdtCliSchema>,
    /// onec-client-mcp tool settings.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "ClientMcpToolSchema")]
    client_mcp: Option<ClientMcpToolSchema>,
    /// Vanessa Automation tool settings.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "VanessaToolSchema")]
    va: Option<VanessaToolSchema>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PartialToolsSchema {
    /// Machine-local platform executable discovery settings.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "PlatformToolSchema")]
    platform: Option<PlatformToolSchema>,
    /// Machine-local enterprise client launch settings.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "EnterpriseToolSchema")]
    enterprise: Option<EnterpriseToolSchema>,
    /// Machine-local EDT CLI discovery and execution settings.
    #[serde(
        rename = "edt_cli",
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "EdtCliSchema")]
    edt_cli: Option<EdtCliSchema>,
    /// Machine-local onec-client-mcp tool settings.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "PartialClientMcpToolSchema")]
    client_mcp: Option<PartialClientMcpToolSchema>,
    /// Machine-local Vanessa Automation tool settings.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "VanessaToolSchema")]
    va: Option<VanessaToolSchema>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PlatformToolSchema {
    /// Platform binary, installation `bin` directory, or platform root discovery hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<PathBuf>,
    /// Platform version requirement used for discovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct EnterpriseToolSchema {
    /// Additional command-line keys appended to enterprise client launches.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Vec<String>")]
    additional_launch_keys: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct EdtCliSchema {
    /// Path to `1cedtcli`, EDT installation root, or version-like discovery hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<PathBuf>,
    /// Optional EDT version hint used for auto-discovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    /// Use long-lived interactive `1cedtcli` processes instead of one-shot invocations.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool")]
    interactive_mode: Option<bool>,
    /// Eagerly prewarm the shared EDT session on MCP server startup.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool")]
    auto_start: Option<bool>,
    /// Time limit for EDT startup until the prompt is ready, in milliseconds.
    #[serde(
        rename = "startup_timeout_ms",
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    startup_timeout_ms: Option<u64>,
    /// Default timeout for interactive EDT commands, in milliseconds.
    #[serde(
        rename = "command_timeout_ms",
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    command_timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct ClientMcpToolSchema {
    /// Default port passed to onec-client-mcp-devkit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    port: Option<u16>,
    /// Optional tool extension prepared by `build` for client MCP launches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    extension: Option<ToolExtensionSchema>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct PartialClientMcpToolSchema {
    /// Machine-local default port passed to onec-client-mcp-devkit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    port: Option<u16>,
    /// Machine-local override or reset for the client MCP tool extension.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "PartialToolExtensionSchema")]
    extension: Option<PartialToolExtensionSchema>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct ToolExtensionSchema {
    /// Extension name in the target infobase.
    name: String,
    /// Source-backed extension input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ToolExtensionSourceSchema")]
    source: Option<ToolExtensionSourceSchema>,
    /// Artifact-backed `.cfe` extension input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ToolExtensionArtifactSchema")]
    artifact: Option<ToolExtensionArtifactSchema>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct PartialToolExtensionSchema {
    /// Machine-local extension name override.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    name: Option<String>,
    /// Machine-local source-backed extension input or branch-switch reset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "PartialToolExtensionSourceSchema")]
    source: Option<PartialToolExtensionSourceSchema>,
    /// Machine-local artifact-backed `.cfe` extension input or branch-switch reset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ToolExtensionArtifactSchema")]
    artifact: Option<ToolExtensionArtifactSchema>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct ToolExtensionSourceBranchSchema {
    /// Extension name for the source-backed branch.
    name: String,
    /// Source-backed extension branch.
    source: ToolExtensionSourceSchema,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct ToolExtensionArtifactBranchSchema {
    /// Extension name for the artifact-backed branch.
    name: String,
    /// Artifact-backed extension branch.
    artifact: ToolExtensionArtifactSchema,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct ToolExtensionSourceSchema {
    /// Path to extension sources.
    path: PathBuf,
    /// Optional source format; when omitted, the project-level format is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    format: Option<SourceFormatSchema>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct PartialToolExtensionSourceSchema {
    /// Machine-local path override for extension sources.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "PathBuf")]
    path: Option<PathBuf>,
    /// Machine-local source format override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    format: Option<SourceFormatSchema>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct ToolExtensionArtifactSchema {
    /// Path to a `.cfe` extension artifact.
    path: PathBuf,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct VanessaToolSchema {
    /// Path to the Vanessa Automation external data processor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    epf_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct McpSchema {
    /// MCP HTTP transport settings.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "McpHttpSchema")]
    http: Option<McpHttpSchema>,
    /// Shared execution limits for MCP calls.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "McpExecutionSchema")]
    execution: Option<McpExecutionSchema>,
}

type PartialMcpSchema = McpSchema;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct McpHttpSchema {
    /// Socket address for the MCP HTTP listener.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    bind_address: Option<String>,
    /// URL path that serves MCP HTTP requests.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    path: Option<String>,
    /// Whether MCP HTTP sessions keep state across requests.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool")]
    stateful_sessions: Option<bool>,
    /// Maximum number of tracked HTTP sessions.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "usize")]
    max_sessions: Option<usize>,
    /// Idle HTTP session eviction timeout in seconds.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    idle_ttl_secs: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct McpExecutionSchema {
    /// Maximum number of MCP calls allowed to execute concurrently.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "usize")]
    max_concurrent_calls: Option<usize>,
    /// Grace period for shutdown drain in seconds.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    shutdown_grace_period_secs: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct TestsSchema {
    /// Default test execution timeout in seconds.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u64")]
    execution_timeout_seconds: Option<u64>,
    /// YAxUnit test runner settings.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "YaxunitTestSchema")]
    yaxunit: Option<YaxunitTestSchema>,
    /// Vanessa Automation test runner settings.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "VanessaTestSchema")]
    va: Option<VanessaTestSchema>,
}

type PartialTestsSchema = TestsSchema;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct YaxunitTestSchema {
    /// YAxUnit startup, run, and total timeout overrides.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "ExecutionTimeoutsSchema")]
    timeouts: Option<ExecutionTimeoutsSchema>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct VanessaTestSchema {
    /// Path to generated Vanessa parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    params_path: Option<PathBuf>,
    /// Vanessa profile name to use by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profile: Option<String>,
    /// Stop the Vanessa test run after the first failure.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool")]
    fail_fast: Option<bool>,
    /// Vanessa startup, run, and total timeout overrides.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "ExecutionTimeoutsSchema")]
    timeouts: Option<ExecutionTimeoutsSchema>,
    /// Named Vanessa profiles available to CLI and MCP test runs.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "BTreeMap<String, VanessaProfileSchema>")]
    profiles: Option<BTreeMap<String, VanessaProfileSchema>>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct VanessaProfileSchema {
    /// Feature file or directory used by this Vanessa profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    feature_path: Option<PathBuf>,
    /// Explicit feature files or directories to run.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Vec<String>")]
    features_to_run: Option<Vec<String>>,
    /// Tags included by this Vanessa profile.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Vec<String>")]
    filter_tags: Option<Vec<String>>,
    /// Tags ignored by this Vanessa profile.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Vec<String>")]
    ignore_tags: Option<Vec<String>>,
    /// Scenario name filters used by this Vanessa profile.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Vec<String>")]
    scenario_filter: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct ExecutionTimeoutsSchema {
    /// Startup timeout in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    startup_ms: Option<u64>,
    /// Main execution timeout in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    run_ms: Option<u64>,
    /// Total execution timeout in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    total_ms: Option<u64>,
}

#[cfg(test)]
mod tests {
    use crate::config::loader::load_config;

    use super::{
        local_config_schema_json, main_config_schema_json, schema_json_pretty,
        LOCAL_CONFIG_SCHEMA_PATH, MAIN_CONFIG_SCHEMA_PATH,
    };
    use std::path::Path;

    const REMOVED_SCHEMA_ALIAS_PROPERTIES: &[&str] = &[
        "basePath",
        "executionTimeout",
        "execution_timeout_ms",
        "edt-cli",
        "additional_launch_keys",
        "additionalLaunchKeys",
        "startup-timeout-ms",
        "command-timeout-ms",
    ];

    #[test]
    fn generated_schema_artifacts_are_current() {
        maybe_update_schema_artifacts();

        assert_schema_file(MAIN_CONFIG_SCHEMA_PATH, &main_config_schema_json());
        assert_schema_file(LOCAL_CONFIG_SCHEMA_PATH, &local_config_schema_json());
    }

    #[test]
    fn generated_schemas_omit_removed_alias_properties() {
        for schema in [main_config_schema_json(), local_config_schema_json()] {
            for alias in REMOVED_SCHEMA_ALIAS_PROPERTIES {
                assert_no_object_key(&schema, alias);
            }
        }
    }

    #[test]
    fn generated_schemas_include_user_facing_field_descriptions() {
        let main_schema = main_config_schema_json();
        assert_property_description_contains(&main_schema, &[], "workPath", "Working directory");
        assert_property_description_contains(
            &main_schema,
            &[],
            "source-set",
            "Project source sets",
        );
        assert_property_description_contains(
            &main_schema,
            &["InfobaseSchema"],
            "connection",
            "infobase connection string",
        );
        assert_property_description_contains(
            &main_schema,
            &["SourceSetSchema"],
            "type",
            "Source-set type",
        );
        assert_property_description_contains(
            &main_schema,
            &["ToolsSchema"],
            "client_mcp",
            "onec-client-mcp",
        );
        assert_property_description_contains(
            &main_schema,
            &["ToolExtensionSchema"],
            "source",
            "Source-backed extension input",
        );
        assert_property_description_contains(
            &main_schema,
            &["McpHttpSchema"],
            "bind_address",
            "Socket address",
        );
        assert_property_description_contains(
            &main_schema,
            &["TestsSchema"],
            "va",
            "Vanessa Automation",
        );

        let local_schema = local_config_schema_json();
        assert_property_description_contains(
            &local_schema,
            &[],
            "workPath",
            "Machine-local working directory",
        );
        assert_property_description_contains(
            &local_schema,
            &["PartialClientMcpToolSchema"],
            "extension",
            "Machine-local override",
        );
    }

    fn maybe_update_schema_artifacts() {
        if std::env::var_os("UPDATE_CONFIG_SCHEMAS").is_none() {
            return;
        }

        write_schema_file(MAIN_CONFIG_SCHEMA_PATH, &main_config_schema_json());
        write_schema_file(LOCAL_CONFIG_SCHEMA_PATH, &local_config_schema_json());
    }

    fn assert_schema_file(path: &str, generated: &serde_json::Value) {
        let expected = schema_json_pretty(generated);
        let actual = std::fs::read_to_string(path).expect("schema artifact");
        assert_eq!(
            actual, expected,
            "{path} is stale; rerun UPDATE_CONFIG_SCHEMAS=1 cargo test generated_schema_artifacts_are_current"
        );
    }

    fn write_schema_file(path: &str, generated: &serde_json::Value) {
        let path = Path::new(path);
        std::fs::create_dir_all(path.parent().expect("schema dir")).expect("schema dir");
        std::fs::write(path, schema_json_pretty(generated)).expect("write schema artifact");
    }

    #[test]
    fn main_schema_accepts_config_without_base_path_and_loader_defaults_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("Configuration.xml"), "<Configuration/>").expect("xml");
        let config_path = dir.path().join("v8project.yaml");
        std::fs::write(&config_path, minimal_project_config_without_base_path()).expect("config");

        assert_schema_valid(
            &main_config_schema_json(),
            &minimal_project_config_without_base_path(),
        );

        let config = load_config(config_path.to_str(), None).expect("load config");
        assert_eq!(config.base_path, dir.path());
    }

    #[test]
    fn local_schema_accepts_credentials_only_overlay_and_loader_merges_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("Configuration.xml"), "<Configuration/>").expect("xml");
        let config_path = dir.path().join("v8project.yaml");
        std::fs::write(&config_path, minimal_project_config_without_base_path()).expect("config");
        let overlay = "infobase:\n  user: Admin\n  password: secret\n";
        std::fs::write(dir.path().join("v8project.local.yaml"), overlay).expect("overlay");

        assert_schema_valid(&local_config_schema_json(), overlay);

        let config = load_config(config_path.to_str(), None).expect("load config");
        assert_eq!(config.infobase.user.as_deref(), Some("Admin"));
        assert_eq!(config.infobase.password.as_deref(), Some("secret"));
    }

    #[test]
    fn local_schema_and_loader_reject_project_identity_and_unknown_keys() {
        for overlay in [
            "source-set: []\n",
            "format: DESIGNER\n",
            "builder: DESIGNER\n",
            "unknown: value\n",
            "infobase:\n  name: unexpected\n",
            "tools:\n  client_mcp:\n    extension:\n      source:\n        extra: unexpected\n",
            "tools:\n  client_mcp:\n    extension:\n      source:\n        path: ext\n      artifact:\n        path: ext.cfe\n",
            "tools:\n  client_mcp:\n    extension:\n      source: null\n      artifact: null\n",
        ] {
            assert_schema_invalid(&local_config_schema_json(), overlay);
            assert_overlay_loader_error(overlay);
        }
    }

    #[test]
    fn main_schema_and_loader_reject_invalid_tool_extension_shapes() {
        let both = format!(
            "{}tools:\n  client_mcp:\n    extension:\n      name: client_mcp\n      source:\n        path: ext\n      artifact:\n        path: ext.cfe\n",
            minimal_project_config_without_base_path()
        );
        let neither = format!(
            "{}tools:\n  client_mcp:\n    extension:\n      name: client_mcp\n",
            minimal_project_config_without_base_path()
        );

        for config in [both, neither] {
            assert_schema_invalid(&main_config_schema_json(), &config);
            assert_config_loader_error(&config, "must specify exactly one of source or artifact");
        }
    }

    #[test]
    fn main_schema_and_loader_accept_canonical_mixed_config_keys() {
        let config = format!(
            "{}execution_timeout: 300000\ntools:\n  enterprise:\n    additional-launch-keys:\n      - /TESTMANAGER\n  edt_cli:\n    startup_timeout_ms: 300000\n    command_timeout_ms: 300000\n",
            minimal_project_config_without_base_path()
        );

        assert_schema_valid(&main_config_schema_json(), &config);
        assert_config_loader_ok(&config);
    }

    #[test]
    fn local_schema_and_loader_accept_canonical_mixed_config_keys() {
        let overlay = "tools:\n  enterprise:\n    additional-launch-keys:\n      - /TESTMANAGER\n  edt_cli:\n    startup_timeout_ms: 300000\n    command_timeout_ms: 300000\n";

        assert_schema_valid(&local_config_schema_json(), overlay);
        assert_overlay_loader_ok(overlay);
    }

    #[test]
    fn local_schema_accepts_client_mcp_extension_null_reset() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("Configuration.xml"), "<Configuration/>").expect("xml");
        std::fs::write(dir.path().join("client-mcp.cfe"), "artifact").expect("artifact");
        let config_path = dir.path().join("v8project.yaml");
        std::fs::write(
            &config_path,
            format!(
                "{}tools:\n  client_mcp:\n    extension:\n      name: client_mcp\n      artifact:\n        path: client-mcp.cfe\n",
                minimal_project_config_without_base_path()
            ),
        )
        .expect("config");
        let overlay = "tools:\n  client_mcp:\n    extension: null\n";
        std::fs::write(dir.path().join("v8project.local.yaml"), overlay).expect("overlay");

        assert_schema_valid(&local_config_schema_json(), overlay);

        let config = load_config(config_path.to_str(), None).expect("load config");
        assert!(config.tools.client_mcp.extension.is_none());
    }

    #[test]
    fn local_schema_accepts_client_mcp_extension_branch_switch_reset() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("Configuration.xml"), "<Configuration/>").expect("xml");
        std::fs::write(dir.path().join("client-mcp.cfe"), "artifact").expect("artifact");
        let config_path = dir.path().join("v8project.yaml");
        std::fs::write(
            &config_path,
            format!(
                "{}tools:\n  client_mcp:\n    extension:\n      name: client_mcp\n      source:\n        path: client-mcp-source\n",
                minimal_project_config_without_base_path()
            ),
        )
        .expect("config");
        let overlay = "tools:\n  client_mcp:\n    extension:\n      source: null\n      artifact:\n        path: client-mcp.cfe\n";
        std::fs::write(dir.path().join("v8project.local.yaml"), overlay).expect("overlay");

        assert_schema_valid(&local_config_schema_json(), overlay);

        let config = load_config(config_path.to_str(), None).expect("load config");
        let mut extension = config.tools.client_mcp.extension.expect("extension");
        assert!(extension.source().is_none());
        assert!(extension.artifact_mut().is_some());
    }

    #[test]
    fn schemas_and_loader_reject_removed_alias_keys() {
        for config in [
            format!(
                "{}executionTimeout: 300000\n",
                minimal_project_config_without_base_path()
            ),
            format!(
                "{}execution_timeout_ms: 300000\n",
                minimal_project_config_without_base_path()
            ),
            format!(
                "{}tools:\n  edt-cli: {{}}\n",
                minimal_project_config_without_base_path()
            ),
            format!(
                "{}tools:\n  enterprise:\n    additional_launch_keys: []\n",
                minimal_project_config_without_base_path()
            ),
            format!(
                "{}tools:\n  enterprise:\n    additionalLaunchKeys: []\n",
                minimal_project_config_without_base_path()
            ),
            format!(
                "{}tools:\n  edt_cli:\n    startup-timeout-ms: 300000\n",
                minimal_project_config_without_base_path()
            ),
            format!(
                "{}tools:\n  edt_cli:\n    command-timeout-ms: 300000\n",
                minimal_project_config_without_base_path()
            ),
        ] {
            assert_schema_invalid(&main_config_schema_json(), &config);
            assert_config_loader_error_any(&config);
        }

        for overlay in [
            "tools:\n  edt-cli: {}\n",
            "tools:\n  enterprise:\n    additional_launch_keys: []\n",
            "tools:\n  enterprise:\n    additionalLaunchKeys: []\n",
            "tools:\n  edt_cli:\n    startup-timeout-ms: 300000\n",
            "tools:\n  edt_cli:\n    command-timeout-ms: 300000\n",
        ] {
            assert_schema_invalid(&local_config_schema_json(), overlay);
            assert_overlay_loader_error(overlay);
        }
    }

    #[test]
    fn schemas_and_loader_reject_invalid_runtime_numeric_boundaries() {
        for config in [
            format!(
                "{}execution_timeout: 0\n",
                minimal_project_config_without_base_path()
            ),
            format!(
                "{}execution_timeout: 86400001\n",
                minimal_project_config_without_base_path()
            ),
            format!(
                "{}build:\n  partialLoadThreshold: 0\n",
                minimal_project_config_without_base_path()
            ),
            format!(
                "{}tools:\n  client_mcp:\n    port: 0\n",
                minimal_project_config_without_base_path()
            ),
            format!(
                "{}tools:\n  edt_cli:\n    startup_timeout_ms: 0\n",
                minimal_project_config_without_base_path()
            ),
            format!(
                "{}tools:\n  edt_cli:\n    command_timeout_ms: 0\n",
                minimal_project_config_without_base_path()
            ),
            format!(
                "{}mcp:\n  http:\n    max_sessions: 0\n",
                minimal_project_config_without_base_path()
            ),
            format!(
                "{}mcp:\n  http:\n    idle_ttl_secs: 0\n",
                minimal_project_config_without_base_path()
            ),
            format!(
                "{}mcp:\n  execution:\n    max_concurrent_calls: 0\n",
                minimal_project_config_without_base_path()
            ),
            format!(
                "{}mcp:\n  execution:\n    shutdown_grace_period_secs: 0\n",
                minimal_project_config_without_base_path()
            ),
            format!(
                "{}tests:\n  execution_timeout_seconds: 0\n",
                minimal_project_config_without_base_path()
            ),
            format!(
                "{}tests:\n  execution_timeout_seconds: 86401\n",
                minimal_project_config_without_base_path()
            ),
            format!(
                "{}tests:\n  yaxunit:\n    timeouts:\n      startup_ms: 0\n",
                minimal_project_config_without_base_path()
            ),
            format!(
                "{}tests:\n  va:\n    timeouts:\n      run_ms: 0\n",
                minimal_project_config_without_base_path()
            ),
        ] {
            assert_schema_invalid(&main_config_schema_json(), &config);
            assert_config_loader_error_any(&config);
        }

        for overlay in [
            "tools:\n  client_mcp:\n    port: 0\n",
            "mcp:\n  http:\n    max_sessions: 0\n",
            "tests:\n  execution_timeout_seconds: 0\n",
            "tests:\n  yaxunit:\n    timeouts:\n      total_ms: 0\n",
        ] {
            assert_schema_invalid(&local_config_schema_json(), overlay);
            assert_overlay_loader_error(overlay);
        }
    }

    #[test]
    fn schemas_and_loader_accept_supported_runtime_sections() {
        let config = format!(
            "{}execution_timeout: 300000\nbuild:\n  partialLoadThreshold: 20\ntools:\n  client_mcp:\n    port: 9874\n  edt_cli:\n    startup_timeout_ms: 300000\n    command_timeout_ms: 300000\nmcp:\n  http:\n    bind_address: '127.0.0.1:3000'\n    path: /mcp\n    stateful_sessions: true\n    max_sessions: 64\n    idle_ttl_secs: 900\n  execution:\n    max_concurrent_calls: 1\n    shutdown_grace_period_secs: 30\ntests:\n  execution_timeout_seconds: 300\n  yaxunit:\n    timeouts:\n      startup_ms: 300000\n      run_ms: 300000\n      total_ms: 300000\n  va:\n    fail_fast: false\n    timeouts:\n      startup_ms: 300000\n      run_ms: 300000\n      total_ms: 300000\n",
            minimal_project_config_without_base_path()
        );
        assert_schema_valid(&main_config_schema_json(), &config);
        assert_config_loader_ok(&config);

        let overlay = "workPath: local-work\ninfobase:\n  user: Admin\n  password: secret\ntools:\n  client_mcp:\n    port: 9874\nmcp:\n  http:\n    max_sessions: 64\n  execution:\n    max_concurrent_calls: 1\ntests:\n  execution_timeout_seconds: 300\n";
        assert_schema_valid(&local_config_schema_json(), overlay);
        assert_overlay_loader_ok(overlay);
    }

    #[test]
    fn schemas_and_loader_reject_null_for_defaulted_non_optional_fields() {
        for config in [
            minimal_project_config_with_format_null(),
            format!(
                "{}build:\n  partialLoadThreshold: null\n",
                minimal_project_config_without_base_path()
            ),
            format!(
                "{}tools: null\n",
                minimal_project_config_without_base_path()
            ),
            format!(
                "{}tools:\n  edt_cli: null\n",
                minimal_project_config_without_base_path()
            ),
            format!(
                "{}tools:\n  enterprise:\n    additional-launch-keys: null\n",
                minimal_project_config_without_base_path()
            ),
            format!(
                "{}tools:\n  edt_cli:\n    startup_timeout_ms: null\n",
                minimal_project_config_without_base_path()
            ),
        ] {
            assert_schema_invalid(&main_config_schema_json(), &config);
            assert_config_loader_error_any(&config);
        }

        for overlay in [
            "tools: null\n",
            "tools:\n  edt_cli: null\n",
            "tools:\n  enterprise:\n    additional-launch-keys: null\n",
            "tools:\n  edt_cli:\n    command_timeout_ms: null\n",
            "tools:\n  edt_cli:\n    interactive-mode: null\n",
        ] {
            assert_schema_invalid(&local_config_schema_json(), overlay);
            assert_overlay_loader_error(overlay);
        }
    }

    #[test]
    fn main_schema_and_loader_reject_unknown_keys() {
        for config in [
            format!(
                "{}toolz: {{}}\n",
                minimal_project_config_without_base_path()
            ),
            format!(
                "{}basePath: /tmp/project\n",
                minimal_project_config_without_base_path()
            ),
            format!(
                "{}tools:\n  platform:\n    typo: value\n",
                minimal_project_config_without_base_path()
            ),
        ] {
            assert_schema_invalid(&main_config_schema_json(), &config);
            assert_config_loader_error_any(&config);
        }
    }

    fn minimal_project_config_without_base_path() -> String {
        "workPath: build\nformat: DESIGNER\nbuilder: DESIGNER\ninfobase:\n  connection: 'File=build/ib'\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: .\n".to_owned()
    }

    fn minimal_project_config_with_format_null() -> String {
        "workPath: build\nformat: null\nbuilder: DESIGNER\ninfobase:\n  connection: 'File=build/ib'\nsource-set:\n  - name: main\n    type: CONFIGURATION\n    path: .\n".to_owned()
    }

    fn assert_schema_valid(schema: &serde_json::Value, yaml: &str) {
        let instance = yaml_to_json(yaml);
        let validator = jsonschema::validator_for(schema).expect("schema validator");
        if let Err(error) = validator.validate(&instance) {
            panic!("expected schema-valid YAML, got {error}");
        }
    }

    fn assert_schema_invalid(schema: &serde_json::Value, yaml: &str) {
        let instance = yaml_to_json(yaml);
        let validator = jsonschema::validator_for(schema).expect("schema validator");
        assert!(
            validator.validate(&instance).is_err(),
            "expected schema-invalid YAML:\n{yaml}"
        );
    }

    fn yaml_to_json(yaml: &str) -> serde_json::Value {
        let value: serde_yaml::Value = serde_yaml::from_str(yaml).expect("yaml");
        serde_json::to_value(value).expect("json")
    }

    fn assert_overlay_loader_error(overlay: &str) {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("Configuration.xml"), "<Configuration/>").expect("xml");
        let config_path = dir.path().join("v8project.yaml");
        std::fs::write(&config_path, minimal_project_config_without_base_path()).expect("config");
        std::fs::write(dir.path().join("v8project.local.yaml"), overlay).expect("overlay");

        load_config(config_path.to_str(), None).expect_err("overlay must be rejected");
    }

    fn assert_overlay_loader_ok(overlay: &str) {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("Configuration.xml"), "<Configuration/>").expect("xml");
        let config_path = dir.path().join("v8project.yaml");
        std::fs::write(&config_path, minimal_project_config_without_base_path()).expect("config");
        std::fs::write(dir.path().join("v8project.local.yaml"), overlay).expect("overlay");

        load_config(config_path.to_str(), None).expect("overlay must be accepted");
    }

    fn assert_config_loader_ok(config: &str) {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("Configuration.xml"), "<Configuration/>").expect("xml");
        let config_path = dir.path().join("v8project.yaml");
        std::fs::write(&config_path, config).expect("config");

        load_config(config_path.to_str(), None).expect("config must be accepted");
    }

    fn assert_config_loader_error_any(config: &str) {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("Configuration.xml"), "<Configuration/>").expect("xml");
        let config_path = dir.path().join("v8project.yaml");
        std::fs::write(&config_path, config).expect("config");

        load_config(config_path.to_str(), None).expect_err("config must be rejected");
    }

    fn assert_config_loader_error(config: &str, expected: &str) {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("Configuration.xml"), "<Configuration/>").expect("xml");
        let config_path = dir.path().join("v8project.yaml");
        std::fs::write(&config_path, config).expect("config");

        let error = load_config(config_path.to_str(), None).expect_err("config must be rejected");
        assert!(
            error.to_string().contains(expected),
            "expected error to contain {expected:?}, got {error}"
        );
    }

    fn assert_no_object_key(value: &serde_json::Value, forbidden: &str) {
        match value {
            serde_json::Value::Object(object) => {
                assert!(
                    !object.contains_key(forbidden),
                    "schema must not contain alias property {forbidden:?}"
                );
                for value in object.values() {
                    assert_no_object_key(value, forbidden);
                }
            }
            serde_json::Value::Array(items) => {
                for value in items {
                    assert_no_object_key(value, forbidden);
                }
            }
            _ => {}
        }
    }

    fn assert_property_description_contains(
        schema: &serde_json::Value,
        def_path: &[&str],
        property: &str,
        expected: &str,
    ) {
        let description = schema_property(schema, def_path, property)
            .and_then(|property| property.get("description"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| {
                panic!(
                    "schema property {}{} lacks description",
                    if def_path.is_empty() {
                        String::new()
                    } else {
                        format!("$defs.{}.", def_path.join("."))
                    },
                    property
                )
            });
        assert!(
            description.contains(expected),
            "schema property {property:?} description {description:?} must contain {expected:?}"
        );
    }

    fn schema_property<'a>(
        schema: &'a serde_json::Value,
        def_path: &[&str],
        property: &str,
    ) -> Option<&'a serde_json::Value> {
        let mut object = schema.as_object()?;
        for def in def_path {
            object = object.get("$defs")?.as_object()?.get(*def)?.as_object()?;
        }
        object.get("properties")?.as_object()?.get(property)
    }
}
