use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Cursor};
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

use serde::Deserialize;
use tracing::debug;
use zip::ZipArchive;

use crate::config::loader::LOCAL_CONFIG_FILE_NAME;
use crate::config::model::{AppConfig, BuilderBackend};
use crate::domain::tools_download::{
    ToolDownloadDestination, ToolDownloadTarget, ToolExtensionInstallMode, ToolsDownloadResult,
};
use crate::platform::download;
use crate::support::error::AppError;
use crate::support::fs::{
    ensure_dir, publish_file_atomically, remove_path_if_exists, replace_dir_atomically,
};
use crate::use_cases::context::ExecutionContext;
use crate::use_cases::request::ToolsDownloadRequest;
use crate::use_cases::result::{UseCaseFailure, UseCaseResult};

const LOCAL_CONFIG_SCHEMA_MODEL_LINE: &str = "# yaml-language-server: $schema=https://raw.githubusercontent.com/alkoleft/v8-runner-rust/master/docs/schemas/v8project.local.schema.json";

const YAXUNIT_REPO: &str = "bia-technologies/yaxunit";
const VANESSA_REPO: &str = "Pr-Mex/vanessa-automation-single";
const CLIENT_MCP_REPO: &str = "1c-neurofish/onec-client-mcp-devkit";

const YAXUNIT_SOURCE_PREFIX: &str = "exts/yaxunit/";
const CLIENT_MCP_SOURCE_PREFIX: &str = "exts/client-mcp/";
const DOWNLOAD_MARKER_FILE: &str = ".v8-runner-tools-download.json";

pub fn execute(
    context: &ExecutionContext,
    config: &AppConfig,
    request: &ToolsDownloadRequest,
) -> UseCaseResult<ToolsDownloadResult> {
    tools_download(context, config, request).map_err(|error| UseCaseFailure::without_payload(error))
}

fn tools_download(
    context: &ExecutionContext,
    config: &AppConfig,
    request: &ToolsDownloadRequest,
) -> Result<ToolsDownloadResult, AppError> {
    let started = Instant::now();
    let config_path = request.config_path.clone();
    let config_dir = config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let local_config_path = config_dir.join(LOCAL_CONFIG_FILE_NAME);
    let tools_dir = config.base_path.join("build").join("tools");

    ensure_dir(&tools_dir).map_err(|error| {
        AppError::Runtime(format!(
            "failed to create tools directory '{}': {error}",
            tools_dir.display()
        ))
    })?;

    let destinations = match request.target {
        ToolDownloadTarget::Yaxunit => download_yaxunit(
            context,
            config,
            &tools_dir,
            request.extensions,
            request.force,
            &config_path,
        )?,
        ToolDownloadTarget::VanessaAutomationSingle => {
            download_vanessa(context, &tools_dir, request.force)?
        }
        ToolDownloadTarget::ClientMcp => download_client_mcp(
            context,
            config,
            &tools_dir,
            request.extensions,
            request.force,
        )?,
    };

    update_config_for_download(
        context,
        &config_path,
        &local_config_path,
        request.target,
        request.extensions,
        &destinations,
    )?;

    Ok(ToolsDownloadResult {
        ok: true,
        tool: target_label(request.target).to_owned(),
        mode: download_mode_label(request.target, request.extensions).to_owned(),
        destinations,
        config_path,
        local_config_path,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

fn download_yaxunit(
    context: &ExecutionContext,
    config: &AppConfig,
    tools_dir: &Path,
    mode: ToolExtensionInstallMode,
    force: bool,
    config_path: &Path,
) -> Result<Vec<ToolDownloadDestination>, AppError> {
    let release = fetch_latest_release(context, YAXUNIT_REPO)?;
    match mode {
        ToolExtensionInstallMode::Sources => {
            validate_yaxunit_source_set_config(config_path)?;
            let path = config.base_path.join("tests");
            let marker_path = config
                .base_path
                .join("build")
                .join(format!(".tests{DOWNLOAD_MARKER_FILE}"));
            download_source_subdir(
                context,
                &release,
                YAXUNIT_SOURCE_PREFIX,
                &path,
                &marker_path,
                force,
            )?;
            Ok(vec![destination(
                "yaxunit",
                &release,
                path,
                "source-set tests",
            )])
        }
        ToolExtensionInstallMode::Artifacts => {
            let asset = release.required_asset("YAxUnit", ".cfe")?;
            let path = tools_dir.join(&asset.name);
            download_asset_file(context, asset, &path, force)?;
            Ok(vec![destination("yaxunit", &release, path, "artifact")])
        }
    }
}

fn download_vanessa(
    context: &ExecutionContext,
    tools_dir: &Path,
    force: bool,
) -> Result<Vec<ToolDownloadDestination>, AppError> {
    let release = fetch_latest_release(context, VANESSA_REPO)?;
    let asset = release.required_asset("vanessa-automation-single", ".zip")?;
    let path = tools_dir.join("vanessa-automation-single.epf");
    download_single_file_from_zip(
        context,
        asset,
        "vanessa-automation-single.epf",
        &path,
        force,
    )?;
    Ok(vec![destination(
        "vanessa-automation-single",
        &release,
        path,
        "tools.va.epf_path",
    )])
}

fn download_client_mcp(
    context: &ExecutionContext,
    config: &AppConfig,
    tools_dir: &Path,
    mode: ToolExtensionInstallMode,
    force: bool,
) -> Result<Vec<ToolDownloadDestination>, AppError> {
    if mode == ToolExtensionInstallMode::Artifacts && config.builder != BuilderBackend::Designer {
        return Err(AppError::Validation(
            "`tools download client-mcp` requires builder=DESIGNER because client_mcp.cfe is registered as a tool extension artifact; use `tools download client-mcp --sources` for builder=IBCMD"
                .to_owned(),
        ));
    }

    let release = fetch_latest_release(context, CLIENT_MCP_REPO)?;
    match mode {
        ToolExtensionInstallMode::Sources => {
            let path = tools_dir
                .join("onec-client-mcp-devkit")
                .join("exts")
                .join("client-mcp");
            let marker_path = source_download_marker_path(&path);
            download_source_subdir(
                context,
                &release,
                CLIENT_MCP_SOURCE_PREFIX,
                &path,
                &marker_path,
                force,
            )?;
            Ok(vec![destination(
                "onec-client-mcp-devkit",
                &release,
                path,
                "tools.client_mcp.extension.source",
            )])
        }
        ToolExtensionInstallMode::Artifacts => {
            let asset = release.required_asset("client_mcp", ".cfe")?;
            let path = tools_dir.join(&asset.name);
            download_asset_file(context, asset, &path, force)?;
            Ok(vec![destination(
                "onec-client-mcp-devkit",
                &release,
                path,
                "tools.client_mcp.extension.artifact",
            )])
        }
    }
}

fn fetch_latest_release(context: &ExecutionContext, repo: &str) -> Result<GitHubRelease, AppError> {
    let base = release_base_url();
    let url = format!("{base}/repos/{repo}/releases/latest");
    debug!(repo, url = %url, "fetching latest tool release");
    let cancellation = context.cancellation();
    let text =
        download::get_text(&url, context.remaining_budget(), &cancellation).map_err(|error| {
            AppError::Runtime(format!("failed to fetch latest release {repo}: {error}"))
        })?;
    serde_json::from_str::<GitHubRelease>(&text).map_err(|error| {
        AppError::Runtime(format!("failed to parse latest release {repo}: {error}"))
    })
}

fn release_base_url() -> String {
    std::env::var("V8TR_GITHUB_API_BASE_URL")
        .unwrap_or_else(|_| "https://api.github.com".to_owned())
        .trim_end_matches('/')
        .to_owned()
}

fn download_asset_file(
    context: &ExecutionContext,
    asset: &GitHubAsset,
    target_path: &Path,
    force: bool,
) -> Result<(), AppError> {
    if !should_download_file(target_path, force)? {
        return Ok(());
    }
    debug!(
        asset = %asset.name,
        url = %asset.browser_download_url,
        path = %target_path.display(),
        "downloading tool asset"
    );
    let cancellation = context.cancellation();
    let bytes = download::get_bytes(
        &asset.browser_download_url,
        context.remaining_budget(),
        &cancellation,
    )
    .map_err(|error| {
        AppError::Runtime(format!(
            "failed to download asset '{}': {error}",
            asset.name
        ))
    })?;
    publish_file_bytes_with_marker(context, &bytes, target_path)
}

fn download_single_file_from_zip(
    context: &ExecutionContext,
    asset: &GitHubAsset,
    file_name: &str,
    target_path: &Path,
    force: bool,
) -> Result<(), AppError> {
    if !should_download_file(target_path, force)? {
        return Ok(());
    }
    debug!(
        asset = %asset.name,
        url = %asset.browser_download_url,
        file_name,
        path = %target_path.display(),
        "downloading tool archive asset"
    );
    let cancellation = context.cancellation();
    let bytes = download::get_bytes(
        &asset.browser_download_url,
        context.remaining_budget(),
        &cancellation,
    )
    .map_err(|error| {
        AppError::Runtime(format!(
            "failed to download asset '{}': {error}",
            asset.name
        ))
    })?;
    let file = find_file_in_zip(&bytes, file_name)?;
    publish_file_bytes_with_marker(context, &file, target_path)
}

fn download_source_subdir(
    context: &ExecutionContext,
    release: &GitHubRelease,
    source_prefix: &str,
    target_path: &Path,
    marker_path: &Path,
    force: bool,
) -> Result<(), AppError> {
    if !should_download_source_dir(target_path, marker_path, force)? {
        return Ok(());
    }
    let archive_url = source_archive_url(release);
    debug!(
        tag = %release.tag_name,
        url = %archive_url,
        source_prefix,
        path = %target_path.display(),
        "downloading tool source archive"
    );
    let cancellation = context.cancellation();
    let bytes = download::get_bytes(&archive_url, context.remaining_budget(), &cancellation)
        .map_err(|error| {
            AppError::Runtime(format!(
                "failed to download source archive '{}': {error}",
                archive_url
            ))
        })?;
    let staged = target_path.with_extension(format!(
        "download-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    if staged.exists() {
        fs::remove_dir_all(&staged).map_err(io_error("failed to cleanup stale staged dir"))?;
    }
    ensure_dir(&staged).map_err(io_error("failed to create staged source dir"))?;
    extract_zip_subdir(&bytes, source_prefix, &staged).inspect_err(|_| {
        let _ = fs::remove_dir_all(&staged);
    })?;

    let marker_existed = marker_path.exists();
    write_source_download_marker(target_path, marker_path)?;
    let publish_phase = context.run_no_process_critical_phase(|| {
        replace_dir_atomically(
            &staged,
            target_path,
            &chrono::Utc::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
                .to_string(),
            "tools-download",
            ".tools-download-backup",
        )
    });
    match publish_phase {
        Ok(_) => Ok(()),
        Err(error) => {
            let _ = fs::remove_dir_all(&staged);
            if !marker_existed {
                let _ = remove_path_if_exists(marker_path);
            }
            Err(AppError::Runtime(format!(
                "failed to publish source directory '{}': {error}",
                target_path.display()
            )))
        }
    }
}

fn find_file_in_zip(bytes: &[u8], file_name: &str) -> Result<Vec<u8>, AppError> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| AppError::Runtime(format!("failed to read zip archive: {error}")))?;
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|error| AppError::Runtime(format!("failed to read zip entry: {error}")))?;
        if !file.is_file() {
            continue;
        }
        let Some(name) = Path::new(file.name())
            .file_name()
            .and_then(|name| name.to_str())
        else {
            continue;
        };
        if name == file_name {
            let mut bytes = Vec::new();
            io::copy(&mut file, &mut bytes).map_err(|error| {
                AppError::Runtime(format!("failed to extract zip entry: {error}"))
            })?;
            return Ok(bytes);
        }
    }
    Err(AppError::Runtime(format!(
        "zip archive does not contain {file_name}"
    )))
}

fn extract_zip_subdir(
    bytes: &[u8],
    source_prefix: &str,
    target_path: &Path,
) -> Result<(), AppError> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| AppError::Runtime(format!("failed to read zip archive: {error}")))?;
    let mut extracted = false;
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|error| AppError::Runtime(format!("failed to read zip entry: {error}")))?;
        let Some(relative) = zip_relative_path(file.name(), source_prefix) else {
            continue;
        };
        if relative.as_os_str().is_empty() {
            continue;
        }
        let target = target_path.join(relative);
        if file.is_dir() {
            ensure_dir(&target).map_err(io_error("failed to create extracted dir"))?;
        } else {
            if let Some(parent) = target.parent() {
                ensure_dir(parent).map_err(io_error("failed to create extracted parent dir"))?;
            }
            let mut output =
                File::create(&target).map_err(io_error("failed to create extracted file"))?;
            io::copy(&mut file, &mut output).map_err(io_error("failed to extract zip file"))?;
            extracted = true;
        }
    }
    if !extracted {
        return Err(AppError::Runtime(format!(
            "source archive does not contain {source_prefix}"
        )));
    }
    Ok(())
}

fn zip_relative_path(name: &str, source_prefix: &str) -> Option<PathBuf> {
    let mut parts = name.splitn(2, '/');
    let _root = parts.next()?;
    let inner = parts.next().unwrap_or_default();
    let relative = inner.strip_prefix(source_prefix)?;
    if relative.contains('\\') {
        return None;
    }
    let mut safe = PathBuf::new();
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(value) => safe.push(value),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(safe)
}

fn publish_bytes(
    context: &ExecutionContext,
    bytes: &[u8],
    target_path: &Path,
) -> Result<(), AppError> {
    if let Some(parent) = target_path.parent() {
        ensure_dir(parent).map_err(io_error("failed to create target parent dir"))?;
    }
    let staged = target_path.with_extension(format!(
        "download-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    fs::write(&staged, bytes).map_err(io_error("failed to write staged file"))?;
    let publish_phase =
        context.run_no_process_critical_phase(|| publish_file_atomically(&staged, target_path));
    match publish_phase {
        Ok(_) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&staged);
            Err(AppError::Runtime(format!(
                "failed to publish downloaded file '{}': {error}",
                target_path.display()
            )))
        }
    }
}

fn publish_file_bytes_with_marker(
    context: &ExecutionContext,
    bytes: &[u8],
    target_path: &Path,
) -> Result<(), AppError> {
    let marker_path = file_download_marker_path(target_path);
    let marker_existed = marker_path.exists();
    write_file_download_marker(target_path)?;
    match publish_bytes(context, bytes, target_path) {
        Ok(()) => Ok(()),
        Err(error) => {
            if !marker_existed {
                let _ = remove_path_if_exists(&marker_path);
            }
            Err(error)
        }
    }
}

fn should_download_file(path: &Path, force: bool) -> Result<bool, AppError> {
    if path.exists() && path.is_dir() {
        return Err(AppError::Validation(format!(
            "download target is a directory: {}",
            path.display()
        )));
    }
    if !path.exists() {
        return Ok(true);
    }
    if force && !file_download_marker_path(path).exists() {
        return Err(AppError::Validation(format!(
            "download target already exists and is not managed by v8-runner: {}",
            path.display()
        )));
    }
    Ok(force)
}

fn should_download_source_dir(
    path: &Path,
    marker_path: &Path,
    force: bool,
) -> Result<bool, AppError> {
    if !path.exists() {
        return Ok(true);
    }
    if !path.is_dir() {
        return Err(AppError::Validation(format!(
            "download target is not a directory: {}",
            path.display()
        )));
    }
    if !marker_path.exists() {
        return Err(AppError::Validation(format!(
            "download target already exists and is not managed by v8-runner: {}",
            path.display()
        )));
    }
    Ok(force)
}

fn write_source_download_marker(target_path: &Path, marker_path: &Path) -> Result<(), AppError> {
    let parent = marker_path.parent().ok_or_else(|| {
        AppError::Runtime(format!(
            "download marker path has no parent: {}",
            marker_path.display()
        ))
    })?;
    ensure_dir(parent).map_err(io_error("failed to create download marker parent"))?;
    fs::write(
        &marker_path,
        format!(
            "{{\n  \"tool\": \"v8-runner\",\n  \"target\": \"{}\"\n}}\n",
            target_path.display()
        ),
    )
    .map_err(io_error("failed to write download marker"))
}

fn write_file_download_marker(target_path: &Path) -> Result<(), AppError> {
    let marker_path = file_download_marker_path(target_path);
    let parent = marker_path.parent().ok_or_else(|| {
        AppError::Runtime(format!(
            "download marker path has no parent: {}",
            marker_path.display()
        ))
    })?;
    ensure_dir(parent).map_err(io_error("failed to create download marker parent"))?;
    fs::write(
        &marker_path,
        format!(
            "{{\n  \"tool\": \"v8-runner\",\n  \"target\": \"{}\"\n}}\n",
            target_path.display()
        ),
    )
    .map_err(io_error("failed to write download marker"))
}

fn source_download_marker_path(path: &Path) -> PathBuf {
    sidecar_download_marker_path(path)
}

fn file_download_marker_path(path: &Path) -> PathBuf {
    sidecar_download_marker_path(path)
}

fn sidecar_download_marker_path(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "download".to_owned());
    parent.join(format!(".{name}{DOWNLOAD_MARKER_FILE}"))
}

fn relative_path(root: &Path, path: &Path) -> String {
    if let Some(relative) = path
        .strip_prefix(root)
        .ok()
        .filter(|relative| !relative.as_os_str().is_empty())
    {
        return relative.display().to_string();
    }

    let root_components = normalized_components(root);
    let path_components = normalized_components(path);
    let common_len = root_components
        .iter()
        .zip(path_components.iter())
        .take_while(|(left, right)| left == right)
        .count();

    let mut relative = PathBuf::new();
    for _ in common_len..root_components.len() {
        relative.push("..");
    }
    for component in &path_components[common_len..] {
        relative.push(component);
    }

    if relative.as_os_str().is_empty() {
        ".".to_owned()
    } else {
        relative.display().to_string()
    }
}

fn normalized_components(path: &Path) -> Vec<OsString> {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => components.push(prefix.as_os_str().to_os_string()),
            Component::RootDir | Component::CurDir => {}
            Component::ParentDir => match components.last() {
                Some(last) if last != ".." => {
                    components.pop();
                }
                _ => components.push(OsString::from("..")),
            },
            Component::Normal(part) => components.push(part.to_os_string()),
        }
    }
    components
}

fn update_config_for_download(
    context: &ExecutionContext,
    config_path: &Path,
    local_config_path: &Path,
    target: ToolDownloadTarget,
    mode: ToolExtensionInstallMode,
    destinations: &[ToolDownloadDestination],
) -> Result<(), AppError> {
    match target {
        ToolDownloadTarget::Yaxunit if mode == ToolExtensionInstallMode::Sources => {
            add_yaxunit_source_set(context, config_path)?;
        }
        ToolDownloadTarget::Yaxunit => {}
        ToolDownloadTarget::VanessaAutomationSingle => {
            let local_overlay = render_vanessa_local_overlay(local_config_path, destinations)?;
            publish_bytes(context, local_overlay.as_bytes(), local_config_path)?;
        }
        ToolDownloadTarget::ClientMcp => {
            let local_overlay =
                render_client_mcp_local_overlay(local_config_path, destinations, mode)?;
            publish_bytes(context, local_overlay.as_bytes(), local_config_path)?;
        }
    }
    Ok(())
}

fn add_yaxunit_source_set(context: &ExecutionContext, config_path: &Path) -> Result<(), AppError> {
    let content = fs::read_to_string(config_path).map_err(io_error("failed to read config"))?;
    let root: serde_yaml::Value = serde_yaml::from_str(&content)
        .map_err(|error| AppError::Runtime(format!("failed to parse config YAML: {error}")))?;
    let mapping = root
        .as_mapping()
        .ok_or_else(|| AppError::Validation("expected a YAML mapping at config root".to_owned()))?;
    let key = serde_yaml::Value::String("source-set".to_owned());
    let source_sets = mapping
        .get(&key)
        .and_then(serde_yaml::Value::as_sequence)
        .ok_or_else(|| AppError::Validation("config must contain source-set list".to_owned()))?;
    if source_sets
        .iter()
        .any(|item| yaml_field_eq(item, "name", "tests"))
    {
        return Ok(());
    }

    let rendered = insert_yaxunit_source_set_text(&content)?;
    publish_bytes(context, rendered.as_bytes(), config_path)
}

fn validate_yaxunit_source_set_config(config_path: &Path) -> Result<(), AppError> {
    let content = fs::read_to_string(config_path).map_err(io_error("failed to read config"))?;
    let root: serde_yaml::Value = serde_yaml::from_str(&content)
        .map_err(|error| AppError::Runtime(format!("failed to parse config YAML: {error}")))?;
    let mapping = root
        .as_mapping()
        .ok_or_else(|| AppError::Validation("expected a YAML mapping at config root".to_owned()))?;
    let source_sets = mapping
        .get(serde_yaml::Value::String("source-set".to_owned()))
        .and_then(serde_yaml::Value::as_sequence)
        .ok_or_else(|| AppError::Validation("config must contain source-set list".to_owned()))?;
    for source_set in source_sets
        .iter()
        .filter(|item| yaml_field_eq(item, "name", "tests"))
    {
        let is_expected = yaml_field_eq(source_set, "type", "EXTENSION")
            && yaml_field_eq(source_set, "path", "tests");
        if !is_expected {
            return Err(AppError::Validation(
                "source-set 'tests' already exists but does not match tools download contract: expected type=EXTENSION and path=tests"
                    .to_owned(),
            ));
        }
    }
    Ok(())
}

fn insert_yaxunit_source_set_text(content: &str) -> Result<String, AppError> {
    let source_set_start = content
        .lines()
        .position(|line| line.trim_end() == "source-set:")
        .ok_or_else(|| {
            AppError::Validation(
                "config source-set list must use block style before tools download can update it"
                    .to_owned(),
            )
        })?;

    let mut insertion_offset = content.len();
    let mut offset = 0usize;
    for (index, line) in content.split_inclusive('\n').enumerate() {
        let line_start = offset;
        offset += line.len();
        if index <= source_set_start {
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let is_top_level = line
            .chars()
            .next()
            .is_some_and(|first| !first.is_whitespace());
        if is_top_level {
            insertion_offset = line_start;
            break;
        }
    }

    let mut rendered = String::with_capacity(content.len() + 64);
    rendered.push_str(&content[..insertion_offset]);
    if !rendered.ends_with('\n') {
        rendered.push('\n');
    }
    rendered.push_str("  - name: tests\n    type: EXTENSION\n    path: tests\n");
    rendered.push_str(&content[insertion_offset..]);
    Ok(rendered)
}

fn render_vanessa_local_overlay(
    path: &Path,
    destinations: &[ToolDownloadDestination],
) -> Result<String, AppError> {
    let mut root = read_local_overlay(path)?;
    let vanessa_path = destinations
        .iter()
        .find(|destination| destination.tool == "vanessa-automation-single")
        .ok_or_else(|| AppError::Runtime("missing Vanessa download destination".to_owned()))?
        .path
        .clone();
    let config_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let vanessa_path = relative_path(config_dir, &vanessa_path);

    let root_mapping = root.as_mapping_mut().ok_or_else(|| {
        AppError::Validation("expected a YAML mapping at local config root".to_owned())
    })?;
    let tools = ensure_mapping(root_mapping, "tools")?;
    let va = ensure_mapping(tools, "va")?;
    va.insert(
        serde_yaml::Value::String("epf_path".to_owned()),
        serde_yaml::Value::String(vanessa_path),
    );
    render_local_overlay(root)
}

fn render_client_mcp_local_overlay(
    path: &Path,
    destinations: &[ToolDownloadDestination],
    mode: ToolExtensionInstallMode,
) -> Result<String, AppError> {
    let mut root = read_local_overlay(path)?;
    let client_path = destinations
        .iter()
        .find(|destination| destination.tool == "onec-client-mcp-devkit")
        .ok_or_else(|| AppError::Runtime("missing client MCP download destination".to_owned()))?
        .path
        .clone();
    let config_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let client_path = relative_path(config_dir, &client_path);

    let root_mapping = root.as_mapping_mut().ok_or_else(|| {
        AppError::Validation("expected a YAML mapping at local config root".to_owned())
    })?;
    let tools = ensure_mapping(root_mapping, "tools")?;
    let client_mcp = ensure_mapping(tools, "client_mcp")?;
    let mut extension = serde_yaml::Mapping::new();
    extension.insert(
        serde_yaml::Value::String("name".to_owned()),
        serde_yaml::Value::String("client_mcp".to_owned()),
    );
    match mode {
        ToolExtensionInstallMode::Sources => {
            let mut source = serde_yaml::Mapping::new();
            source.insert(
                serde_yaml::Value::String("path".to_owned()),
                serde_yaml::Value::String(client_path.clone()),
            );
            source.insert(
                serde_yaml::Value::String("format".to_owned()),
                serde_yaml::Value::String("EDT".to_owned()),
            );
            extension.insert(
                serde_yaml::Value::String("source".to_owned()),
                serde_yaml::Value::Mapping(source),
            );
        }
        ToolExtensionInstallMode::Artifacts => {
            let mut artifact = serde_yaml::Mapping::new();
            artifact.insert(
                serde_yaml::Value::String("path".to_owned()),
                serde_yaml::Value::String(client_path),
            );
            extension.insert(
                serde_yaml::Value::String("artifact".to_owned()),
                serde_yaml::Value::Mapping(artifact),
            );
        }
    }
    client_mcp.insert(
        serde_yaml::Value::String("extension".to_owned()),
        serde_yaml::Value::Mapping(extension),
    );

    render_local_overlay(root)
}

fn read_local_overlay(path: &Path) -> Result<serde_yaml::Value, AppError> {
    let mut root = if path.exists() {
        let content = fs::read_to_string(path).map_err(io_error("failed to read local config"))?;
        serde_yaml::from_str::<serde_yaml::Value>(&content).map_err(|error| {
            AppError::Runtime(format!("failed to parse local config YAML: {error}"))
        })?
    } else {
        serde_yaml::Value::Mapping(serde_yaml::Mapping::new())
    };
    if root.is_null() {
        root = serde_yaml::Value::Mapping(serde_yaml::Mapping::new());
    }
    Ok(root)
}

fn render_local_overlay(root: serde_yaml::Value) -> Result<String, AppError> {
    let mut rendered = serde_yaml::to_string(&root).map_err(|error| {
        AppError::Runtime(format!("failed to render local config YAML: {error}"))
    })?;
    rendered = with_local_schema_modeline(&rendered);
    Ok(rendered)
}

fn ensure_mapping<'a>(
    parent: &'a mut serde_yaml::Mapping,
    key: &str,
) -> Result<&'a mut serde_yaml::Mapping, AppError> {
    let key_value = serde_yaml::Value::String(key.to_owned());
    if !parent.contains_key(&key_value) {
        parent.insert(
            key_value.clone(),
            serde_yaml::Value::Mapping(serde_yaml::Mapping::new()),
        );
    }
    parent
        .get_mut(&key_value)
        .and_then(serde_yaml::Value::as_mapping_mut)
        .ok_or_else(|| {
            AppError::Validation(format!("local config field '{key}' must be a mapping"))
        })
}

fn yaml_field_eq(value: &serde_yaml::Value, field: &str, expected: &str) -> bool {
    value
        .as_mapping()
        .and_then(|mapping| mapping.get(serde_yaml::Value::String(field.to_owned())))
        .and_then(serde_yaml::Value::as_str)
        == Some(expected)
}

fn with_local_schema_modeline(content: &str) -> String {
    let content = content
        .lines()
        .filter(|line| {
            !line
                .trim_start()
                .starts_with("# yaml-language-server: $schema=")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut rendered = format!("{LOCAL_CONFIG_SCHEMA_MODEL_LINE}\n{content}");
    if !rendered.ends_with('\n') {
        rendered.push('\n');
    }
    rendered
}

fn destination(
    tool: &str,
    release: &GitHubRelease,
    path: PathBuf,
    config: &str,
) -> ToolDownloadDestination {
    ToolDownloadDestination {
        tool: tool.to_owned(),
        tag: release.tag_name.clone(),
        source: release.html_url.clone(),
        path,
        config: config.to_owned(),
    }
}

fn mode_label(mode: ToolExtensionInstallMode) -> &'static str {
    match mode {
        ToolExtensionInstallMode::Sources => "sources",
        ToolExtensionInstallMode::Artifacts => "artifacts",
    }
}

fn download_mode_label(target: ToolDownloadTarget, mode: ToolExtensionInstallMode) -> &'static str {
    match target {
        ToolDownloadTarget::VanessaAutomationSingle => "epf",
        ToolDownloadTarget::Yaxunit | ToolDownloadTarget::ClientMcp => mode_label(mode),
    }
}

fn target_label(target: ToolDownloadTarget) -> &'static str {
    match target {
        ToolDownloadTarget::Yaxunit => "yaxunit",
        ToolDownloadTarget::VanessaAutomationSingle => "vanessa",
        ToolDownloadTarget::ClientMcp => "client-mcp",
    }
}

fn source_archive_url(release: &GitHubRelease) -> String {
    let Some(rest) = release
        .zipball_url
        .strip_prefix("https://api.github.com/repos/")
    else {
        return release.zipball_url.clone();
    };
    let Some((repo, tag)) = rest.split_once("/zipball/") else {
        return release.zipball_url.clone();
    };
    format!("https://codeload.github.com/{repo}/zip/refs/tags/{tag}")
}

fn io_error(context: &'static str) -> impl FnOnce(io::Error) -> AppError {
    move |error| AppError::Runtime(format!("{context}: {error}"))
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    assets: Vec<GitHubAsset>,
    zipball_url: String,
}

impl GitHubRelease {
    fn required_asset(
        &self,
        name_contains: &str,
        extension: &str,
    ) -> Result<&GitHubAsset, AppError> {
        self.assets
            .iter()
            .find(|asset| asset.name.contains(name_contains) && asset.name.ends_with(extension))
            .ok_or_else(|| {
                AppError::Runtime(format!(
                    "latest release '{}' does not contain asset matching '*{}*{}'",
                    self.tag_name, name_contains, extension
                ))
            })
    }
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zip_relative_path_accepts_safe_source_entry() {
        assert_eq!(
            zip_relative_path(
                "bia-technologies-yaxunit/exts/yaxunit/src/Configuration/Configuration.mdo",
                YAXUNIT_SOURCE_PREFIX,
            ),
            Some(PathBuf::from("src/Configuration/Configuration.mdo"))
        );
    }

    #[test]
    fn zip_relative_path_rejects_absolute_and_parent_entries() {
        assert_eq!(
            zip_relative_path("repo/exts/yaxunit//tmp/pwned", YAXUNIT_SOURCE_PREFIX),
            None
        );
        assert_eq!(
            zip_relative_path("repo/exts/yaxunit/../pwned", YAXUNIT_SOURCE_PREFIX),
            None
        );
        assert_eq!(
            zip_relative_path("repo/exts/yaxunit/C:\\temp\\pwned", YAXUNIT_SOURCE_PREFIX,),
            None
        );
    }

    #[test]
    fn source_archive_url_uses_codeload_for_github_zipball() {
        let release = GitHubRelease {
            tag_name: "25.12".to_owned(),
            html_url: "https://github.com/bia-technologies/yaxunit/releases/tag/25.12".to_owned(),
            assets: Vec::new(),
            zipball_url: "https://api.github.com/repos/bia-technologies/yaxunit/zipball/25.12"
                .to_owned(),
        };

        assert_eq!(
            source_archive_url(&release),
            "https://codeload.github.com/bia-technologies/yaxunit/zip/refs/tags/25.12"
        );
    }

    #[test]
    fn source_archive_url_keeps_test_or_custom_urls() {
        let release = GitHubRelease {
            tag_name: "test".to_owned(),
            html_url: "https://example.invalid/test".to_owned(),
            assets: Vec::new(),
            zipball_url: "http://127.0.0.1:1234/archive.zip".to_owned(),
        };

        assert_eq!(
            source_archive_url(&release),
            "http://127.0.0.1:1234/archive.zip"
        );
    }
}
