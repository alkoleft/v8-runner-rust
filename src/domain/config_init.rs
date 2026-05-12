use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigInitResult {
    pub ok: bool,
    pub path: String,
    pub local_path: String,
    pub gitignore_path: String,
    pub format: String,
    pub builder: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform_version: Option<String>,
    pub source_sets: Vec<ConfigInitSourceSet>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    pub overwritten: bool,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigInitSourceSet {
    pub name: String,
    #[serde(rename = "type")]
    pub source_type: String,
    pub path: String,
}
