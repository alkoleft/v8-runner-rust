use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use uuid::Uuid;

use crate::config::model::{AppConfig, VanessaProfileConfig};
use crate::domain::runner::LaunchOptions;
use crate::platform::enterprise::normalize_launch_payload_path;
use crate::support::error::AppError;
use crate::support::path::is_safe_path_segment;

pub(crate) struct VanessaLaunch {
    pub(crate) epf_path: PathBuf,
    pub(crate) params_path: PathBuf,
}

pub(crate) struct VanessaTestArtifacts<'a> {
    pub(crate) run_dir: &'a Path,
    pub(crate) junit_dir: &'a Path,
    pub(crate) runner_log: &'a Path,
}

pub(crate) fn prepare_test_launch(
    config: &AppConfig,
    profile_name: &str,
    artifacts: VanessaTestArtifacts<'_>,
) -> Result<VanessaLaunch, AppError> {
    let va = &config.tests.va;
    let (epf_path, params_template_path) = resolve_vanessa_paths(config)?;
    let profile = resolve_profile(va, profile_name)?;

    fs::create_dir_all(artifacts.junit_dir)
        .map_err(|error| AppError::Runtime(format!("failed to create JUnit directory: {error}")))?;

    let runtime_params_path = artifacts.run_dir.join("va-params.json");
    validate_params_payload_path(&runtime_params_path, "test va")?;

    let mut payload = read_params_template(params_template_path)?;
    let object = payload
        .as_object_mut()
        .ok_or_else(|| AppError::Runtime("Vanessa params JSON must be an object".to_owned()))?;
    apply_workspace_root_overlay(object, &config.base_path);
    apply_profile_overlay(object, profile, va.fail_fast);
    apply_test_overlay(object, artifacts);
    write_params_file(&runtime_params_path, &payload)
        .map_err(|error| AppError::Runtime(format!("failed to write Vanessa params: {error}")))?;

    Ok(VanessaLaunch {
        epf_path,
        params_path: runtime_params_path,
    })
}

pub(crate) fn prepare_client_mcp_launch(config: &AppConfig) -> Result<VanessaLaunch, AppError> {
    let va = &config.tests.va;
    let (epf_path, params_template_path) = resolve_vanessa_paths(config)?;
    let profile_name = va
        .profile
        .as_deref()
        .ok_or_else(|| AppError::Validation("tests.va.profile is not configured".to_owned()))?;
    let profile = resolve_profile(va, profile_name)?;

    let run_dir = config
        .work_path
        .join("temp")
        .join("client-mcp")
        .join("va")
        .join(format!(
            "{}-{}-{}",
            chrono::Utc::now().timestamp_millis(),
            std::process::id(),
            Uuid::new_v4().simple()
        ));
    fs::create_dir_all(&run_dir).map_err(|error| {
        AppError::Runtime(format!(
            "failed to create Vanessa params directory: {error}"
        ))
    })?;
    set_dir_permissions(&run_dir).map_err(|error| {
        AppError::Runtime(format!("failed to chmod Vanessa params directory: {error}"))
    })?;

    let runtime_params_path = run_dir.join("va-params.json");
    validate_params_payload_path(&runtime_params_path, "launch mcp")?;

    let mut payload = read_params_template(params_template_path)?;
    let object = payload
        .as_object_mut()
        .ok_or_else(|| AppError::Runtime("Vanessa params JSON must be an object".to_owned()))?;
    apply_workspace_root_overlay(object, &config.base_path);
    apply_profile_overlay(object, profile, va.fail_fast);
    apply_logging_overlay(
        object,
        &run_dir.join("va-text.log"),
        &run_dir.join("va-status.log"),
    );
    write_params_file(&runtime_params_path, &payload)
        .map_err(|error| AppError::Runtime(format!("failed to write Vanessa params: {error}")))?;

    Ok(VanessaLaunch {
        epf_path,
        params_path: runtime_params_path,
    })
}

pub(crate) fn apply_test_player_launch(
    base: &LaunchOptions,
    launch: &VanessaLaunch,
) -> LaunchOptions {
    let mut options = base.clone();
    options.execute = Some(normalize_launch_payload_path(&launch.epf_path));
    options.c = Some(format!(
        "StartFeaturePlayer;VAParams={}",
        normalize_launch_payload_path(&launch.params_path)
    ));
    options
}

pub(crate) fn apply_client_mcp_launch(
    launch: &mut LaunchOptions,
    payload: &mut String,
    va: &VanessaLaunch,
) {
    launch.execute = Some(normalize_launch_payload_path(&va.epf_path));
    payload.push_str(&format!(
        ";VAParams={}",
        normalize_launch_payload_path(&va.params_path)
    ));
}

fn resolve_vanessa_paths(config: &AppConfig) -> Result<(PathBuf, &Path), AppError> {
    let epf_path =
        config.tools.va.epf_path.clone().ok_or_else(|| {
            AppError::Validation("tools.va.epf_path is not configured".to_owned())
        })?;
    let params_path =
        config.tests.va.params_path.as_deref().ok_or_else(|| {
            AppError::Validation("tests.va.params_path is not configured".to_owned())
        })?;
    Ok((epf_path, params_path))
}

fn resolve_profile<'a>(
    va: &'a crate::config::model::VanessaTestConfig,
    profile_name: &str,
) -> Result<&'a VanessaProfileConfig, AppError> {
    if !is_safe_path_segment(profile_name) {
        return Err(AppError::Validation(format!(
            "tests.va.profile contains unsafe path characters: {profile_name}"
        )));
    }
    va.profiles.get(profile_name).ok_or_else(|| {
        AppError::Validation(format!(
            "unknown Vanessa Automation profile '{profile_name}'"
        ))
    })
}

fn validate_params_payload_path(path: &Path, command: &str) -> Result<(), AppError> {
    let payload_path = normalize_launch_payload_path(path);
    if payload_path.contains(';') {
        return Err(AppError::Validation(format!(
            "generated Vanessa params path for {command} must not contain ';' because the /C payload is semicolon-delimited"
        )));
    }
    Ok(())
}

fn read_params_template(path: &Path) -> Result<Value, AppError> {
    let base = fs::read_to_string(path).map_err(|error| {
        AppError::Runtime(format!("failed to read Vanessa params template: {error}"))
    })?;
    serde_json::from_str(&base)
        .map_err(|error| AppError::Runtime(format!("failed to parse Vanessa params JSON: {error}")))
}

fn apply_profile_overlay(
    object: &mut Map<String, Value>,
    profile: &VanessaProfileConfig,
    fail_fast: bool,
) {
    object.insert(
        "ОстановкаПриВозникновенииОшибки".to_owned(),
        Value::Bool(fail_fast),
    );
    if let Some(feature_path) = profile.feature_path.as_ref() {
        object.insert(
            "КаталогФич".to_owned(),
            Value::String(feature_path.display().to_string()),
        );
    }
    insert_string_array_if_non_empty(object, "СписокФичДляВыполнения", &profile.features_to_run);
    insert_normalized_tag_array_if_non_empty(object, "СписокТеговОтбор", &profile.filter_tags);
    insert_normalized_tag_array_if_non_empty(object, "СписокТеговИсключение", &profile.ignore_tags);
    insert_string_array_if_non_empty(
        object,
        "СписокСценариевДляВыполнения",
        &profile.scenario_filter,
    );
}

fn apply_workspace_root_overlay(object: &mut Map<String, Value>, base_path: &Path) {
    if object
        .get("WorkspaceRoot")
        .is_some_and(|value| !value.is_null())
    {
        return;
    }
    object.insert(
        "WorkspaceRoot".to_owned(),
        Value::String(base_path.display().to_string()),
    );
}

fn apply_test_overlay(object: &mut Map<String, Value>, artifacts: VanessaTestArtifacts<'_>) {
    object.insert("ВыполнитьСценарии".to_owned(), Value::Bool(true));
    object.insert("ЗавершитьРаботуСистемы".to_owned(), Value::Bool(true));
    object.insert(
        "ЗакрытьTestClientПослеЗапускаСценариев".to_owned(),
        Value::Bool(true),
    );
    object.insert(
        "ЗакрыватьКлиентТестированияПринудительно".to_owned(),
        Value::Bool(true),
    );
    object.insert("ДелатьОтчетВФорматеjUnit".to_owned(), Value::Bool(true));
    let junit_dir = Value::String(artifacts.junit_dir.display().to_string());
    object.insert(
        "КаталогВыгрузкиJUnit".to_owned(),
        junit_dir.clone(),
    );
    ensure_object(object, "ОтчетJUnit").insert("КаталогВыгрузкиJUnit".to_owned(), junit_dir);
    apply_logging_overlay(
        object,
        artifacts.runner_log,
        &artifacts.run_dir.join("va-status.log"),
    );
}

fn ensure_object<'a>(object: &'a mut Map<String, Value>, key: &str) -> &'a mut Map<String, Value> {
    if !object.get(key).is_some_and(Value::is_object) {
        object.insert(key.to_owned(), Value::Object(Map::new()));
    }
    object
        .get_mut(key)
        .and_then(Value::as_object_mut)
        .expect("object value was just inserted")
}

fn apply_logging_overlay(
    object: &mut Map<String, Value>,
    text_log_path: &Path,
    status_path: &Path,
) {
    object.insert(
        "ДелатьЛогВыполненияСценариевВТекстовыйФайл".to_owned(),
        Value::Bool(true),
    );
    object.insert("ВыводитьВЛогВыполнениеШагов".to_owned(), Value::Bool(true));
    object.insert(
        "ПодробныйЛогВыполненияСценариев".to_owned(),
        Value::Number(1.into()),
    );
    object.insert(
        "ВыгружатьСтатусВыполненияСценариевВФайл".to_owned(),
        Value::Bool(true),
    );
    object.insert(
        "ПутьКФайлуДляВыгрузкиСтатусаВыполненияСценариев".to_owned(),
        Value::String(status_path.display().to_string()),
    );
    object.insert(
        "ИмяФайлаЛогВыполненияСценариев".to_owned(),
        Value::String(text_log_path.display().to_string()),
    );
}

fn insert_string_array_if_non_empty(object: &mut Map<String, Value>, key: &str, values: &[String]) {
    if values.is_empty() {
        object.remove(key);
        return;
    }
    object.insert(
        key.to_owned(),
        Value::Array(values.iter().cloned().map(Value::String).collect()),
    );
}

fn insert_normalized_tag_array_if_non_empty(
    object: &mut Map<String, Value>,
    key: &str,
    values: &[String],
) {
    if values.is_empty() {
        object.remove(key);
        return;
    }
    object.insert(
        key.to_owned(),
        Value::Array(
            values
                .iter()
                .map(|value| Value::String(value.strip_prefix('@').unwrap_or(value).to_owned()))
                .collect(),
        ),
    );
}

fn write_params_file(path: &Path, payload: &Value) -> std::io::Result<()> {
    let payload = serde_json::to_vec_pretty(payload)?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(&payload)?;
    set_file_permissions(path)?;
    Ok(())
}

fn set_dir_permissions(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

fn set_file_permissions(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}
