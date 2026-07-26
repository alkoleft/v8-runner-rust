use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use thiserror::Error;
use walkdir::WalkDir;

use crate::change_detection::scanner::{
    is_always_ignored_relative_path, portable_relative_path, ScanError, SourceInventoryPolicy,
};
use crate::domain::runtime_state::{BaselineRole, IbBaseline, StateGeneration};
use crate::domain::source_set::SourceSetContext;
use crate::use_cases::request::DumpModeRequest;
use crate::use_cases::runtime_state::{inspect_private_cdfi, PrivateCdfiState, RuntimeStateError};

pub(crate) type BaselineFileHash = [u8; 32];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManagedFileAccess {
    Read,
    UpdateMetadata,
}

const BASELINE_MANIFEST_VERSION: u32 = 1;
const BASELINE_MANIFEST_NAME: &str = "manifest.json";
const BASELINE_FILES_NAME: &str = "files";

#[cfg(test)]
thread_local! {
    static AFTER_INVENTORY_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static BEFORE_BASELINE_SEED_COPY: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn set_after_inventory_hook(hook: impl FnOnce() + 'static) {
    AFTER_INVENTORY_HOOK.with(|slot| slot.replace(Some(Box::new(hook))));
}

#[cfg(test)]
fn run_after_inventory_hook() {
    AFTER_INVENTORY_HOOK.with(|slot| {
        if let Some(hook) = slot.take() {
            hook();
        }
    });
}

#[cfg(not(test))]
fn run_after_inventory_hook() {}

#[cfg(test)]
fn run_before_baseline_seed_copy() {
    BEFORE_BASELINE_SEED_COPY.with(|slot| {
        if let Some(hook) = slot.take() {
            hook();
        }
    });
}

#[cfg(test)]
fn set_before_baseline_seed_copy(hook: impl FnOnce() + 'static) {
    BEFORE_BASELINE_SEED_COPY.with(|slot| slot.replace(Some(Box::new(hook))));
}

#[cfg(not(test))]
fn run_before_baseline_seed_copy() {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EffectiveDumpMode {
    Full,
    Incremental,
    Partial,
}

#[derive(Debug)]
pub(crate) enum BaselineInspection {
    Missing,
    Valid(ValidatedBaseline),
    Corrupt(String),
}

#[derive(Debug)]
pub(crate) struct ValidatedBaseline {
    files_root: PathBuf,
    files: Vec<ValidatedBaselineFile>,
}

impl ValidatedBaseline {
    pub(crate) fn files_root(&self) -> &Path {
        &self.files_root
    }

    pub(crate) fn files(&self) -> &[ValidatedBaselineFile] {
        &self.files
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidatedBaselineFile {
    path: String,
    len: u64,
    sha256: BaselineFileHash,
}

impl ValidatedBaselineFile {
    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    #[cfg(test)]
    pub(crate) const fn byte_len(&self) -> u64 {
        self.len
    }

    pub(crate) const fn sha256(&self) -> BaselineFileHash {
        self.sha256
    }
}

#[derive(Debug, Error)]
pub(crate) enum DumpShadowError {
    #[cfg(not(any(unix, windows)))]
    #[error("safe descriptor-relative dump shadow access is unavailable on this platform")]
    UnsupportedSafeFilesystem,
    #[error("failed to inspect dump baseline '{path}': {source}")]
    Inspect { path: PathBuf, source: io::Error },
    #[error("failed to access dump shadow '{path}': {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("invalid managed source inventory: {0}")]
    Inventory(#[from] ScanError),
    #[error("failed to walk dump baseline: {0}")]
    Walk(#[from] walkdir::Error),
    #[error("failed to serialize dump baseline manifest: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("failed to inspect private CDFI: {0}")]
    Cdfi(#[from] RuntimeStateError),
    #[error("dump baseline already exists: '{0}'")]
    AlreadyExists(PathBuf),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BaselineManifest {
    version: u32,
    files: Vec<BaselineFile>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BaselineFile {
    path: String,
    len: u64,
    sha256: String,
}

#[cfg(test)]
pub(crate) fn publish_complete_baseline(
    source_root: &Path,
    excluded_roots: &[PathBuf],
    handle: &IbBaseline,
) -> Result<(), DumpShadowError> {
    stage_complete_baseline(source_root, excluded_roots, handle.path())
}

/// Materializes a complete baseline at an owned, currently absent destination.
///
/// Runtime-state publication uses this to stage the directory inside its journaled
/// transaction before the generation becomes visible in redb.
pub(crate) fn stage_complete_baseline(
    source_root: &Path,
    excluded_roots: &[PathBuf],
    destination: &Path,
) -> Result<(), DumpShadowError> {
    #[cfg(not(any(unix, windows)))]
    return Err(DumpShadowError::UnsupportedSafeFilesystem);
    #[cfg(any(unix, windows))]
    {
        let (source_root, excluded_roots) = canonical_inventory_scope(source_root, excluded_roots)?;
        match fs::symlink_metadata(destination) {
            Ok(_) => return Err(DumpShadowError::AlreadyExists(destination.to_path_buf())),
            Err(source) if source.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(DumpShadowError::Inspect {
                    path: destination.to_path_buf(),
                    source,
                })
            }
        }

        let parent = destination.parent().ok_or_else(|| DumpShadowError::Io {
            path: destination.to_path_buf(),
            source: io::Error::new(io::ErrorKind::InvalidInput, "baseline has no parent"),
        })?;
        fs::create_dir_all(parent).map_err(|source| io_error(parent, source))?;
        let transaction = tempfile::Builder::new()
            .prefix("baseline-")
            .tempdir_in(parent)
            .map_err(|source| io_error(parent, source))?;
        let files_root = transaction.path().join(BASELINE_FILES_NAME);
        fs::create_dir(&files_root).map_err(|source| io_error(&files_root, source))?;

        let inventory = managed_paths(&source_root, &excluded_roots)?;
        run_after_inventory_hook();
        let mut files = Vec::with_capacity(inventory.len());
        for (relative_path, source_path) in inventory {
            let relative = Path::new(&relative_path);
            let target = files_root.join(relative);
            if let Some(target_parent) = target.parent() {
                fs::create_dir_all(target_parent)
                    .map_err(|source| io_error(target_parent, source))?;
            }
            copy_regular_no_follow(&source_root, relative, &source_path, &target)?;
            let (copied_len, copied_hash) =
                hash_regular_file(&target).map_err(|error| regular_error(&target, error))?;
            files.push(BaselineFile {
                path: relative_path,
                len: copied_len,
                sha256: hex_hash(copied_hash),
            });
        }
        files.sort_by(|left, right| left.path.cmp(&right.path));
        let manifest = BaselineManifest {
            version: BASELINE_MANIFEST_VERSION,
            files,
        };
        let manifest_path = transaction.path().join(BASELINE_MANIFEST_NAME);
        let bytes = serde_json::to_vec(&manifest)?;
        write_new_synced(&manifest_path, &bytes)?;
        sync_directory(&files_root)?;
        sync_directory(transaction.path())?;

        let transaction_path = transaction.keep();
        if let Err(source) = fs::rename(&transaction_path, destination) {
            let _ = fs::remove_dir_all(&transaction_path);
            return Err(io_error(destination, source));
        }
        sync_directory(parent)?;
        Ok(())
    }
}

fn managed_paths(
    source_root: &Path,
    excluded_roots: &[PathBuf],
) -> Result<Vec<(String, PathBuf)>, DumpShadowError> {
    let policy = SourceInventoryPolicy::new(source_root, excluded_roots)?;
    let mut files = Vec::new();
    for entry in WalkDir::new(source_root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !entry.file_type().is_dir() || policy.should_descend(entry.path()))
    {
        let entry = entry?;
        if !entry.file_type().is_file() || !policy.includes_file(entry.path()) {
            continue;
        }
        files.push((
            portable_relative_path(source_root, entry.path())?,
            entry.path().to_path_buf(),
        ));
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(files)
}

/// Scan one complete managed tree into a deterministic byte-exact manifest.
///
/// The inventory policy is shared with baseline publication, so platform-owned CDFI,
/// excluded roots and symlinks cannot enter a three-way merge accidentally.
pub(crate) fn managed_manifest(
    source_root: &Path,
    excluded_roots: &[PathBuf],
) -> Result<BTreeMap<String, BaselineFileHash>, DumpShadowError> {
    #[cfg(not(any(unix, windows)))]
    return Err(DumpShadowError::UnsupportedSafeFilesystem);
    #[cfg(any(unix, windows))]
    {
        let (source_root, excluded_roots) = canonical_inventory_scope(source_root, excluded_roots)?;
        let inventory = managed_paths(&source_root, &excluded_roots)?;
        run_after_inventory_hook();
        inventory
            .into_iter()
            .map(|(relative_path, source_path)| {
                #[cfg(unix)]
                let observation = hash_regular_beneath_unix(
                    &source_root,
                    Path::new(&relative_path),
                    &source_path,
                );
                #[cfg(windows)]
                let observation = hash_regular_beneath_windows(
                    &source_root,
                    Path::new(&relative_path),
                    &source_path,
                );
                let (_, hash) = observation.map_err(|error| regular_error(&source_path, error))?;
                Ok((relative_path, hash))
            })
            .collect()
    }
}

pub(crate) fn visit_managed_files(
    root: &Path,
    excluded_roots: &[PathBuf],
    access: ManagedFileAccess,
    mut visitor: impl FnMut(&str, &mut fs::File) -> io::Result<()>,
) -> Result<(), DumpShadowError> {
    let (root, excluded_roots) = canonical_inventory_scope(root, excluded_roots)?;
    let inventory = managed_paths(&root, &excluded_roots)?;
    run_after_inventory_hook();
    for (relative, display) in inventory {
        let mut file = open_managed_file(&root, Path::new(&relative), &display, access)?;
        visitor(&relative, &mut file).map_err(|source| io_error(&display, source))?;
    }
    Ok(())
}

fn canonical_inventory_scope(
    root: &Path,
    excluded_roots: &[PathBuf],
) -> Result<(PathBuf, Vec<PathBuf>), DumpShadowError> {
    let canonical = fs::canonicalize(root).map_err(|source| io_error(root, source))?;
    let metadata =
        fs::symlink_metadata(&canonical).map_err(|source| io_error(&canonical, source))?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(io_error(
            root,
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "managed root is not a directory",
            ),
        ));
    }
    let excluded = excluded_roots
        .iter()
        .map(|excluded| {
            excluded
                .strip_prefix(root)
                .map_or_else(|_| excluded.clone(), |relative| canonical.join(relative))
        })
        .collect();
    Ok((canonical, excluded))
}

fn open_managed_file(
    root: &Path,
    relative: &Path,
    display: &Path,
    access: ManagedFileAccess,
) -> Result<fs::File, DumpShadowError> {
    #[cfg(unix)]
    {
        let _ = access;
        return open_regular_beneath_unix(root, relative, display);
    }
    #[cfg(windows)]
    {
        let relative = relative.to_str().ok_or_else(|| {
            io_error(
                display,
                io::Error::new(io::ErrorKind::InvalidInput, "managed path is not UTF-8"),
            )
        })?;
        let parent = crate::support::windows_fs::open_parent(root, relative, false)
            .map_err(|source| io_error(display, source))?
            .ok_or_else(|| io_error(display, io::Error::from(io::ErrorKind::NotFound)))?;
        return match access {
            ManagedFileAccess::Read => {
                crate::support::windows_fs::open_regular_existing(&parent, false)
            }
            ManagedFileAccess::UpdateMetadata => {
                crate::support::windows_fs::open_regular_existing_for_metadata(&parent)
            }
        }
        .map_err(|source| io_error(display, source));
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (root, relative, display, access);
        Err(DumpShadowError::UnsupportedSafeFilesystem)
    }
}

pub(crate) fn inspect_baseline(handle: &IbBaseline) -> Result<BaselineInspection, DumpShadowError> {
    inspect_baseline_path(handle.path())
}

pub(crate) fn inspect_baseline_path(
    baseline_path: &Path,
) -> Result<BaselineInspection, DumpShadowError> {
    let root_metadata = match fs::symlink_metadata(baseline_path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            return Ok(BaselineInspection::Missing)
        }
        Err(source) => {
            return Err(DumpShadowError::Inspect {
                path: baseline_path.to_path_buf(),
                source,
            })
        }
    };
    if root_metadata.file_type().is_symlink() || !root_metadata.file_type().is_dir() {
        return Ok(BaselineInspection::Corrupt(
            "baseline root is not a regular directory".to_owned(),
        ));
    }

    let manifest_path = baseline_path.join(BASELINE_MANIFEST_NAME);
    let manifest_bytes = match read_regular_file(&manifest_path) {
        Ok(bytes) => bytes,
        Err(ReadRegularError::Invalid(reason)) => return Ok(BaselineInspection::Corrupt(reason)),
        Err(ReadRegularError::Io(source)) if source.kind() == io::ErrorKind::NotFound => {
            return Ok(BaselineInspection::Corrupt(
                "manifest is missing".to_owned(),
            ))
        }
        Err(ReadRegularError::Io(source)) => return Err(io_error(&manifest_path, source)),
    };
    let manifest: BaselineManifest = match serde_json::from_slice(&manifest_bytes) {
        Ok(manifest) => manifest,
        Err(error) => {
            return Ok(BaselineInspection::Corrupt(format!(
                "manifest is malformed: {error}"
            )))
        }
    };
    if manifest.version != BASELINE_MANIFEST_VERSION {
        return Ok(BaselineInspection::Corrupt(format!(
            "unsupported manifest version {}",
            manifest.version
        )));
    }
    if !manifest
        .files
        .windows(2)
        .all(|pair| pair[0].path < pair[1].path)
    {
        return Ok(BaselineInspection::Corrupt(
            "manifest paths are not strictly sorted".to_owned(),
        ));
    }

    let files_root = baseline_path.join(BASELINE_FILES_NAME);
    let files_metadata = match fs::symlink_metadata(&files_root) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            return Ok(BaselineInspection::Corrupt(
                "files root is missing".to_owned(),
            ))
        }
        Err(source) => return Err(io_error(&files_root, source)),
    };
    if files_metadata.file_type().is_symlink() || !files_metadata.file_type().is_dir() {
        return Ok(BaselineInspection::Corrupt(
            "files root is not a regular directory".to_owned(),
        ));
    }

    let mut actual_paths = BTreeSet::new();
    for entry in WalkDir::new(&files_root).follow_links(false) {
        let entry = entry?;
        if entry.path() == files_root {
            continue;
        }
        if entry.file_type().is_dir() {
            continue;
        }
        if !entry.file_type().is_file() || entry.file_type().is_symlink() {
            return Ok(BaselineInspection::Corrupt(format!(
                "baseline contains non-regular entry '{}'",
                entry.path().display()
            )));
        }
        actual_paths.insert(portable_relative_path(&files_root, entry.path())?);
    }

    let expected_paths = manifest
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect::<BTreeSet<_>>();
    if actual_paths != expected_paths {
        return Ok(BaselineInspection::Corrupt(
            "manifest file set differs from baseline files".to_owned(),
        ));
    }
    let mut validated_files = Vec::with_capacity(manifest.files.len());
    for file in &manifest.files {
        if !is_safe_manifest_path(&file.path)
            || is_cdfi_path(&file.path)
            || is_always_ignored_relative_path(Path::new(&file.path))
        {
            return Ok(BaselineInspection::Corrupt(format!(
                "manifest contains invalid managed path '{}'",
                file.path
            )));
        }
        let path = files_root.join(&file.path);
        let (observed_len, hash) = match hash_regular_file(&path) {
            Ok(observation) => observation,
            Err(ReadRegularError::Invalid(reason)) => {
                return Ok(BaselineInspection::Corrupt(reason))
            }
            Err(ReadRegularError::Io(source)) => return Err(io_error(&path, source)),
        };
        let Some(expected_hash) = decode_sha256(&file.sha256) else {
            return Ok(BaselineInspection::Corrupt(format!(
                "baseline file '{}' has invalid SHA-256",
                file.path
            )));
        };
        if observed_len != file.len || hash != expected_hash {
            return Ok(BaselineInspection::Corrupt(format!(
                "baseline file '{}' does not match manifest",
                file.path
            )));
        }
        validated_files.push(ValidatedBaselineFile {
            path: file.path.clone(),
            len: file.len,
            sha256: expected_hash,
        });
    }

    Ok(BaselineInspection::Valid(ValidatedBaseline {
        files_root,
        files: validated_files,
    }))
}

pub(crate) struct DumpShadow {
    _transaction: TempDir,
    path: PathBuf,
    mode: EffectiveDumpMode,
}

impl DumpShadow {
    pub(crate) fn prepare(
        context: &SourceSetContext,
        role: BaselineRole,
        generation: StateGeneration,
        requested: DumpModeRequest,
    ) -> Result<Self, DumpShadowError> {
        let transactions_dir = context.transactions_dir();
        fs::create_dir_all(&transactions_dir)
            .map_err(|source| io_error(&transactions_dir, source))?;
        let transaction = tempfile::Builder::new()
            .prefix("dump-shadow-")
            .tempdir_in(&transactions_dir)
            .map_err(|source| io_error(&transactions_dir, source))?;
        let path = transaction.path().join("shadow");
        fs::create_dir(&path).map_err(|source| io_error(&path, source))?;

        let requested_mode = match requested {
            DumpModeRequest::Full => EffectiveDumpMode::Full,
            DumpModeRequest::Incremental => EffectiveDumpMode::Incremental,
            DumpModeRequest::Partial => EffectiveDumpMode::Partial,
        };
        if requested_mode == EffectiveDumpMode::Full {
            return Ok(Self {
                _transaction: transaction,
                path,
                mode: EffectiveDumpMode::Full,
            });
        }

        let baseline = inspect_baseline(&context.baseline(role, generation))?;
        let cdfi = inspect_private_cdfi(&context.private_cdfi_path())?;
        match baseline {
            BaselineInspection::Missing => Ok(Self::full(transaction, path)),
            BaselineInspection::Corrupt(_reason) => Ok(Self::full(transaction, path)),
            BaselineInspection::Valid(baseline) => match cdfi {
                PrivateCdfiState::Missing => Ok(Self::full(transaction, path)),
                PrivateCdfiState::Corrupt(_reason) => Ok(Self::full(transaction, path)),
                PrivateCdfiState::Valid(cdfi) => {
                    run_before_baseline_seed_copy();
                    let baseline_still_valid = baseline.files_root().parent().is_some_and(|root| {
                        matches!(
                            inspect_baseline_path(root),
                            Ok(BaselineInspection::Valid(_))
                        )
                    });
                    if !baseline_still_valid {
                        return Ok(Self::full(transaction, path));
                    }
                    if copy_validated_baseline(&baseline, &path).is_err() {
                        fs::remove_dir_all(&path).map_err(|source| io_error(&path, source))?;
                        fs::create_dir(&path).map_err(|source| io_error(&path, source))?;
                        return Ok(Self::full(transaction, path));
                    }
                    write_new_synced(&path.join("ConfigDumpInfo.xml"), cdfi.bytes())?;
                    Ok(Self {
                        _transaction: transaction,
                        path,
                        mode: requested_mode,
                    })
                }
            },
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) const fn mode(&self) -> EffectiveDumpMode {
        self.mode
    }

    fn full(transaction: TempDir, path: PathBuf) -> Self {
        Self {
            _transaction: transaction,
            path,
            mode: EffectiveDumpMode::Full,
        }
    }
}

fn copy_validated_baseline(
    baseline: &ValidatedBaseline,
    target_root: &Path,
) -> Result<(), DumpShadowError> {
    for file in baseline.files() {
        let relative = Path::new(file.path());
        let source = baseline.files_root().join(relative);
        let target = target_root.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|source| io_error(parent, source))?;
        }
        copy_regular_no_follow(baseline.files_root(), relative, &source, &target)?;
        let (len, sha256) =
            hash_regular_file(&target).map_err(|error| regular_error(&target, error))?;
        if len != file.len || sha256 != file.sha256 {
            return Err(io_error(
                &source,
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "baseline changed after validation",
                ),
            ));
        }
    }
    Ok(())
}

fn is_safe_manifest_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn is_cdfi_path(path: &str) -> bool {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("ConfigDumpInfo.xml"))
}

fn decode_sha256(value: &str) -> Option<BaselineFileHash> {
    if value.len() != 64 {
        return None;
    }
    let mut result = [0_u8; 32];
    for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        let encoded = std::str::from_utf8(chunk).ok()?;
        result[index] = u8::from_str_radix(encoded, 16).ok()?;
    }
    Some(result)
}

enum ReadRegularError {
    Invalid(String),
    Io(io::Error),
}

fn read_regular_file(path: &Path) -> Result<Vec<u8>, ReadRegularError> {
    let metadata = fs::symlink_metadata(path).map_err(ReadRegularError::Io)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(ReadRegularError::Invalid(format!(
            "'{}' is not a regular file",
            path.display()
        )));
    }
    fs::read(path).map_err(ReadRegularError::Io)
}

fn hash_regular_file(path: &Path) -> Result<(u64, BaselineFileHash), ReadRegularError> {
    let metadata = fs::symlink_metadata(path).map_err(ReadRegularError::Io)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(ReadRegularError::Invalid(format!(
            "'{}' is not a regular file",
            path.display()
        )));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let mut file = options.open(path).map_err(ReadRegularError::Io)?;
    let opened = file.metadata().map_err(ReadRegularError::Io)?;
    #[cfg(windows)]
    let is_reparse_point = {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        opened.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    };
    #[cfg(not(windows))]
    let is_reparse_point = false;
    if !opened.file_type().is_file() || is_reparse_point {
        return Err(ReadRegularError::Invalid(format!(
            "'{}' is not a regular file",
            path.display()
        )));
    }
    let mut digest = Sha256::new();
    let mut length = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(ReadRegularError::Io)?;
        if read == 0 {
            break;
        }
        length = length.saturating_add(read as u64);
        digest.update(&buffer[..read]);
    }
    Ok((length, digest.finalize().into()))
}

#[cfg(windows)]
fn hash_open_file(file: &mut fs::File) -> Result<(u64, BaselineFileHash), ReadRegularError> {
    let mut digest = Sha256::new();
    let mut length = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(ReadRegularError::Io)?;
        if read == 0 {
            break;
        }
        length = length.saturating_add(read as u64);
        digest.update(&buffer[..read]);
    }
    Ok((length, digest.finalize().into()))
}

#[cfg(unix)]
fn hash_regular_beneath_unix(
    source_root: &Path,
    relative: &Path,
    source: &Path,
) -> Result<(u64, BaselineFileHash), ReadRegularError> {
    let mut file =
        open_regular_beneath_unix(source_root, relative, source).map_err(|error| match error {
            DumpShadowError::Io { source, .. } | DumpShadowError::Inspect { source, .. } => {
                ReadRegularError::Io(source)
            }
            other => ReadRegularError::Invalid(other.to_string()),
        })?;
    let mut digest = Sha256::new();
    let mut length = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(ReadRegularError::Io)?;
        if read == 0 {
            break;
        }
        length = length.saturating_add(read as u64);
        digest.update(&buffer[..read]);
    }
    Ok((length, digest.finalize().into()))
}

fn regular_error(path: &Path, error: ReadRegularError) -> DumpShadowError {
    match error {
        ReadRegularError::Invalid(reason) => {
            io_error(path, io::Error::new(io::ErrorKind::InvalidData, reason))
        }
        ReadRegularError::Io(source) => io_error(path, source),
    }
}

fn hex_hash(hash: BaselineFileHash) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(64);
    for byte in hash {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn write_new_synced(path: &Path, bytes: &[u8]) -> Result<(), DumpShadowError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| io_error(path, source))?;
    file.write_all(bytes)
        .map_err(|source| io_error(path, source))?;
    file.sync_all().map_err(|source| io_error(path, source))
}

fn copy_regular_no_follow(
    source_root: &Path,
    relative: &Path,
    source: &Path,
    target: &Path,
) -> Result<(), DumpShadowError> {
    #[cfg(unix)]
    return copy_regular_beneath_unix(source_root, relative, source, target);
    #[cfg(windows)]
    return copy_regular_beneath_windows(source_root, relative, source, target);
    #[cfg(not(any(unix, windows)))]
    Err(DumpShadowError::UnsupportedSafeFilesystem)
}

#[cfg(windows)]
fn open_regular_beneath_windows(
    source_root: &Path,
    relative: &Path,
    source: &Path,
) -> Result<fs::File, DumpShadowError> {
    let relative = relative.to_str().ok_or_else(|| {
        io_error(
            source,
            io::Error::new(io::ErrorKind::InvalidInput, "managed path is not UTF-8"),
        )
    })?;
    let parent = crate::support::windows_fs::open_parent(source_root, relative, false)
        .map_err(|error| io_error(source, error))?
        .ok_or_else(|| io_error(source, io::Error::from(io::ErrorKind::NotFound)))?;
    crate::support::windows_fs::open_regular_existing(&parent, false)
        .map_err(|error| io_error(source, error))
}

#[cfg(windows)]
fn hash_regular_beneath_windows(
    source_root: &Path,
    relative: &Path,
    source: &Path,
) -> Result<(u64, BaselineFileHash), ReadRegularError> {
    let mut file = open_regular_beneath_windows(source_root, relative, source).map_err(
        |error| match error {
            DumpShadowError::Io { source, .. } | DumpShadowError::Inspect { source, .. } => {
                ReadRegularError::Io(source)
            }
            other => ReadRegularError::Invalid(other.to_string()),
        },
    )?;
    hash_open_file(&mut file)
}

#[cfg(windows)]
fn copy_regular_beneath_windows(
    source_root: &Path,
    relative: &Path,
    source: &Path,
    target: &Path,
) -> Result<(), DumpShadowError> {
    let mut input = open_regular_beneath_windows(source_root, relative, source)?;
    let before = input.metadata().map_err(|error| io_error(source, error))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|error| io_error(target, error))?;
    io::copy(&mut input, &mut output).map_err(|error| io_error(target, error))?;
    output.sync_all().map_err(|error| io_error(target, error))?;
    let after = input.metadata().map_err(|error| io_error(source, error))?;
    use std::os::windows::fs::MetadataExt;
    if before.volume_serial_number() != after.volume_serial_number()
        || before.file_index() != after.file_index()
        || before.file_size() != after.file_size()
    {
        return Err(io_error(
            source,
            io::Error::new(io::ErrorKind::InvalidData, "source changed during copy"),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn copy_regular_beneath_unix(
    source_root: &Path,
    relative: &Path,
    source: &Path,
    target: &Path,
) -> Result<(), DumpShadowError> {
    let mut input = open_regular_beneath_unix(source_root, relative, source)?;
    let before = input.metadata().map_err(|error| io_error(source, error))?;
    if !before.file_type().is_file() {
        return Err(io_error(
            source,
            io::Error::new(io::ErrorKind::InvalidData, "source is not a regular file"),
        ));
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|error| io_error(target, error))?;
    io::copy(&mut input, &mut output).map_err(|error| io_error(target, error))?;
    output.sync_all().map_err(|error| io_error(target, error))?;
    let after = input.metadata().map_err(|error| io_error(source, error))?;
    use std::os::unix::fs::MetadataExt;
    if before.dev() != after.dev() || before.ino() != after.ino() || before.len() != after.len() {
        return Err(io_error(
            source,
            io::Error::new(io::ErrorKind::InvalidData, "source changed during copy"),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn open_regular_beneath_unix(
    source_root: &Path,
    relative: &Path,
    source: &Path,
) -> Result<fs::File, DumpShadowError> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;

    let mut root_options = OpenOptions::new();
    root_options
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let mut directory = root_options
        .open(source_root)
        .map_err(|source_error| io_error(source_root, source_error))?;
    let components = relative.components().collect::<Vec<_>>();
    let Some((file_component, parent_components)) = components.split_last() else {
        return Err(io_error(
            source,
            io::Error::new(io::ErrorKind::InvalidInput, "empty relative path"),
        ));
    };
    for component in parent_components {
        let Component::Normal(name) = component else {
            return Err(io_error(
                source,
                io::Error::new(io::ErrorKind::InvalidInput, "non-normal path component"),
            ));
        };
        let name = CString::new(name.as_bytes()).map_err(|_| {
            io_error(
                source,
                io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"),
            )
        })?;
        // SAFETY: the descriptor is live and each single component is opened no-follow.
        let descriptor = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if descriptor < 0 {
            return Err(io_error(source, io::Error::last_os_error()));
        }
        // SAFETY: openat returned a new owned descriptor.
        directory = unsafe { fs::File::from_raw_fd(descriptor) };
    }
    let Component::Normal(file_name) = file_component else {
        return Err(io_error(
            source,
            io::Error::new(io::ErrorKind::InvalidInput, "non-normal file component"),
        ));
    };
    let file_name = CString::new(file_name.as_bytes()).map_err(|_| {
        io_error(
            source,
            io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"),
        )
    })?;
    // SAFETY: same descriptor/name invariants as the directory traversal above.
    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            file_name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        return Err(io_error(source, io::Error::last_os_error()));
    }
    // SAFETY: openat returned a new owned descriptor.
    let input = unsafe { fs::File::from_raw_fd(descriptor) };
    if !input
        .metadata()
        .map_err(|error| io_error(source, error))?
        .file_type()
        .is_file()
    {
        return Err(io_error(
            source,
            io::Error::new(io::ErrorKind::InvalidData, "source is not a regular file"),
        ));
    }
    Ok(input)
}

fn io_error(path: &Path, source: io::Error) -> DumpShadowError {
    DumpShadowError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), DumpShadowError> {
    fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| io_error(path, source))
}

#[cfg(windows)]
fn sync_directory(path: &Path) -> Result<(), DumpShadowError> {
    let directory =
        crate::support::windows_fs::open_root(path).map_err(|source| io_error(path, source))?;
    crate::support::windows_fs::flush(&directory).map_err(|source| io_error(path, source))
}

#[cfg(not(any(unix, windows)))]
fn sync_directory(_path: &Path) -> Result<(), DumpShadowError> {
    Err(DumpShadowError::UnsupportedSafeFilesystem)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use sha2::{Digest, Sha256};
    use tempfile::tempdir;

    use super::{
        inspect_baseline, inspect_baseline_path, managed_manifest, publish_complete_baseline,
        set_after_inventory_hook, set_before_baseline_seed_copy, BaselineInspection, DumpShadow,
        EffectiveDumpMode,
    };
    use crate::config::model::{BuilderBackend, InfobaseConfig, SourceFormat, SourceSetPurpose};
    use crate::domain::runtime_state::{
        BaselineRole, InfobaseIdentity, LogicalSourceRole, RuntimeSourceDescriptor,
        RuntimeSourceIdentityInputs, RuntimeStateLayout, StateGeneration,
    };
    use crate::domain::source_set::SourceSetContext;
    use crate::use_cases::request::DumpModeRequest;

    fn context(base: &Path) -> SourceSetContext {
        let source = base.join("source");
        fs::create_dir_all(&source).expect("source");
        let identity = InfobaseIdentity::normalize(&InfobaseConfig::file(format!(
            "File={}",
            base.join("ib").display()
        )))
        .expect("identity");
        let layout = RuntimeStateLayout::new(base.join("work"), identity).expect("layout");
        let descriptor = RuntimeSourceDescriptor::new(RuntimeSourceIdentityInputs {
            configured_source_identity: Path::new("source"),
            source_root: &source,
            purpose: SourceSetPurpose::Configuration,
            format: SourceFormat::Designer,
            backend: BuilderBackend::Designer,
            logical_role: LogicalSourceRole::DesignerSource,
        })
        .expect("descriptor");
        SourceSetContext::new("main", source, layout.source_state("main", &descriptor))
    }

    fn baseline(
        context: &SourceSetContext,
        generation: u64,
    ) -> crate::domain::runtime_state::IbBaseline {
        context.baseline(
            BaselineRole::ConfiguredSource,
            StateGeneration::new(generation),
        )
    }

    fn valid_cdfi() -> &'static [u8] {
        br#"<ConfigDumpInfo version="2.17"><Metadata id="private-id" configVersion="7"/></ConfigDumpInfo>"#
    }

    #[test]
    fn baseline_round_trip_preserves_bytes_and_sorts_manifest() {
        let dir = tempdir().expect("tempdir");
        let context = context(dir.path());
        let dump = dir.path().join("dump");
        fs::create_dir_all(dump.join("z")).expect("nested");
        fs::write(dump.join("z/last.bin"), [0, 0xff, 7]).expect("binary");
        fs::write(dump.join("alpha.xml"), b"alpha\r\n").expect("alpha");
        fs::write(dump.join("CONFIGDUMPINFO.XML"), b"platform-owned").expect("cdfi");

        let handle = baseline(&context, 1);
        publish_complete_baseline(&dump, &[], &handle).expect("publish baseline");

        let BaselineInspection::Valid(valid) = inspect_baseline(&handle).expect("inspect") else {
            panic!("published baseline must be valid");
        };
        assert_eq!(
            valid.files_root().join("alpha.xml").read_bytes(),
            b"alpha\r\n"
        );
        assert_eq!(
            valid.files_root().join("z/last.bin").read_bytes(),
            &[0, 0xff, 7]
        );
        assert!(!valid.files_root().join("CONFIGDUMPINFO.XML").exists());

        let manifest = fs::read_to_string(handle.path().join("manifest.json")).expect("manifest");
        assert!(manifest.starts_with("{\"version\":1,\"files\":["));
        assert!(
            manifest.find("alpha.xml").expect("alpha") < manifest.find("z/last.bin").expect("z")
        );
        assert_eq!(valid.files().len(), 2);
        assert_eq!(valid.files()[0].path(), "alpha.xml");
        assert_eq!(valid.files()[0].byte_len(), 7);
        let expected: [u8; 32] = Sha256::digest(b"alpha\r\n").into();
        assert_eq!(valid.files()[0].sha256(), expected);
    }

    #[test]
    fn managed_manifest_hashes_exact_bytes_and_excludes_private_entries() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("source");
        let nested_work = root.join("build");
        fs::create_dir_all(root.join("nested")).expect("nested");
        fs::create_dir_all(&nested_work).expect("work");
        fs::write(root.join("alpha.xml"), b"alpha\r\n").expect("alpha");
        fs::write(root.join("nested/binary.bin"), [0, 0xff, 7]).expect("binary");
        fs::write(root.join("ConfigDumpInfo.xml"), b"platform-owned").expect("cdfi");
        fs::write(nested_work.join("ignored.xml"), b"ignored").expect("ignored");

        let manifest = managed_manifest(&root, &[nested_work]).expect("manifest");

        assert_eq!(
            manifest.keys().cloned().collect::<Vec<_>>(),
            vec!["alpha.xml".to_owned(), "nested/binary.bin".to_owned(),]
        );
        let alpha: [u8; 32] = Sha256::digest(b"alpha\r\n").into();
        let binary: [u8; 32] = Sha256::digest([0, 0xff, 7]).into();
        assert_eq!(manifest["alpha.xml"], alpha);
        assert_eq!(manifest["nested/binary.bin"], binary);
    }

    #[cfg(unix)]
    #[test]
    fn managed_manifest_does_not_follow_symlinks() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("source");
        let outside = dir.path().join("outside");
        fs::create_dir_all(&root).expect("source");
        fs::create_dir_all(&outside).expect("outside");
        fs::write(outside.join("secret.xml"), b"secret").expect("secret");
        symlink(&outside, root.join("linked-dir")).expect("linked dir");
        symlink(outside.join("secret.xml"), root.join("linked-file.xml")).expect("linked file");
        fs::write(root.join("managed.xml"), b"managed").expect("managed");

        let manifest = managed_manifest(&root, &[]).expect("manifest");

        assert_eq!(
            manifest.keys().cloned().collect::<Vec<_>>(),
            vec!["managed.xml".to_owned()]
        );
    }

    #[cfg(unix)]
    #[test]
    fn managed_manifest_rejects_parent_replaced_by_symlink_after_inventory() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("source");
        let outside = dir.path().join("outside");
        fs::create_dir_all(root.join("managed")).expect("managed");
        fs::create_dir(&outside).expect("outside");
        fs::write(root.join("managed/file.xml"), b"inside").expect("inside");
        fs::write(outside.join("file.xml"), b"outside").expect("outside file");
        let managed = root.join("managed");
        let displaced = root.join("managed-displaced");
        set_after_inventory_hook({
            let outside = outside.clone();
            move || {
                fs::rename(&managed, displaced).expect("displace managed parent");
                symlink(outside, managed).expect("swap symlink");
            }
        });

        managed_manifest(&root, &[]).expect_err("parent symlink swap must fail");

        assert_eq!(fs::read(outside.join("file.xml")).unwrap(), b"outside");
    }

    #[test]
    fn manifest_observes_the_byte_exact_copied_version() {
        let dir = tempdir().expect("tempdir");
        let context = context(dir.path());
        let source_file = context.path().join("Configuration.xml");
        fs::write(&source_file, b"before").expect("source");
        set_after_inventory_hook({
            let source_file = source_file.clone();
            move || fs::write(source_file, b"after!").expect("replace after inventory")
        });

        let handle = baseline(&context, 1);
        publish_complete_baseline(context.path(), &[], &handle).expect("baseline");

        let BaselineInspection::Valid(valid) = inspect_baseline(&handle).expect("inspect") else {
            panic!("copied baseline must remain self-consistent");
        };
        assert_eq!(
            fs::read(valid.files_root().join("Configuration.xml")).expect("copied"),
            b"after!"
        );
    }

    #[test]
    fn inspection_reports_missing_and_every_corrupt_baseline_shape() {
        let dir = tempdir().expect("tempdir");
        let context = context(dir.path());
        let source = context.path();
        fs::write(source.join("a.xml"), b"one").expect("source");

        let missing = baseline(&context, 1);
        assert!(matches!(
            inspect_baseline(&missing).expect("missing"),
            BaselineInspection::Missing
        ));

        let corrupt_cases: &[(&str, fn(&Path))] = &[
            ("malformed manifest", |root| {
                fs::write(root.join("manifest.json"), b"not-json").expect("manifest")
            }),
            ("missing file", |root| {
                fs::remove_file(root.join("files/a.xml")).expect("remove")
            }),
            ("extra file", |root| {
                fs::write(root.join("files/extra.xml"), b"extra").expect("extra")
            }),
            ("mismatched file", |root| {
                fs::write(root.join("files/a.xml"), b"changed").expect("change")
            }),
        ];

        for (index, (name, corrupt)) in corrupt_cases.iter().enumerate() {
            let handle = baseline(&context, index as u64 + 2);
            publish_complete_baseline(source, &[], &handle).expect("baseline");
            corrupt(handle.path());
            assert!(
                matches!(
                    inspect_baseline(&handle).expect("inspect"),
                    BaselineInspection::Corrupt(_)
                ),
                "{name}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn inspection_rejects_symlink_and_nonregular_entries() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().expect("tempdir");
        let context = context(dir.path());
        fs::write(context.path().join("a.xml"), b"one").expect("source");

        let linked = baseline(&context, 1);
        publish_complete_baseline(context.path(), &[], &linked).expect("baseline");
        fs::remove_file(linked.path().join("files/a.xml")).expect("remove");
        symlink(
            context.path().join("a.xml"),
            linked.path().join("files/a.xml"),
        )
        .expect("link");
        assert!(matches!(
            inspect_baseline(&linked).expect("inspect link"),
            BaselineInspection::Corrupt(_)
        ));

        let fifo = baseline(&context, 2);
        publish_complete_baseline(context.path(), &[], &fifo).expect("baseline");
        fs::remove_file(fifo.path().join("files/a.xml")).expect("remove");
        let status = std::process::Command::new("mkfifo")
            .arg(fifo.path().join("files/a.xml"))
            .status()
            .expect("mkfifo");
        assert!(status.success());
        assert!(matches!(
            inspect_baseline(&fifo).expect("inspect fifo"),
            BaselineInspection::Corrupt(_)
        ));
    }

    #[test]
    fn incremental_shadow_seeds_valid_baseline_and_private_cdfi() {
        let dir = tempdir().expect("tempdir");
        let context = context(dir.path());
        fs::write(context.path().join("Configuration.xml"), b"baseline").expect("source");
        publish_complete_baseline(context.path(), &[], &baseline(&context, 1)).expect("baseline");
        fs::create_dir_all(context.private_cdfi_path().parent().expect("parent")).expect("state");
        fs::write(context.private_cdfi_path(), valid_cdfi()).expect("cdfi");

        let shadow = DumpShadow::prepare(
            &context,
            BaselineRole::ConfiguredSource,
            StateGeneration::new(1),
            DumpModeRequest::Incremental,
        )
        .expect("shadow");

        assert_eq!(shadow.mode(), EffectiveDumpMode::Incremental);
        assert_eq!(
            fs::read(shadow.path().join("Configuration.xml")).expect("file"),
            b"baseline"
        );
        assert_eq!(
            fs::read(shadow.path().join("ConfigDumpInfo.xml")).expect("cdfi"),
            valid_cdfi()
        );
    }

    #[test]
    fn seed_promotes_to_full_when_baseline_changes_after_validation() {
        for mutation in ["extra", "modified"] {
            let dir = tempdir().expect("tempdir");
            let context = context(dir.path());
            fs::write(context.path().join("Configuration.xml"), b"baseline").expect("source");
            let handle = baseline(&context, 1);
            publish_complete_baseline(context.path(), &[], &handle).expect("baseline");
            fs::create_dir_all(context.private_cdfi_path().parent().expect("parent"))
                .expect("state");
            fs::write(context.private_cdfi_path(), valid_cdfi()).expect("cdfi");
            let files = handle.path().join("files");
            set_before_baseline_seed_copy(move || match mutation {
                "extra" => fs::write(files.join("foreign.xml"), b"foreign").expect("extra"),
                "modified" => {
                    fs::write(files.join("Configuration.xml"), b"changed").expect("modified")
                }
                _ => unreachable!(),
            });

            let shadow = DumpShadow::prepare(
                &context,
                BaselineRole::ConfiguredSource,
                StateGeneration::new(1),
                DumpModeRequest::Incremental,
            )
            .expect("safe promotion");

            assert_eq!(shadow.mode(), EffectiveDumpMode::Full);
            assert!(fs::read_dir(shadow.path())
                .expect("empty shadow")
                .next()
                .is_none());
        }
    }

    #[cfg(unix)]
    #[test]
    fn seed_parent_swap_promotes_to_full_without_reading_symlink_target() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().expect("tempdir");
        let context = context(dir.path());
        fs::write(context.path().join("Configuration.xml"), b"baseline").expect("source");
        let handle = baseline(&context, 1);
        publish_complete_baseline(context.path(), &[], &handle).expect("baseline");
        fs::create_dir_all(context.private_cdfi_path().parent().expect("parent")).expect("state");
        fs::write(context.private_cdfi_path(), valid_cdfi()).expect("cdfi");
        let files = handle.path().join("files");
        let displaced = handle.path().join("files-displaced");
        let outside = dir.path().join("outside");
        fs::create_dir(&outside).expect("outside");
        fs::write(outside.join("secret.xml"), b"secret").expect("secret");
        let outside_hook = outside.clone();
        set_before_baseline_seed_copy(move || {
            fs::rename(&files, &displaced).expect("displace files");
            symlink(&outside_hook, &files).expect("swap files root");
        });

        let shadow = DumpShadow::prepare(
            &context,
            BaselineRole::ConfiguredSource,
            StateGeneration::new(1),
            DumpModeRequest::Partial,
        )
        .expect("safe promotion");

        assert_eq!(shadow.mode(), EffectiveDumpMode::Full);
        assert!(fs::read_dir(shadow.path())
            .expect("empty shadow")
            .next()
            .is_none());
        assert_eq!(
            fs::read(outside.join("secret.xml")).expect("secret"),
            b"secret"
        );
    }

    #[test]
    fn full_shadow_is_empty_even_when_seed_state_is_valid() {
        let dir = tempdir().expect("tempdir");
        let context = context(dir.path());
        fs::write(context.path().join("Configuration.xml"), b"baseline").expect("source");
        publish_complete_baseline(context.path(), &[], &baseline(&context, 1)).expect("baseline");
        fs::create_dir_all(context.private_cdfi_path().parent().expect("parent")).expect("state");
        fs::write(context.private_cdfi_path(), valid_cdfi()).expect("cdfi");

        let shadow = DumpShadow::prepare(
            &context,
            BaselineRole::ConfiguredSource,
            StateGeneration::new(1),
            DumpModeRequest::Full,
        )
        .expect("shadow");

        assert_eq!(shadow.mode(), EffectiveDumpMode::Full);
        assert!(fs::read_dir(shadow.path())
            .expect("shadow dir")
            .next()
            .is_none());
    }

    #[test]
    fn missing_or_corrupt_seed_promotes_incremental_and_partial_to_full() {
        let dir = tempdir().expect("tempdir");
        let context = context(dir.path());
        fs::create_dir_all(context.private_cdfi_path().parent().expect("parent")).expect("state");
        fs::write(context.private_cdfi_path(), valid_cdfi()).expect("cdfi");

        let missing = DumpShadow::prepare(
            &context,
            BaselineRole::ConfiguredSource,
            StateGeneration::new(1),
            DumpModeRequest::Incremental,
        )
        .expect("missing baseline promotion");
        assert_eq!(missing.mode(), EffectiveDumpMode::Full);

        fs::write(context.path().join("Configuration.xml"), b"baseline").expect("source");
        publish_complete_baseline(context.path(), &[], &baseline(&context, 2)).expect("baseline");
        fs::write(context.private_cdfi_path(), b"broken xml").expect("corrupt cdfi");
        let corrupt = DumpShadow::prepare(
            &context,
            BaselineRole::ConfiguredSource,
            StateGeneration::new(2),
            DumpModeRequest::Partial,
        )
        .expect("corrupt cdfi promotion");
        assert_eq!(corrupt.mode(), EffectiveDumpMode::Full);
        assert!(fs::read_dir(corrupt.path())
            .expect("shadow dir")
            .next()
            .is_none());
    }

    #[test]
    fn corrupt_baseline_or_missing_cdfi_also_promotes_to_full() {
        let dir = tempdir().expect("tempdir");
        let context = context(dir.path());
        fs::write(context.path().join("Configuration.xml"), b"baseline").expect("source");

        let corrupt_handle = baseline(&context, 1);
        publish_complete_baseline(context.path(), &[], &corrupt_handle).expect("baseline");
        fs::write(corrupt_handle.path().join("manifest.json"), b"broken").expect("corrupt");
        fs::create_dir_all(context.private_cdfi_path().parent().expect("parent")).expect("state");
        fs::write(context.private_cdfi_path(), valid_cdfi()).expect("cdfi");
        let corrupt_baseline = DumpShadow::prepare(
            &context,
            BaselineRole::ConfiguredSource,
            StateGeneration::new(1),
            DumpModeRequest::Incremental,
        )
        .expect("corrupt baseline promotion");
        assert_eq!(corrupt_baseline.mode(), EffectiveDumpMode::Full);

        let valid_handle = baseline(&context, 2);
        publish_complete_baseline(context.path(), &[], &valid_handle).expect("baseline");
        fs::remove_file(context.private_cdfi_path()).expect("remove cdfi");
        let missing_cdfi = DumpShadow::prepare(
            &context,
            BaselineRole::ConfiguredSource,
            StateGeneration::new(2),
            DumpModeRequest::Partial,
        )
        .expect("missing cdfi promotion");
        assert_eq!(missing_cdfi.mode(), EffectiveDumpMode::Full);
    }

    #[cfg(unix)]
    #[test]
    fn managed_manifest_accepts_symlinked_configured_root() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().expect("tempdir");
        let real = dir.path().join("real-source");
        fs::create_dir(&real).expect("real source");
        fs::write(real.join("Configuration.xml"), b"source").expect("source file");
        let linked = dir.path().join("configured-source");
        symlink(&real, &linked).expect("source symlink");

        let manifest = managed_manifest(&linked, &[]).expect("manifest through symlink root");

        assert!(manifest.contains_key("Configuration.xml"));
    }

    #[test]
    fn baseline_manifest_rejects_always_ignored_paths() {
        let dir = tempdir().expect("tempdir");
        let baseline = dir.path().join("baseline");
        let files = baseline.join("files/.git");
        fs::create_dir_all(&files).expect("files");
        let bytes = b"foreign";
        fs::write(files.join("config"), bytes).expect("ignored file");
        let hash = format!("{:x}", Sha256::digest(bytes));
        fs::write(
            baseline.join("manifest.json"),
            serde_json::to_vec(&serde_json::json!({
                "version": 1,
                "files": [{"path": ".git/config", "len": bytes.len(), "sha256": hash}]
            }))
            .expect("manifest json"),
        )
        .expect("manifest");

        let inspection = inspect_baseline_path(&baseline).expect("inspection");

        assert!(matches!(inspection, BaselineInspection::Corrupt(_)));
    }

    trait ReadBytes {
        fn read_bytes(&self) -> Vec<u8>;
    }

    impl ReadBytes for PathBuf {
        fn read_bytes(&self) -> Vec<u8> {
            fs::read(self).expect("read bytes")
        }
    }
}
