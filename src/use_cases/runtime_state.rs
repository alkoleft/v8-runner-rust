use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

#[cfg(test)]
thread_local! {
    static BEFORE_REDB_CLAIM_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static AFTER_REDB_CLAIM_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static BEFORE_REDB_ROLLBACK_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static FORCE_REDB_PUBLISH_FAILURE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static DUMP_COMMIT_CRASH_PHASE: std::cell::Cell<Option<DumpCommitCrashPhase>> =
        const { std::cell::Cell::new(None) };
    static FAIL_DUMP_COMMIT_AFTER_REDB: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static BEFORE_BASELINE_DESTRUCTIVE_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

use crate::change_detection::analyzer::PreparedStateUpdate;
use crate::change_detection::hash_storage::{
    HashStorage, ObservedStorageState, StorageError, StoredFileState,
};
use crate::domain::runtime_state::{BaselineRole, DumpTransactionId, StateGeneration};
use crate::domain::source_set::SourceSetContext;
use crate::support::fs::{acquire_advisory_lock, AdvisoryLockGuard};
use crate::use_cases::dump_shadow::{
    inspect_baseline_path, stage_complete_baseline, BaselineInspection, DumpShadowError,
};

pub(crate) struct DesignerStateLock {
    path: PathBuf,
    _guard: AdvisoryLockGuard,
}

/// Complete private artifacts committed by a successful dump operation.
///
/// The `SourceSetContext` passed to [`commit_dump_state_with_lock`] is the sole
/// owner of every artifact in this request. For EDT this is the configured EDT
/// context: its observation, configured-source baseline, optional intermediate
/// Designer baseline and private CDFI deliberately share one generation. The
/// generated Designer build context is not a second dump-state owner.
pub(crate) struct DumpStateCommitRequest<'a> {
    prepared: &'a PreparedStateUpdate,
    configured_source_root: &'a Path,
    edt_platform_designer_root: Option<&'a Path>,
    produced_cdfi: &'a Path,
    transaction_id: DumpTransactionId,
}

impl<'a> DumpStateCommitRequest<'a> {
    pub(crate) fn new(
        prepared: &'a PreparedStateUpdate,
        configured_source_root: &'a Path,
        produced_cdfi: &'a Path,
    ) -> Self {
        Self {
            prepared,
            configured_source_root,
            edt_platform_designer_root: None,
            produced_cdfi,
            transaction_id: DumpTransactionId::new(),
        }
    }

    pub(crate) const fn with_edt_platform_designer(mut self, root: &'a Path) -> Self {
        self.edt_platform_designer_root = Some(root);
        self
    }

    pub(crate) fn with_transaction_id(mut self, transaction_id: DumpTransactionId) -> Self {
        self.transaction_id = transaction_id;
        self
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DumpCommitCrashPhase {
    AfterBaselines,
    AfterCdfi,
    AfterRedb,
    AfterBaselineMarkerRemoval,
}

#[cfg(test)]
fn set_dump_commit_crash_phase(phase: DumpCommitCrashPhase) {
    DUMP_COMMIT_CRASH_PHASE.with(|slot| slot.set(Some(phase)));
}

#[cfg(test)]
fn fail_next_dump_commit_after_redb() {
    FAIL_DUMP_COMMIT_AFTER_REDB.with(|slot| slot.set(true));
}

#[cfg(test)]
fn inject_dump_crash(phase: DumpCommitCrashPhase) -> Result<(), RuntimeStateError> {
    if DUMP_COMMIT_CRASH_PHASE.with(|slot| slot.get() == Some(phase)) {
        DUMP_COMMIT_CRASH_PHASE.with(|slot| slot.set(None));
        Err(RuntimeStateError::InjectedDumpCrash(phase))
    } else {
        Ok(())
    }
}

pub(crate) fn lock_designer_state(
    context: &SourceSetContext,
) -> Result<DesignerStateLock, RuntimeStateError> {
    let path = context.state_lock_path();
    let guard = acquire_advisory_lock(&path)?;
    Ok(DesignerStateLock {
        path,
        _guard: guard,
    })
}

fn validate_lock(
    context: &SourceSetContext,
    lock: &DesignerStateLock,
) -> Result<(), RuntimeStateError> {
    if lock.path == context.state_lock_path() {
        Ok(())
    } else {
        Err(RuntimeStateError::InvalidJournal {
            path: lock.path.clone(),
            reason: "runtime-state lock belongs to another source context".to_owned(),
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ValidatedCdfi {
    bytes: Vec<u8>,
}

impl ValidatedCdfi {
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[derive(Debug, Clone)]
pub(crate) enum PrivateCdfiState {
    Missing,
    Valid(ValidatedCdfi),
    Corrupt(String),
}

#[derive(Debug, Error)]
pub(crate) enum RuntimeStateError {
    #[error("failed to inspect private CDFI '{path}': {source}")]
    Inspect {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to read private CDFI '{path}': {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid produced private CDFI '{path}': {reason}")]
    InvalidProducedCdfi { path: PathBuf, reason: String },
    #[error("failed to prepare private runtime-state transaction: {0}")]
    TransactionIo(#[from] std::io::Error),
    #[error("failed to {operation} runtime-state path '{path}': {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to prepare private hash storage: {0}")]
    Storage(#[from] StorageError),
    #[error(
        "runtime state generation changed before publication: expected {expected}, found {actual}"
    )]
    StaleGeneration { expected: u64, actual: u64 },
    #[error("invalid runtime-state journal '{path}': {reason}")]
    InvalidJournal { path: PathBuf, reason: String },
    #[error("runtime state generation cannot advance beyond u64::MAX")]
    GenerationOverflow,
    #[error("storage state changed before publication at '{path}'")]
    StorageObservationChanged { path: PathBuf },
    #[error("failed to prepare private dump baseline: {0}")]
    DumpShadow(Box<DumpShadowError>),
    #[cfg(test)]
    #[error("injected dump-state crash after {0:?}")]
    InjectedDumpCrash(DumpCommitCrashPhase),
    #[error("runtime-state publication failed ({publication}); rollback also failed ({rollback}); journal retained at '{journal}'")]
    PublicationAndRollback {
        publication: Box<RuntimeStateError>,
        rollback: Box<RuntimeStateError>,
        journal: PathBuf,
    },
}

impl From<DumpShadowError> for RuntimeStateError {
    fn from(error: DumpShadowError) -> Self {
        Self::DumpShadow(Box::new(error))
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum JournalStatus {
    ClaimingRecoverable,
    Prepared,
    Committed,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StateJournal {
    status: JournalStatus,
    #[serde(default)]
    generation: u64,
    redb_existed: bool,
    cdfi_existed: bool,
    redb_staged: FileFingerprint,
    cdfi_staged: FileFingerprint,
    #[serde(default)]
    baselines: Vec<JournalBaseline>,
    #[serde(default)]
    dump_transaction_id: Option<DumpTransactionId>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum JournalBaselineRole {
    ConfiguredSource,
    EdtPlatformDesigner,
}

impl JournalBaselineRole {
    const fn domain(self) -> BaselineRole {
        match self {
            Self::ConfiguredSource => BaselineRole::ConfiguredSource,
            Self::EdtPlatformDesigner => BaselineRole::EdtPlatformDesigner,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalBaseline {
    role: JournalBaselineRole,
    staged_name: String,
    ownership_token: String,
    manifest_fingerprint: FileFingerprint,
    #[serde(default)]
    directory_identity: Option<FileIdentity>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct FileFingerprint {
    len: u64,
    sha256: String,
    identity: Option<FileIdentity>,
}

impl FileFingerprint {
    fn same_contents(&self, other: &Self) -> bool {
        self.len == other.len && self.sha256 == other.sha256
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct FileIdentity {
    volume: u64,
    index: u64,
}

const JOURNAL_FILE: &str = "journal.json";
const STAGED_REDB: &str = "new-hash-storage.redb";
const STAGED_CDFI: &str = "new-ConfigDumpInfo.xml";
const BACKUP_REDB: &str = "old-hash-storage.redb";
const BACKUP_CDFI: &str = "old-ConfigDumpInfo.xml";
const BASELINE_OWNERSHIP_FILE: &str = ".runtime-state-transaction";
const FULL_REBUILD_MARKER: &str = "full-rebuild-required";

pub(crate) fn designer_full_rebuild_required(
    context: &SourceSetContext,
) -> Result<bool, RuntimeStateError> {
    full_rebuild_marker_path(context)
        .try_exists()
        .map_err(RuntimeStateError::TransactionIo)
}

pub(crate) fn require_designer_full_rebuild(
    context: &SourceSetContext,
) -> Result<(), RuntimeStateError> {
    let marker = full_rebuild_marker_path(context);
    if let Some(parent) = marker.parent() {
        ensure_directory_synced(parent)?;
    }
    write_synced_file(&marker, b"previous Designer result was ambiguous\n")
}

pub(crate) fn inspect_private_cdfi(path: &Path) -> Result<PrivateCdfiState, RuntimeStateError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == ErrorKind::NotFound => {
            return Ok(PrivateCdfiState::Missing)
        }
        Err(source) => {
            return Err(RuntimeStateError::Inspect {
                path: path.to_path_buf(),
                source,
            })
        }
    };
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Ok(PrivateCdfiState::Corrupt(
            "private CDFI is not a regular file".to_owned(),
        ));
    }
    let bytes = fs::read(path).map_err(|source| RuntimeStateError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    match validate_cdfi_bytes(&bytes) {
        Ok(()) => Ok(PrivateCdfiState::Valid(ValidatedCdfi { bytes })),
        Err(reason) => Ok(PrivateCdfiState::Corrupt(reason)),
    }
}

fn validate_cdfi_bytes(bytes: &[u8]) -> Result<(), String> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut root_seen = false;
    let mut root_closed = false;
    let mut depth = 0_u64;
    let mut metadata_identity_seen = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => {
                if depth == 0 && root_seen {
                    return Err("CDFI contains more than one root element".to_owned());
                }
                inspect_cdfi_element(
                    &event,
                    reader.decoder(),
                    &mut root_seen,
                    &mut metadata_identity_seen,
                )?;
                depth += 1;
            }
            Ok(Event::Empty(event)) => {
                if depth == 0 && root_seen {
                    return Err("CDFI contains more than one root element".to_owned());
                }
                inspect_cdfi_element(
                    &event,
                    reader.decoder(),
                    &mut root_seen,
                    &mut metadata_identity_seen,
                )?;
                if depth == 0 {
                    root_closed = true;
                }
            }
            Ok(Event::End(_)) => {
                if depth == 0 {
                    return Err("CDFI contains an unmatched closing element".to_owned());
                }
                depth -= 1;
                if depth == 0 {
                    root_closed = true;
                }
            }
            Ok(Event::Text(text)) if depth == 0 => {
                let decoded = text
                    .unescape()
                    .map_err(|error| format!("invalid text outside CDFI root: {error}"))?;
                if !decoded.trim().is_empty() {
                    return Err("CDFI contains text outside the root element".to_owned());
                }
            }
            Ok(Event::CData(text)) if depth == 0 => {
                let decoded = reader
                    .decoder()
                    .decode(text.as_ref())
                    .map_err(|error| format!("invalid CDATA outside CDFI root: {error}"))?;
                if !decoded.trim().is_empty() {
                    return Err("CDFI contains CDATA outside the root element".to_owned());
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(error) => return Err(format!("malformed CDFI XML: {error}")),
        }
    }
    if !root_seen || !root_closed || depth != 0 {
        return Err("CDFI root element is missing or truncated".to_owned());
    }
    if !metadata_identity_seen {
        return Err("CDFI has no non-empty metadata identity/version".to_owned());
    }
    Ok(())
}

fn inspect_cdfi_element(
    event: &BytesStart<'_>,
    decoder: quick_xml::encoding::Decoder,
    root_seen: &mut bool,
    metadata_identity_seen: &mut bool,
) -> Result<(), String> {
    let local_name = event.local_name();
    if !*root_seen {
        if local_name.as_ref() != b"ConfigDumpInfo" {
            return Err("unexpected CDFI root element".to_owned());
        }
        *root_seen = true;
        let mut version = None;
        for attribute in event.attributes() {
            let attribute = attribute.map_err(|error| error.to_string())?;
            if attribute.key.local_name().as_ref() == b"version" {
                version = Some(
                    attribute
                        .decode_and_unescape_value(decoder)
                        .map_err(|error| error.to_string())?
                        .into_owned(),
                );
            }
        }
        if version
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        {
            return Err("CDFI root has no non-empty version".to_owned());
        }
    } else if local_name.as_ref() == b"Metadata" {
        let mut id = None;
        let mut config_version = None;
        for attribute in event.attributes() {
            let attribute = attribute.map_err(|error| error.to_string())?;
            match attribute.key.local_name().as_ref() {
                b"id" => {
                    id = Some(
                        attribute
                            .decode_and_unescape_value(decoder)
                            .map_err(|error| error.to_string())?
                            .into_owned(),
                    )
                }
                b"configVersion" => {
                    config_version = Some(
                        attribute
                            .decode_and_unescape_value(decoder)
                            .map_err(|error| error.to_string())?
                            .into_owned(),
                    )
                }
                _ => {}
            }
        }
        if id.as_deref().is_some_and(|value| !value.trim().is_empty())
            && config_version
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
        {
            *metadata_identity_seen = true;
        }
    }
    Ok(())
}

pub(crate) fn recover_designer_state(context: &SourceSetContext) -> Result<(), RuntimeStateError> {
    let lock = lock_designer_state(context)?;
    recover_designer_state_with_lock(context, &lock)
}

pub(crate) fn recover_designer_state_with_lock(
    context: &SourceSetContext,
    lock: &DesignerStateLock,
) -> Result<(), RuntimeStateError> {
    validate_lock(context, lock)?;
    let transactions = context.transactions_dir();
    let entries = match fs::read_dir(&transactions) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(RuntimeStateError::TransactionIo(error)),
    };
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir()
            || !entry.file_name().to_string_lossy().starts_with("state-")
        {
            continue;
        }
        recover_one_transaction(context, &entry.path())?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn commit_designer_state(
    context: &SourceSetContext,
    prepared: &PreparedStateUpdate,
    produced_cdfi: &Path,
) -> Result<(), RuntimeStateError> {
    let lock = lock_designer_state(context)?;
    commit_designer_state_with_lock(context, &lock, prepared, produced_cdfi)
}

pub(crate) fn commit_designer_state_with_lock(
    context: &SourceSetContext,
    lock: &DesignerStateLock,
    prepared: &PreparedStateUpdate,
    produced_cdfi: &Path,
) -> Result<(), RuntimeStateError> {
    validate_lock(context, lock)?;
    let produced = match inspect_private_cdfi(produced_cdfi)? {
        PrivateCdfiState::Valid(cdfi) => cdfi,
        PrivateCdfiState::Missing => {
            require_designer_full_rebuild(context)?;
            return Err(RuntimeStateError::InvalidProducedCdfi {
                path: produced_cdfi.to_path_buf(),
                reason: "platform did not produce ConfigDumpInfo.xml".to_owned(),
            });
        }
        PrivateCdfiState::Corrupt(reason) => {
            require_designer_full_rebuild(context)?;
            return Err(RuntimeStateError::InvalidProducedCdfi {
                path: produced_cdfi.to_path_buf(),
                reason,
            });
        }
    };
    recover_designer_state_with_lock(context, lock)?;
    verify_storage_observation(context, &prepared.observed_storage)?;

    ensure_directory_synced(&context.transactions_dir())?;
    let transaction = context
        .transactions_dir()
        .join(format!("state-{}", Uuid::new_v4()));
    fs::create_dir(&transaction)?;
    sync_directory(&context.transactions_dir())?;
    let result = publish_state_transaction(context, prepared, &produced, &[], None, &transaction);
    if let Err(publication) = result {
        return match recover_one_transaction(context, &transaction) {
            Ok(()) => Err(publication),
            Err(rollback) => Err(RuntimeStateError::PublicationAndRollback {
                publication: Box::new(publication),
                rollback: Box::new(rollback),
                journal: transaction,
            }),
        };
    }
    result?;
    clear_full_rebuild_marker(context)
}

pub(crate) fn commit_dump_state_with_lock(
    context: &SourceSetContext,
    lock: &DesignerStateLock,
    request: DumpStateCommitRequest<'_>,
) -> Result<StateGeneration, RuntimeStateError> {
    validate_lock(context, lock)?;
    let produced = match inspect_private_cdfi(request.produced_cdfi)? {
        PrivateCdfiState::Valid(cdfi) => cdfi,
        PrivateCdfiState::Missing => {
            return Err(RuntimeStateError::InvalidProducedCdfi {
                path: request.produced_cdfi.to_path_buf(),
                reason: "platform did not produce ConfigDumpInfo.xml".to_owned(),
            })
        }
        PrivateCdfiState::Corrupt(reason) => {
            return Err(RuntimeStateError::InvalidProducedCdfi {
                path: request.produced_cdfi.to_path_buf(),
                reason,
            })
        }
    };
    recover_designer_state_with_lock(context, lock)?;
    verify_storage_observation(context, &request.prepared.observed_storage)?;
    let next_generation = request
        .prepared
        .observed_storage
        .generation()
        .checked_add(1)
        .ok_or(RuntimeStateError::GenerationOverflow)?;

    ensure_directory_synced(&context.transactions_dir())?;
    let transaction = context
        .transactions_dir()
        .join(format!("state-{}", Uuid::new_v4()));
    fs::create_dir(&transaction)?;
    sync_directory(&context.transactions_dir())?;
    let mut baselines = vec![BaselineInput {
        role: JournalBaselineRole::ConfiguredSource,
        root: request.configured_source_root,
    }];
    if let Some(root) = request.edt_platform_designer_root {
        baselines.push(BaselineInput {
            role: JournalBaselineRole::EdtPlatformDesigner,
            root,
        });
    }
    let result = publish_state_transaction(
        context,
        request.prepared,
        &produced,
        &baselines,
        Some(&request.transaction_id),
        &transaction,
    );
    if let Err(publication) = result {
        #[cfg(test)]
        if matches!(publication, RuntimeStateError::InjectedDumpCrash(_)) {
            return Err(publication);
        }
        return match recover_one_transaction(context, &transaction) {
            Ok(())
                if HashStorage::new(context.storage_path())
                    .current_generation()
                    .is_ok_and(|generation| generation == next_generation)
                    && HashStorage::new(context.storage_path())
                        .current_dump_transaction_id()
                        .is_ok_and(|transaction_id| {
                            transaction_id.as_ref() == Some(&request.transaction_id)
                        }) =>
            {
                Ok(StateGeneration::new(next_generation))
            }
            Ok(()) => Err(publication),
            Err(rollback) => Err(RuntimeStateError::PublicationAndRollback {
                publication: Box::new(publication),
                rollback: Box::new(rollback),
                journal: transaction,
            }),
        };
    }
    result
}

struct BaselineInput<'a> {
    role: JournalBaselineRole,
    root: &'a Path,
}

pub(crate) fn cleanup_orphan_designer_transactions(
    context: &SourceSetContext,
    lock: &DesignerStateLock,
) -> Result<(), RuntimeStateError> {
    validate_lock(context, lock)?;
    let entries = match fs::read_dir(context.transactions_dir()) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(RuntimeStateError::TransactionIo(error)),
    };
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir()
            && entry
                .file_name()
                .to_string_lossy()
                .starts_with("designer-build-")
        {
            fs::remove_dir_all(entry.path())?;
        }
    }
    sync_directory(&context.transactions_dir())
}

fn clear_full_rebuild_marker(context: &SourceSetContext) -> Result<(), RuntimeStateError> {
    let marker = full_rebuild_marker_path(context);
    match fs::remove_file(&marker) {
        Ok(()) => {
            if let Some(parent) = marker.parent() {
                sync_directory(parent)?;
            }
            Ok(())
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(RuntimeStateError::TransactionIo(error)),
    }
}

fn full_rebuild_marker_path(context: &SourceSetContext) -> PathBuf {
    context
        .transactions_dir()
        .parent()
        .map(|state_dir| state_dir.join(FULL_REBUILD_MARKER))
        .unwrap_or_else(|| context.transactions_dir().join(FULL_REBUILD_MARKER))
}

fn verify_storage_observation(
    context: &SourceSetContext,
    expected: &ObservedStorageState,
) -> Result<(), RuntimeStateError> {
    let storage = HashStorage::new(context.storage_path());
    match expected {
        ObservedStorageState::MissingPath => match storage.load_state() {
            Ok(crate::change_detection::hash_storage::HashStorageLoad::MissingPath) => Ok(()),
            Ok(crate::change_detection::hash_storage::HashStorageLoad::ExistingUninitialized)
            | Ok(crate::change_detection::hash_storage::HashStorageLoad::Initialized(_)) => {
                Err(RuntimeStateError::StorageObservationChanged {
                    path: context.storage_path(),
                })
            }
            Err(error) => Err(RuntimeStateError::Storage(error)),
        },
        ObservedStorageState::ExistingUninitialized { .. } => {
            let actual = storage.uninitialized_observation()?;
            if &actual == expected {
                Ok(())
            } else {
                Err(RuntimeStateError::StorageObservationChanged {
                    path: context.storage_path(),
                })
            }
        }
        ObservedStorageState::Initialized {
            generation: expected,
        } => match storage.current_generation() {
            Ok(actual) if actual == *expected => Ok(()),
            Ok(actual) => Err(RuntimeStateError::StaleGeneration {
                expected: *expected,
                actual,
            }),
            Err(error) => Err(RuntimeStateError::Storage(error)),
        },
        ObservedStorageState::Recoverable { .. } => {
            let actual = storage.recoverable_observation()?;
            if &actual == expected {
                Ok(())
            } else {
                Err(RuntimeStateError::StorageObservationChanged {
                    path: context.storage_path(),
                })
            }
        }
    }
}

fn publish_state_transaction(
    context: &SourceSetContext,
    prepared: &PreparedStateUpdate,
    cdfi: &ValidatedCdfi,
    baselines: &[BaselineInput<'_>],
    dump_transaction_id: Option<&DumpTransactionId>,
    transaction: &Path,
) -> Result<StateGeneration, RuntimeStateError> {
    let snapshot: HashMap<String, StoredFileState> = prepared
        .snapshot
        .iter()
        .map(|file| {
            (
                file.rel_path.clone(),
                StoredFileState {
                    mtime_ns: file.mtime_ns,
                    hash: file.hash.clone(),
                },
            )
        })
        .collect();
    let next_generation = prepared
        .observed_storage
        .generation()
        .checked_add(1)
        .ok_or(RuntimeStateError::GenerationOverflow)?;
    if let Some(transaction_id) = dump_transaction_id {
        HashStorage::create_dump_replacement(
            transaction.join(STAGED_REDB),
            &snapshot,
            prepared.scan_started_at,
            next_generation,
            transaction_id,
        )?;
    } else {
        HashStorage::create_replacement(
            transaction.join(STAGED_REDB),
            &snapshot,
            prepared.scan_started_at,
            next_generation,
        )?;
    }
    write_synced_file(&transaction.join(STAGED_CDFI), cdfi.bytes())?;
    let mut journal_baselines = Vec::with_capacity(baselines.len());
    for baseline in baselines {
        let staged_name = match baseline.role {
            JournalBaselineRole::ConfiguredSource => "new-baseline-configured-source",
            JournalBaselineRole::EdtPlatformDesigner => "new-baseline-edt-platform-designer",
        };
        let staged = transaction.join(staged_name);
        stage_complete_baseline(baseline.root, &[], &staged)?;
        let manifest_fingerprint = file_fingerprint(&staged.join("manifest.json"))?;
        let ownership_token = Uuid::new_v4().to_string();
        write_synced_file(
            &staged.join(BASELINE_OWNERSHIP_FILE),
            ownership_token.as_bytes(),
        )?;
        journal_baselines.push(JournalBaseline {
            role: baseline.role,
            staged_name: staged_name.to_owned(),
            ownership_token,
            manifest_fingerprint,
            directory_identity: opened_file_identity(&fs::File::open(&staged)?)?,
        });
    }
    let redb_staged = file_fingerprint(&transaction.join(STAGED_REDB))?;
    let cdfi_staged = file_fingerprint(&transaction.join(STAGED_CDFI))?;

    let redb_target = context.storage_path();
    let cdfi_target = context.private_cdfi_path();
    let cdfi_existed = backup_regular_file(&cdfi_target, &transaction.join(BACKUP_CDFI))?;
    if baselines.is_empty() && requires_storage_claim(&prepared.observed_storage) {
        write_journal(
            transaction,
            &StateJournal {
                status: JournalStatus::ClaimingRecoverable,
                generation: next_generation,
                redb_existed: true,
                cdfi_existed,
                redb_staged: redb_staged.clone(),
                cdfi_staged: cdfi_staged.clone(),
                baselines: journal_baselines.clone(),
                dump_transaction_id: dump_transaction_id.cloned(),
            },
        )?;
    }
    let redb_existed = if baselines.is_empty() {
        prepare_redb_backup(
            &redb_target,
            &transaction.join(BACKUP_REDB),
            &transaction.join(STAGED_REDB),
            &prepared.observed_storage,
        )?
    } else {
        let existed = backup_regular_file(&redb_target, &transaction.join(BACKUP_REDB))?;
        verify_storage_observation(context, &prepared.observed_storage)?;
        existed
    };
    #[cfg(test)]
    if baselines.is_empty() && requires_storage_claim(&prepared.observed_storage) {
        AFTER_REDB_CLAIM_HOOK.with(|cell| {
            if let Some(hook) = cell.borrow_mut().take() {
                hook();
            }
        });
    }
    // Close the staging/backup TOCTOU window before the first live-file mutation.
    if baselines.is_empty() && !requires_storage_claim(&prepared.observed_storage) {
        verify_storage_observation(context, &prepared.observed_storage)?;
    }
    write_journal(
        transaction,
        &StateJournal {
            status: JournalStatus::Prepared,
            generation: next_generation,
            redb_existed,
            cdfi_existed,
            redb_staged: redb_staged.clone(),
            cdfi_staged: cdfi_staged.clone(),
            baselines: journal_baselines.clone(),
            dump_transaction_id: dump_transaction_id.cloned(),
        },
    )?;

    for baseline in &journal_baselines {
        publish_baseline(
            context,
            StateGeneration::new(next_generation),
            transaction,
            baseline,
        )?;
    }
    #[cfg(test)]
    if !baselines.is_empty() {
        inject_dump_crash(DumpCommitCrashPhase::AfterBaselines)?;
    }
    publish_observed_file(
        &transaction.join(STAGED_CDFI),
        &cdfi_target,
        &transaction.join(BACKUP_CDFI),
        cdfi_existed,
    )?;
    #[cfg(test)]
    if !baselines.is_empty() {
        inject_dump_crash(DumpCommitCrashPhase::AfterCdfi)?;
    }
    if baselines.is_empty() && requires_storage_claim(&prepared.observed_storage) {
        publish_absent_file(&transaction.join(STAGED_REDB), &redb_target)?;
    } else {
        // For dump commits redb stays at the old generation until this final CAS publication.
        publish_observed_file(
            &transaction.join(STAGED_REDB),
            &redb_target,
            &transaction.join(BACKUP_REDB),
            redb_existed,
        )?;
    }
    if let Some(state_dir) = redb_target.parent() {
        sync_directory(state_dir)?;
    }
    #[cfg(test)]
    if !baselines.is_empty() {
        inject_dump_crash(DumpCommitCrashPhase::AfterRedb)?;
        if FAIL_DUMP_COMMIT_AFTER_REDB.with(|slot| slot.replace(false)) {
            return Err(RuntimeStateError::TransactionIo(std::io::Error::other(
                "forced post-redb finalization failure",
            )));
        }
    }
    remove_baseline_ownership_markers(
        context,
        StateGeneration::new(next_generation),
        &journal_baselines,
    )?;
    write_journal(
        transaction,
        &StateJournal {
            status: JournalStatus::Committed,
            generation: next_generation,
            redb_existed,
            cdfi_existed,
            redb_staged,
            cdfi_staged,
            baselines: journal_baselines,
            dump_transaction_id: dump_transaction_id.cloned(),
        },
    )?;
    // Publication is committed at this point. Cleanup is deliberately best-effort;
    // a surviving committed journal is removed by the next recovery pass.
    fs::remove_dir_all(transaction)?;
    if let Some(parent) = transaction.parent() {
        sync_directory(parent)?;
    }
    Ok(StateGeneration::new(next_generation))
}

fn requires_storage_claim(observation: &ObservedStorageState) -> bool {
    matches!(
        observation,
        ObservedStorageState::ExistingUninitialized { .. }
            | ObservedStorageState::Initialized { .. }
            | ObservedStorageState::Recoverable { .. }
    )
}

fn publish_baseline(
    context: &SourceSetContext,
    generation: StateGeneration,
    transaction: &Path,
    baseline: &JournalBaseline,
) -> Result<(), RuntimeStateError> {
    let staged = transaction.join(&baseline.staged_name);
    let target = context.baseline(baseline.role.domain(), generation);
    let parent = target
        .path()
        .parent()
        .ok_or_else(|| RuntimeStateError::InvalidJournal {
            path: target.path().to_path_buf(),
            reason: "baseline target has no parent".to_owned(),
        })?;
    ensure_directory_synced(parent)?;
    rename_directory_no_replace(&staged, target.path())?;
    sync_rename_parents(&staged, target.path())
}

#[cfg(target_os = "macos")]
fn rename_directory_no_replace(source: &Path, target: &Path) -> Result<(), RuntimeStateError> {
    rename_directory_with_flags(source, target, libc::RENAME_EXCL)
}

#[cfg(target_os = "linux")]
fn rename_directory_no_replace(source: &Path, target: &Path) -> Result<(), RuntimeStateError> {
    rename_directory_with_flags(source, target, libc::RENAME_NOREPLACE)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn rename_directory_with_flags(
    source: &Path,
    target: &Path,
    flags: libc::c_uint,
) -> Result<(), RuntimeStateError> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let source_c = CString::new(source.as_os_str().as_bytes()).map_err(|_| {
        RuntimeStateError::InvalidJournal {
            path: source.to_path_buf(),
            reason: "staged baseline path contains NUL".to_owned(),
        }
    })?;
    let target_c = CString::new(target.as_os_str().as_bytes()).map_err(|_| {
        RuntimeStateError::InvalidJournal {
            path: target.to_path_buf(),
            reason: "baseline target path contains NUL".to_owned(),
        }
    })?;
    #[cfg(target_os = "macos")]
    // SAFETY: both values are NUL-free C strings valid for the duration of the syscall.
    let result = unsafe {
        libc::renameatx_np(
            libc::AT_FDCWD,
            source_c.as_ptr(),
            libc::AT_FDCWD,
            target_c.as_ptr(),
            flags,
        )
    };
    #[cfg(target_os = "linux")]
    // SAFETY: both values are NUL-free C strings valid for the duration of the syscall.
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            source_c.as_ptr(),
            libc::AT_FDCWD,
            target_c.as_ptr(),
            flags,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        let source_error = std::io::Error::last_os_error();
        if matches!(
            source_error.kind(),
            ErrorKind::AlreadyExists | ErrorKind::DirectoryNotEmpty
        ) {
            Err(RuntimeStateError::StorageObservationChanged {
                path: target.to_path_buf(),
            })
        } else {
            Err(runtime_io("publish staged baseline", target, source_error))
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn rename_directory_no_replace(source: &Path, target: &Path) -> Result<(), RuntimeStateError> {
    if target.try_exists()? {
        return Err(RuntimeStateError::StorageObservationChanged {
            path: target.to_path_buf(),
        });
    }
    fs::rename(source, target).map_err(|error| runtime_io("publish staged baseline", target, error))
}

enum BaselineOwnership {
    Owned,
    Missing,
    Foreign,
}

fn baseline_manifest_matches(
    path: &Path,
    expected: &FileFingerprint,
) -> Result<bool, RuntimeStateError> {
    match file_fingerprint(&path.join("manifest.json")) {
        Ok(actual) => Ok(actual == *expected),
        Err(RuntimeStateError::TransactionIo(error)) if error.kind() == ErrorKind::NotFound => {
            Ok(false)
        }
        Err(RuntimeStateError::Io { source, .. }) if source.kind() == ErrorKind::NotFound => {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

fn baseline_ownership(
    path: &Path,
    expected_token: &str,
) -> Result<BaselineOwnership, RuntimeStateError> {
    let marker = path.join(BASELINE_OWNERSHIP_FILE);
    match fs::read(&marker) {
        Ok(bytes) if bytes == expected_token.as_bytes() => Ok(BaselineOwnership::Owned),
        Ok(_) => Ok(BaselineOwnership::Foreign),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(BaselineOwnership::Missing),
        Err(error) => Err(runtime_io("read baseline ownership marker", &marker, error)),
    }
}

fn baseline_tree_matches(
    target: &Path,
    baseline: &JournalBaseline,
    marker_present: bool,
) -> Result<bool, RuntimeStateError> {
    let actual_identity = opened_file_identity(&fs::File::open(target)?)?;
    if baseline.directory_identity.is_none() || actual_identity != baseline.directory_identity {
        return Ok(false);
    }
    let mut root_entries = fs::read_dir(target)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<Result<Vec<_>, _>>()?;
    root_entries.sort();
    let mut expected_entries = vec![
        std::ffi::OsString::from("files"),
        std::ffi::OsString::from("manifest.json"),
    ];
    if marker_present {
        expected_entries.push(std::ffi::OsString::from(BASELINE_OWNERSHIP_FILE));
        expected_entries.sort();
    }
    if root_entries != expected_entries
        || !baseline_manifest_matches(target, &baseline.manifest_fingerprint)?
    {
        return Ok(false);
    }
    Ok(matches!(
        inspect_baseline_path(target)?,
        BaselineInspection::Valid(_)
    ))
}

fn baseline_directory_identity_matches(
    target: &Path,
    baseline: &JournalBaseline,
) -> Result<bool, RuntimeStateError> {
    let actual = opened_file_identity(&fs::File::open(target)?)?;
    Ok(baseline.directory_identity.is_some() && actual == baseline.directory_identity)
}

fn remove_baseline_ownership_markers(
    context: &SourceSetContext,
    generation: StateGeneration,
    baselines: &[JournalBaseline],
) -> Result<(), RuntimeStateError> {
    for baseline in baselines {
        let target = context.baseline(baseline.role.domain(), generation);
        finalize_baseline_marker(target.path(), baseline)?;
    }
    Ok(())
}

fn finalize_baseline_marker(
    target: &Path,
    baseline: &JournalBaseline,
) -> Result<(), RuntimeStateError> {
    let claim = target.with_file_name(format!(".baseline-finalize-{}", baseline.ownership_token));
    let working = if claim.try_exists()? {
        if target.try_exists()? {
            return Err(RuntimeStateError::InvalidJournal {
                path: target.to_path_buf(),
                reason: "foreign baseline appeared while finalization was claimed".to_owned(),
            });
        }
        claim.as_path()
    } else {
        match baseline_ownership(target, &baseline.ownership_token)? {
            BaselineOwnership::Missing
                if target.is_dir() && baseline_tree_matches(target, baseline, false)? =>
            {
                return Ok(())
            }
            BaselineOwnership::Owned => {
                rename_directory_no_replace(target, &claim)?;
                claim.as_path()
            }
            BaselineOwnership::Missing | BaselineOwnership::Foreign => {
                return Err(RuntimeStateError::InvalidJournal {
                    path: target.to_path_buf(),
                    reason: "published baseline ownership changed before commit".to_owned(),
                })
            }
        }
    };
    match baseline_ownership(working, &baseline.ownership_token)? {
        BaselineOwnership::Owned if baseline_tree_matches(working, baseline, true)? => {
            fs::remove_file(working.join(BASELINE_OWNERSHIP_FILE))?;
            sync_directory(working)?;
            #[cfg(test)]
            inject_dump_crash(DumpCommitCrashPhase::AfterBaselineMarkerRemoval)?;
        }
        BaselineOwnership::Missing if baseline_tree_matches(working, baseline, false)? => {}
        BaselineOwnership::Owned | BaselineOwnership::Missing | BaselineOwnership::Foreign => {
            if !target.try_exists()? {
                rename_directory_no_replace(working, target)?;
            }
            return Err(RuntimeStateError::InvalidJournal {
                path: target.to_path_buf(),
                reason: "published baseline content or identity changed before commit".to_owned(),
            });
        }
    }
    rename_directory_no_replace(working, target)
}

fn rollback_baselines(
    context: &SourceSetContext,
    generation: StateGeneration,
    baselines: &[JournalBaseline],
) -> Result<(), RuntimeStateError> {
    for baseline in baselines {
        let target = context.baseline(baseline.role.domain(), generation);
        let claim = target
            .path()
            .with_file_name(format!(".baseline-rollback-{}", baseline.ownership_token));
        if claim.try_exists()? {
            if !baseline_directory_identity_matches(&claim, baseline)? {
                return Err(RuntimeStateError::InvalidJournal {
                    path: claim,
                    reason: "foreign claimed baseline prevents safe rollback".to_owned(),
                });
            }
            fs::remove_dir_all(&claim)?;
            if let Some(parent) = claim.parent() {
                sync_directory(parent)?;
            }
            continue;
        }
        match fs::symlink_metadata(target.path()) {
            Ok(metadata) if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() => {
                match baseline_ownership(target.path(), &baseline.ownership_token)? {
                    BaselineOwnership::Owned
                        if baseline_tree_matches(target.path(), baseline, true)? => {}
                    BaselineOwnership::Owned => {
                        return Err(RuntimeStateError::InvalidJournal {
                            path: target.path().to_path_buf(),
                            reason: "foreign baseline content prevents safe rollback".to_owned(),
                        })
                    }
                    BaselineOwnership::Missing | BaselineOwnership::Foreign => {
                        return Err(RuntimeStateError::InvalidJournal {
                            path: target.path().to_path_buf(),
                            reason: "foreign baseline prevents safe rollback".to_owned(),
                        })
                    }
                }
                rename_directory_no_replace(target.path(), &claim)?;
                if !baseline_tree_matches(&claim, baseline, true)? {
                    rename_directory_no_replace(&claim, target.path())?;
                    return Err(RuntimeStateError::InvalidJournal {
                        path: target.path().to_path_buf(),
                        reason: "foreign baseline content prevents safe rollback".to_owned(),
                    });
                }
                #[cfg(test)]
                BEFORE_BASELINE_DESTRUCTIVE_HOOK.with(|slot| {
                    if let Some(hook) = slot.borrow_mut().take() {
                        hook();
                    }
                });
                fs::remove_dir_all(&claim)?;
                if let Some(parent) = claim.parent() {
                    sync_directory(parent)?;
                }
            }
            Ok(_) => {
                return Err(RuntimeStateError::InvalidJournal {
                    path: target.path().to_path_buf(),
                    reason: "foreign non-directory baseline prevents safe rollback".to_owned(),
                })
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(RuntimeStateError::TransactionIo(error)),
        }
    }
    Ok(())
}

fn prepare_redb_backup(
    target: &Path,
    backup: &Path,
    staged: &Path,
    observation: &ObservedStorageState,
) -> Result<bool, RuntimeStateError> {
    if requires_storage_claim(observation) {
        backup_regular_file(target, backup)?;
        #[cfg(test)]
        BEFORE_REDB_CLAIM_HOOK.with(|cell| {
            if let Some(hook) = cell.borrow_mut().take() {
                hook();
            }
        });
        let original_claim = transaction_original_claim_path(staged)?;
        fs::rename(target, &original_claim)
            .map_err(|error| runtime_io("claim observed hash storage", target, error))?;
        sync_rename_parents(target, &original_claim)?;
        if !file_fingerprint(backup)?.same_contents(&file_fingerprint(&original_claim)?) {
            restore_claimed_original(&original_claim, target)?;
            return Err(RuntimeStateError::StorageObservationChanged {
                path: target.to_path_buf(),
            });
        }
        let claimed_storage = HashStorage::new(original_claim.clone());
        let claimed = match observation {
            ObservedStorageState::ExistingUninitialized { .. } => {
                claimed_storage.uninitialized_observation()?
            }
            ObservedStorageState::Recoverable { .. } => {
                claimed_storage.recoverable_observation()?
            }
            ObservedStorageState::Initialized { generation } => {
                match claimed_storage.current_generation() {
                    Ok(actual) if actual == *generation => {
                        ObservedStorageState::Initialized { generation: actual }
                    }
                    Ok(_) => {
                        let failure = RuntimeStateError::StorageObservationChanged {
                            path: target.to_path_buf(),
                        };
                        return Err(restore_claimed_original_after_error(
                            &original_claim,
                            target,
                            failure,
                        ));
                    }
                    Err(error) => {
                        return Err(restore_claimed_original_after_error(
                            &original_claim,
                            target,
                            RuntimeStateError::Storage(error),
                        ));
                    }
                }
            }
            ObservedStorageState::MissingPath => {
                restore_claimed_original(&original_claim, target)?;
                return Err(RuntimeStateError::StorageObservationChanged {
                    path: target.to_path_buf(),
                });
            }
        };
        if &claimed == observation {
            fs::File::open(&original_claim)
                .and_then(|file| file.sync_all())
                .map_err(|error| runtime_io("sync claimed hash storage", &original_claim, error))?;
            return Ok(true);
        }
        restore_claimed_original(&original_claim, target)?;
        return Err(RuntimeStateError::StorageObservationChanged {
            path: target.to_path_buf(),
        });
    }
    backup_regular_file(target, backup)
}

fn restore_claimed_original(claim: &Path, target: &Path) -> Result<(), RuntimeStateError> {
    publish_absent_file(claim, target)
}

fn restore_claimed_original_after_error(
    claim: &Path,
    target: &Path,
    failure: RuntimeStateError,
) -> RuntimeStateError {
    match restore_claimed_original(claim, target) {
        Ok(()) => failure,
        Err(rollback) => RuntimeStateError::PublicationAndRollback {
            publication: Box::new(failure),
            rollback: Box::new(rollback),
            journal: claim.to_path_buf(),
        },
    }
}

fn backup_regular_file(target: &Path, backup: &Path) -> Result<bool, RuntimeStateError> {
    match fs::symlink_metadata(target) {
        Ok(metadata) if metadata.file_type().is_file() && !metadata.file_type().is_symlink() => {
            fs::copy(target, backup)?;
            fs::File::open(backup)?.sync_all()?;
            if let Some(parent) = backup.parent() {
                sync_directory(parent)?;
            }
            Ok(true)
        }
        Ok(_) => Err(RuntimeStateError::InvalidJournal {
            path: target.to_path_buf(),
            reason: "runtime-state target is not a regular file".to_owned(),
        }),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(RuntimeStateError::TransactionIo(error)),
    }
}

fn write_synced_file(path: &Path, bytes: &[u8]) -> Result<(), RuntimeStateError> {
    let mut file =
        fs::File::create(path).map_err(|error| runtime_io("create synced file", path, error))?;
    file.write_all(bytes)
        .map_err(|error| runtime_io("write synced file", path, error))?;
    file.sync_all()
        .map_err(|error| runtime_io("sync file", path, error))?;
    if let Some(parent) = path.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}

fn ensure_directory_synced(path: &Path) -> Result<(), RuntimeStateError> {
    let existed = path.try_exists()?;
    fs::create_dir_all(path)?;
    if !existed {
        if let Some(parent) = path.parent() {
            sync_directory(parent)?;
        }
    }
    Ok(())
}

fn publish_absent_file(staged: &Path, target: &Path) -> Result<(), RuntimeStateError> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    #[cfg(test)]
    if FORCE_REDB_PUBLISH_FAILURE.with(|forced| forced.replace(false)) {
        return Err(runtime_io(
            "publish staged file without overwriting",
            target,
            std::io::Error::other("forced redb publication failure"),
        ));
    }
    fs::hard_link(staged, target).map_err(|error| {
        if error.kind() == ErrorKind::AlreadyExists {
            RuntimeStateError::StorageObservationChanged {
                path: target.to_path_buf(),
            }
        } else {
            runtime_io("publish staged file without overwriting", target, error)
        }
    })?;
    if staged.parent() != target.parent() {
        if let Some(parent) = staged.parent() {
            sync_directory(parent)?;
        }
    }
    if let Some(parent) = target.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}

fn publish_observed_file(
    staged: &Path,
    target: &Path,
    backup: &Path,
    existed: bool,
) -> Result<(), RuntimeStateError> {
    if !existed {
        return publish_absent_file(staged, target);
    }
    let claimed_current = transaction_original_claim_path(staged)?;
    fs::rename(target, &claimed_current)
        .map_err(|error| runtime_io("claim observed live file for publication", target, error))?;
    sync_rename_parents(target, &claimed_current)?;
    if fs::read(&claimed_current)? != fs::read(backup)? {
        match publish_absent_file(&claimed_current, target) {
            Ok(()) | Err(RuntimeStateError::StorageObservationChanged { .. }) => {}
            Err(error) => return Err(error),
        }
        return Err(RuntimeStateError::StorageObservationChanged {
            path: target.to_path_buf(),
        });
    }
    match publish_absent_file(staged, target) {
        Ok(()) => Ok(()),
        Err(publication) => {
            let restoration = publish_absent_file(&claimed_current, target);
            match restoration {
                Ok(()) | Err(RuntimeStateError::StorageObservationChanged { .. }) => {
                    Err(publication)
                }
                Err(rollback) => Err(RuntimeStateError::PublicationAndRollback {
                    publication: Box::new(publication),
                    rollback: Box::new(rollback),
                    journal: claimed_current,
                }),
            }
        }
    }
}

fn sync_rename_parents(source: &Path, destination: &Path) -> Result<(), RuntimeStateError> {
    if let Some(parent) = destination.parent() {
        sync_directory(parent)?;
    }
    if destination.parent() != source.parent() {
        if let Some(parent) = source.parent() {
            sync_directory(parent)?;
        }
    }
    Ok(())
}

fn runtime_io(operation: &'static str, path: &Path, source: std::io::Error) -> RuntimeStateError {
    RuntimeStateError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    }
}

fn write_journal(transaction: &Path, journal: &StateJournal) -> Result<(), RuntimeStateError> {
    let path = transaction.join(JOURNAL_FILE);
    let temporary = transaction.join("journal.tmp");
    let bytes = serde_json::to_vec(journal).map_err(|error| RuntimeStateError::InvalidJournal {
        path: path.clone(),
        reason: error.to_string(),
    })?;
    let mut file = fs::File::create(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    fs::rename(temporary, path)?;
    sync_directory(transaction)?;
    Ok(())
}

fn validate_state_journal(
    journal: &StateJournal,
    journal_path: &Path,
) -> Result<(), RuntimeStateError> {
    let invalid = |reason: &str| RuntimeStateError::InvalidJournal {
        path: journal_path.to_path_buf(),
        reason: reason.to_owned(),
    };
    if journal.generation == 0 {
        return Err(invalid("generation must be nonzero"));
    }
    for (label, fingerprint) in [
        ("redb", &journal.redb_staged),
        ("cdfi", &journal.cdfi_staged),
    ] {
        validate_journal_fingerprint(fingerprint)
            .map_err(|reason| invalid(&format!("invalid {label} fingerprint: {reason}")))?;
    }
    if journal.status == JournalStatus::ClaimingRecoverable && !journal.baselines.is_empty() {
        return Err(invalid(
            "recoverable-claim journal cannot contain dump baselines",
        ));
    }
    if journal.baselines.is_empty() != journal.dump_transaction_id.is_none() {
        return Err(invalid(
            "dump transaction id must exist exactly for journals with dump baselines",
        ));
    }
    let mut roles = std::collections::BTreeSet::new();
    for baseline in &journal.baselines {
        if !roles.insert(baseline.role) {
            return Err(invalid("baseline roles must be unique"));
        }
        let expected_name = match baseline.role {
            JournalBaselineRole::ConfiguredSource => "new-baseline-configured-source",
            JournalBaselineRole::EdtPlatformDesigner => "new-baseline-edt-platform-designer",
        };
        if baseline.staged_name != expected_name {
            return Err(invalid("baseline staged name does not match its role"));
        }
        let mut components = Path::new(&baseline.ownership_token).components();
        if !matches!(components.next(), Some(std::path::Component::Normal(_)))
            || components.next().is_some()
            || !Uuid::parse_str(&baseline.ownership_token)
                .is_ok_and(|uuid| uuid.hyphenated().to_string() == baseline.ownership_token)
        {
            return Err(invalid(
                "baseline ownership token must be one canonical UUID component",
            ));
        }
        validate_journal_fingerprint(&baseline.manifest_fingerprint).map_err(|reason| {
            invalid(&format!("invalid baseline manifest fingerprint: {reason}"))
        })?;
    }
    Ok(())
}

fn validate_journal_fingerprint(fingerprint: &FileFingerprint) -> Result<(), &'static str> {
    if fingerprint.sha256.len() != 64
        || !fingerprint
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err("SHA-256 must be 64 lowercase hexadecimal characters");
    }
    Ok(())
}

fn recover_one_transaction(
    context: &SourceSetContext,
    transaction: &Path,
) -> Result<(), RuntimeStateError> {
    let journal_path = transaction.join(JOURNAL_FILE);
    let bytes = match fs::read(&journal_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            fs::remove_dir_all(transaction)?;
            if let Some(parent) = transaction.parent() {
                sync_directory(parent)?;
            }
            return Ok(());
        }
        Err(error) => return Err(RuntimeStateError::TransactionIo(error)),
    };
    let journal: StateJournal =
        serde_json::from_slice(&bytes).map_err(|error| RuntimeStateError::InvalidJournal {
            path: journal_path.clone(),
            reason: error.to_string(),
        })?;
    validate_state_journal(&journal, &journal_path)?;
    let generation = StateGeneration::new(journal.generation);
    let matching_generation_visible = if journal.generation == 0 {
        false
    } else {
        recovery_observation_matches(
            HashStorage::new(context.storage_path()).current_generation(),
            |actual| actual == journal.generation,
        )?
    };
    let matching_dump_transaction = match &journal.dump_transaction_id {
        Some(expected) => recovery_observation_matches(
            HashStorage::new(context.storage_path()).current_dump_transaction_id(),
            |actual| actual.as_ref() == Some(expected),
        )?,
        None => true,
    };
    let generation_visible = matching_generation_visible
        && matching_dump_transaction
        && same_file_identity(&context.storage_path(), &transaction.join(STAGED_REDB))?;
    if matching_generation_visible && !generation_visible {
        return Err(RuntimeStateError::InvalidJournal {
            path: context.storage_path(),
            reason: "foreign hash storage uses the pending generation".to_owned(),
        });
    }
    match journal.status {
        JournalStatus::ClaimingRecoverable => restore_transaction_target(
            &context.storage_path(),
            &transaction.join(BACKUP_REDB),
            &transaction.join(STAGED_REDB),
            true,
            &journal.redb_staged,
        )?,
        JournalStatus::Prepared if generation_visible => {
            if !same_file_identity(&context.private_cdfi_path(), &transaction.join(STAGED_CDFI))? {
                return Err(RuntimeStateError::InvalidJournal {
                    path: context.private_cdfi_path(),
                    reason: "foreign CDFI prevents visible-generation recovery".to_owned(),
                });
            }
            remove_baseline_ownership_markers(context, generation, &journal.baselines)?;
        }
        JournalStatus::Prepared => {
            #[cfg(test)]
            BEFORE_REDB_ROLLBACK_HOOK.with(|cell| {
                if let Some(hook) = cell.borrow_mut().take() {
                    hook();
                }
            });
            restore_transaction_target(
                &context.storage_path(),
                &transaction.join(BACKUP_REDB),
                &transaction.join(STAGED_REDB),
                journal.redb_existed,
                &journal.redb_staged,
            )?;
            restore_transaction_target(
                &context.private_cdfi_path(),
                &transaction.join(BACKUP_CDFI),
                &transaction.join(STAGED_CDFI),
                journal.cdfi_existed,
                &journal.cdfi_staged,
            )?;
            rollback_baselines(context, generation, &journal.baselines)?;
        }
        JournalStatus::Committed => {
            for baseline in &journal.baselines {
                let target = context.baseline(baseline.role.domain(), generation);
                finalize_baseline_marker(target.path(), baseline)?;
            }
        }
    }
    fs::remove_dir_all(transaction)?;
    if let Some(parent) = transaction.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}

fn recovery_observation_matches<T>(
    observation: Result<T, StorageError>,
    predicate: impl FnOnce(T) -> bool,
) -> Result<bool, RuntimeStateError> {
    match observation {
        Ok(value) => Ok(predicate(value)),
        Err(error) if error.is_recoverable() => Ok(false),
        Err(error) => Err(RuntimeStateError::Storage(error)),
    }
}

fn restore_transaction_target(
    target: &Path,
    backup: &Path,
    staged: &Path,
    existed: bool,
    staged_fingerprint: &FileFingerprint,
) -> Result<(), RuntimeStateError> {
    let original_claim = transaction_original_claim_path(staged)?;
    let published_claim = transaction_published_claim_path(staged)?;
    if target.try_exists()? {
        if published_claim.try_exists()? {
            return Ok(());
        }
        fs::rename(target, &published_claim).map_err(|error| {
            runtime_io("claim live file for rollback classification", target, error)
        })?;
        sync_rename_parents(target, &published_claim)?;
    }
    if published_claim.try_exists()? {
        let staged_is_valid =
            staged.try_exists()? && file_fingerprint(staged)? == *staged_fingerprint;
        let published_is_ours = staged_is_valid && same_file_identity(&published_claim, staged)?;
        if !published_is_ours {
            match publish_absent_file(&published_claim, target) {
                Ok(()) | Err(RuntimeStateError::StorageObservationChanged { .. }) => return Ok(()),
                Err(error) => return Err(error),
            }
        }
    }
    if existed && !target.try_exists()? {
        let backup_exists = backup.try_exists()?;
        let original_exists = original_claim.try_exists()?;
        let restore_source = if original_exists {
            original_claim.as_path()
        } else if backup_exists {
            backup
        } else {
            return Err(RuntimeStateError::InvalidJournal {
                path: backup.to_path_buf(),
                reason: "runtime-state backup is missing during rollback".to_owned(),
            });
        };
        publish_absent_file(restore_source, target)?;
    }
    Ok(())
}

fn transaction_original_claim_path(staged: &Path) -> Result<PathBuf, RuntimeStateError> {
    transaction_artifact_path(staged, ".original.claimed")
}

fn transaction_published_claim_path(staged: &Path) -> Result<PathBuf, RuntimeStateError> {
    transaction_artifact_path(staged, ".published.claimed")
}

fn transaction_artifact_path(staged: &Path, suffix: &str) -> Result<PathBuf, RuntimeStateError> {
    let file_name = staged
        .file_name()
        .ok_or_else(|| RuntimeStateError::InvalidJournal {
            path: staged.to_path_buf(),
            reason: "staged runtime-state path has no file name".to_owned(),
        })?;
    let mut claimed_name = file_name.to_os_string();
    claimed_name.push(suffix);
    Ok(staged.with_file_name(claimed_name))
}

fn file_fingerprint(path: &Path) -> Result<FileFingerprint, RuntimeStateError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(RuntimeStateError::InvalidJournal {
            path: path.to_path_buf(),
            reason: "runtime-state fingerprint target is not a regular file".to_owned(),
        });
    }
    let mut file = fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut len = 0_u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        len = len
            .checked_add(read as u64)
            .ok_or(RuntimeStateError::InvalidJournal {
                path: path.to_path_buf(),
                reason: "runtime-state file length overflowed u64".to_owned(),
            })?;
    }
    Ok(FileFingerprint {
        len,
        sha256: format!("{:x}", digest.finalize()),
        identity: opened_file_identity(&file)?,
    })
}

fn same_file_identity(left: &Path, right: &Path) -> Result<bool, RuntimeStateError> {
    let left_file = fs::File::open(left)?;
    let right_file = fs::File::open(right)?;
    let left_identity = opened_file_identity(&left_file)?;
    let right_identity = opened_file_identity(&right_file)?;
    Ok(left_identity.is_some() && left_identity == right_identity)
}

#[cfg(unix)]
fn opened_file_identity(file: &fs::File) -> Result<Option<FileIdentity>, RuntimeStateError> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    Ok(Some(FileIdentity {
        volume: metadata.dev(),
        index: metadata.ino(),
    }))
}

#[cfg(windows)]
fn opened_file_identity(file: &fs::File) -> Result<Option<FileIdentity>, RuntimeStateError> {
    windows_file_identity(file).map(Some)
}

#[cfg(not(any(unix, windows)))]
fn opened_file_identity(_file: &fs::File) -> Result<Option<FileIdentity>, RuntimeStateError> {
    Ok(None)
}

#[cfg(windows)]
fn windows_file_identity(file: &fs::File) -> Result<FileIdentity, RuntimeStateError> {
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };
    let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    // SAFETY: the handle comes from a live File and the API initializes the
    // output structure on success.
    let succeeded = unsafe {
        GetFileInformationByHandle(file.as_raw_handle().cast(), information.as_mut_ptr())
    };
    if succeeded == 0 {
        return Err(RuntimeStateError::TransactionIo(
            std::io::Error::last_os_error(),
        ));
    }
    // SAFETY: guarded by the successful Win32 call above.
    let information = unsafe { information.assume_init() };
    Ok(FileIdentity {
        volume: u64::from(information.dwVolumeSerialNumber),
        index: (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow),
    })
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), RuntimeStateError> {
    fs::File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), RuntimeStateError> {
    // Rust does not expose a portable directory-fsync primitive. File contents are
    // still synced, and the persisted journal remains available for recovery.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        commit_designer_state, commit_dump_state_with_lock, fail_next_dump_commit_after_redb,
        file_fingerprint, inspect_private_cdfi, lock_designer_state, prepare_redb_backup,
        recover_designer_state, set_dump_commit_crash_phase, transaction_original_claim_path,
        DumpCommitCrashPhase, DumpStateCommitRequest, JournalStatus, PrivateCdfiState,
        RuntimeStateError, StateJournal, AFTER_REDB_CLAIM_HOOK, BACKUP_CDFI, BACKUP_REDB,
        BASELINE_OWNERSHIP_FILE, BEFORE_BASELINE_DESTRUCTIVE_HOOK, BEFORE_REDB_CLAIM_HOOK,
        BEFORE_REDB_ROLLBACK_HOOK, FORCE_REDB_PUBLISH_FAILURE, JOURNAL_FILE, STAGED_CDFI,
        STAGED_REDB,
    };
    use crate::change_detection::analyzer::{PreparedFileState, PreparedStateUpdate};
    use crate::change_detection::hash_storage::ObservedStorageState;
    use crate::change_detection::hash_storage::{HashStorage, StoredFileState};
    use crate::config::model::{BuilderBackend, InfobaseConfig, SourceFormat, SourceSetPurpose};
    use crate::domain::runtime_state::{
        BaselineRole, DumpTransactionId, InfobaseIdentity, LogicalSourceRole,
        RuntimeSourceDescriptor, RuntimeSourceIdentityInputs, RuntimeStateLayout, StateGeneration,
    };
    use crate::domain::source_set::SourceSetContext;
    use std::collections::HashMap;
    use std::fs;
    use std::path::Path;
    use std::sync::mpsc;
    use std::time::Duration;
    use tempfile::tempdir;
    use uuid::Uuid;

    const VALID: &str = r#"<?xml version="1.0"?><ConfigDumpInfo version="2.17"><Metadata id="abc" configVersion="42"/></ConfigDumpInfo>"#;

    fn prepared_journal(
        transaction: &Path,
        redb_existed: bool,
        cdfi_existed: bool,
    ) -> StateJournal {
        StateJournal {
            status: JournalStatus::Prepared,
            generation: 99,
            redb_existed,
            cdfi_existed,
            redb_staged: file_fingerprint(&transaction.join(STAGED_REDB))
                .expect("staged redb fingerprint"),
            cdfi_staged: file_fingerprint(&transaction.join(STAGED_CDFI))
                .expect("staged CDFI fingerprint"),
            baselines: Vec::new(),
            dump_transaction_id: None,
        }
    }

    #[test]
    fn classifies_valid_missing_and_corrupt_private_cdfi() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("ConfigDumpInfo.xml");
        assert!(matches!(
            inspect_private_cdfi(&path).expect("missing"),
            PrivateCdfiState::Missing
        ));

        fs::write(&path, VALID).expect("valid");
        assert!(matches!(
            inspect_private_cdfi(&path).expect("valid"),
            PrivateCdfiState::Valid(_)
        ));

        fs::write(&path, "<Wrong version=\"\"></Wrong>").expect("invalid");
        assert!(matches!(
            inspect_private_cdfi(&path).expect("corrupt"),
            PrivateCdfiState::Corrupt(_)
        ));
    }

    #[test]
    fn rejects_missing_platform_owned_identity_values() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("ConfigDumpInfo.xml");
        fs::write(
            &path,
            r#"<ConfigDumpInfo version="2.17"><Metadata id="" configVersion="42"/></ConfigDumpInfo>"#,
        )
        .expect("invalid cdfi");
        assert!(matches!(
            inspect_private_cdfi(&path).expect("classification"),
            PrivateCdfiState::Corrupt(_)
        ));
    }

    #[test]
    fn rejects_truncated_cdfi_with_valid_identity_prefix() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("ConfigDumpInfo.xml");
        fs::write(
            &path,
            r#"<ConfigDumpInfo version="2.17"><Metadata id="abc" configVersion="42"/>"#,
        )
        .expect("truncated CDFI");

        assert!(matches!(
            inspect_private_cdfi(&path).expect("classification"),
            PrivateCdfiState::Corrupt(_)
        ));
    }

    #[test]
    fn rejects_non_whitespace_text_or_cdata_outside_root() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("ConfigDumpInfo.xml");
        for xml in [
            format!("prefix{VALID}"),
            format!("{VALID}suffix"),
            format!("<![CDATA[prefix]]>{VALID}"),
        ] {
            fs::write(&path, xml).expect("invalid CDFI");
            assert!(matches!(
                inspect_private_cdfi(&path).expect("classification"),
                PrivateCdfiState::Corrupt(_)
            ));
        }
    }

    #[test]
    fn rejects_whitespace_only_decoded_identity_attributes() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("ConfigDumpInfo.xml");
        for xml in [
            r#"<ConfigDumpInfo version="&#x20;"><Metadata id="abc" configVersion="42"/></ConfigDumpInfo>"#,
            r#"<ConfigDumpInfo version="2.17"><Metadata id="&#x20;" configVersion="42"/></ConfigDumpInfo>"#,
            r#"<ConfigDumpInfo version="2.17"><Metadata id="abc" configVersion="&#x9;"/></ConfigDumpInfo>"#,
        ] {
            fs::write(&path, xml).expect("invalid CDFI");
            assert!(matches!(
                inspect_private_cdfi(&path).expect("classification"),
                PrivateCdfiState::Corrupt(_)
            ));
        }
    }

    #[test]
    fn accepts_bom_and_namespaced_cdfi_fixture() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("ConfigDumpInfo.xml");
        fs::write(
            &path,
            b"\xEF\xBB\xBF<?xml version=\"1.0\"?><v8:ConfigDumpInfo xmlns:v8=\"urn:v8\" version=\"2.17\"><v8:Metadata id=\"fixture-id\" configVersion=\"9\"/></v8:ConfigDumpInfo>",
        )
        .expect("fixture");

        assert!(matches!(
            inspect_private_cdfi(&path).expect("fixture state"),
            PrivateCdfiState::Valid(_)
        ));
    }

    #[test]
    fn accepts_repository_designer_cdfi_fixture() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("ConfigDumpInfo.xml");
        fs::write(
            &path,
            include_bytes!("../../tests/fixtures/designer/configuration/ConfigDumpInfo.xml"),
        )
        .expect("fixture");

        assert!(matches!(
            inspect_private_cdfi(&path).expect("fixture state"),
            PrivateCdfiState::Valid(_)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_and_directory_are_corrupt_private_state() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().expect("tempdir");
        let target = dir.path().join("target.xml");
        fs::write(&target, VALID).expect("target");
        let symlink_path = dir.path().join("ConfigDumpInfo.xml");
        symlink(&target, &symlink_path).expect("symlink");
        assert!(matches!(
            inspect_private_cdfi(&symlink_path).expect("symlink state"),
            PrivateCdfiState::Corrupt(_)
        ));

        fs::remove_file(&symlink_path).expect("remove symlink");
        fs::create_dir(&symlink_path).expect("directory");
        assert!(matches!(
            inspect_private_cdfi(&symlink_path).expect("directory state"),
            PrivateCdfiState::Corrupt(_)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn surfaces_private_cdfi_permission_errors_as_hard_failures() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("ConfigDumpInfo.xml");
        fs::write(&path, VALID).expect("CDFI");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).expect("deny read");
        let result = inspect_private_cdfi(&path);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("restore read");

        assert!(result.is_err());
    }

    fn context(root: &Path) -> SourceSetContext {
        let source = root.join("source");
        fs::create_dir_all(&source).expect("source");
        let identity = InfobaseIdentity::normalize(&InfobaseConfig::file(format!(
            "File={}",
            root.join("ib").display()
        )))
        .expect("identity");
        let layout = RuntimeStateLayout::new(root.join("work"), identity).expect("layout");
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

    fn seed_state(context: &SourceSetContext, cdfi: &str) {
        let mut snapshot = HashMap::new();
        snapshot.insert(
            "Configuration.xml".to_owned(),
            StoredFileState {
                mtime_ns: 1,
                hash: "a".repeat(64),
            },
        );
        HashStorage::new(context.storage_path())
            .commit_snapshot(&snapshot, 1, 0)
            .expect("seed storage");
        fs::write(context.private_cdfi_path(), cdfi).expect("seed CDFI");
    }

    fn prepared(observed_generation: u64) -> PreparedStateUpdate {
        PreparedStateUpdate {
            snapshot: vec![PreparedFileState {
                rel_path: "Configuration.xml".to_owned(),
                mtime_ns: 2,
                hash: "b".repeat(64),
            }],
            scan_started_at: 2,
            observed_storage: ObservedStorageState::Initialized {
                generation: observed_generation,
            },
        }
    }

    fn dump_roots(root: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
        let configured = root.join("configured-shadow");
        let edt = root.join("edt-platform-shadow");
        fs::create_dir_all(&configured).expect("configured shadow");
        fs::create_dir_all(&edt).expect("EDT platform shadow");
        fs::write(configured.join("Configuration.xml"), b"configured-v2").expect("configured file");
        fs::write(configured.join("ConfigDumpInfo.xml"), b"must be excluded")
            .expect("configured CDFI sentinel");
        fs::write(edt.join("Configuration.xml"), b"edt-platform-v2").expect("EDT file");
        (configured, edt)
    }

    fn malicious_baseline_journal(
        transaction: &Path,
        staged_name: String,
        ownership_token: String,
    ) -> StateJournal {
        fs::write(transaction.join(STAGED_REDB), b"staged-redb").expect("staged redb");
        fs::write(transaction.join(STAGED_CDFI), b"staged-cdfi").expect("staged cdfi");
        let mut journal = prepared_journal(transaction, false, false);
        journal.generation = 2;
        journal.baselines.push(super::JournalBaseline {
            role: super::JournalBaselineRole::ConfiguredSource,
            staged_name,
            ownership_token,
            manifest_fingerprint: file_fingerprint(&transaction.join(STAGED_CDFI))
                .expect("fingerprint"),
            directory_identity: None,
        });
        journal
    }

    #[test]
    fn recovery_rejects_unbounded_baseline_journal_fields_before_mutation() {
        for (staged_name, ownership_token) in [
            (
                "/tmp/v8-runner-external-baseline".to_owned(),
                Uuid::new_v4().to_string(),
            ),
            (
                "../external-baseline".to_owned(),
                Uuid::new_v4().to_string(),
            ),
            (
                "new-baseline-configured-source".to_owned(),
                "../external-token".to_owned(),
            ),
        ] {
            let dir = tempdir().expect("tempdir");
            let context = context(dir.path());
            let transaction = context.transactions_dir().join("state-malicious");
            fs::create_dir_all(&transaction).expect("transaction");
            let sentinel = dir.path().join("external-sentinel");
            fs::write(&sentinel, b"preserve").expect("sentinel");
            let journal = malicious_baseline_journal(&transaction, staged_name, ownership_token);
            fs::write(
                transaction.join(JOURNAL_FILE),
                serde_json::to_vec(&journal).expect("journal"),
            )
            .expect("write journal");

            recover_designer_state(&context).expect_err("invalid journal must fail closed");

            assert_eq!(fs::read(&sentinel).expect("sentinel"), b"preserve");
            assert!(transaction.exists());
        }
    }

    #[test]
    fn recovery_rejects_unknown_journal_fields_before_mutation() {
        let dir = tempdir().expect("tempdir");
        let context = context(dir.path());
        let transaction = context.transactions_dir().join("state-unknown-field");
        fs::create_dir_all(&transaction).expect("transaction");
        fs::write(transaction.join(STAGED_REDB), b"staged-redb").expect("staged redb");
        fs::write(transaction.join(STAGED_CDFI), b"staged-cdfi").expect("staged cdfi");
        let mut value = serde_json::to_value(prepared_journal(&transaction, false, false))
            .expect("journal value");
        value
            .as_object_mut()
            .expect("journal object")
            .insert("unknown".to_owned(), serde_json::json!(true));
        fs::write(
            transaction.join(JOURNAL_FILE),
            serde_json::to_vec(&value).expect("journal"),
        )
        .expect("write journal");

        recover_designer_state(&context).expect_err("unknown field must fail closed");

        assert!(transaction.exists());
    }

    #[test]
    fn dump_commit_publishes_both_baselines_cdfi_and_observation_as_one_generation() {
        let dir = tempdir().expect("tempdir");
        let context = context(dir.path());
        seed_state(&context, VALID);
        let produced = dir.path().join("produced.xml");
        let next = r#"<ConfigDumpInfo version="2.17"><Metadata id="dump" configVersion="2"/></ConfigDumpInfo>"#;
        fs::write(&produced, next).expect("produced CDFI");
        let (configured, edt) = dump_roots(dir.path());
        let lock = lock_designer_state(&context).expect("lock");
        let transaction_id = DumpTransactionId::new();

        let generation = commit_dump_state_with_lock(
            &context,
            &lock,
            DumpStateCommitRequest::new(&prepared(1), &configured, &produced)
                .with_edt_platform_designer(&edt)
                .with_transaction_id(transaction_id.clone()),
        )
        .expect("dump state commit");

        assert_eq!(generation, StateGeneration::new(2));
        assert_eq!(
            HashStorage::new(context.storage_path())
                .current_dump_transaction_id()
                .expect("dump transaction"),
            Some(transaction_id)
        );
        assert_eq!(
            HashStorage::new(context.storage_path())
                .current_generation()
                .expect("generation"),
            2
        );
        assert_eq!(
            fs::read_to_string(context.private_cdfi_path()).unwrap(),
            next
        );
        for role in [
            BaselineRole::ConfiguredSource,
            BaselineRole::EdtPlatformDesigner,
        ] {
            let baseline = context.baseline(role, generation);
            assert!(baseline.path().join("manifest.json").is_file());
            assert!(!baseline.path().join("files/ConfigDumpInfo.xml").exists());
        }
    }

    #[test]
    fn dump_commit_returns_visible_generation_after_forward_recovery() {
        let dir = tempdir().expect("tempdir");
        let context = context(dir.path());
        seed_state(&context, VALID);
        let produced = dir.path().join("produced.xml");
        fs::write(
            &produced,
            r#"<ConfigDumpInfo version="2.17"><Metadata id="recovered" configVersion="2"/></ConfigDumpInfo>"#,
        )
        .expect("produced CDFI");
        let (configured, edt) = dump_roots(dir.path());
        let lock = lock_designer_state(&context).expect("lock");
        fail_next_dump_commit_after_redb();

        let generation = commit_dump_state_with_lock(
            &context,
            &lock,
            DumpStateCommitRequest::new(&prepared(1), &configured, &produced)
                .with_edt_platform_designer(&edt),
        )
        .expect("forward recovery made the next generation coherent");

        assert_eq!(generation, StateGeneration::new(2));
        assert_eq!(
            HashStorage::new(context.storage_path())
                .current_generation()
                .expect("visible generation"),
            2
        );
    }

    #[test]
    fn dump_commit_recovery_never_exposes_a_mixed_generation() {
        for phase in [
            DumpCommitCrashPhase::AfterBaselines,
            DumpCommitCrashPhase::AfterCdfi,
            DumpCommitCrashPhase::AfterRedb,
        ] {
            let dir = tempdir().expect("tempdir");
            let context = context(dir.path());
            seed_state(&context, VALID);
            let produced = dir.path().join("produced.xml");
            fs::write(
                &produced,
                r#"<ConfigDumpInfo version="2.17"><Metadata id="crash" configVersion="2"/></ConfigDumpInfo>"#,
            )
            .expect("produced CDFI");
            let (configured, edt) = dump_roots(dir.path());
            let lock = lock_designer_state(&context).expect("lock");
            set_dump_commit_crash_phase(phase);

            commit_dump_state_with_lock(
                &context,
                &lock,
                DumpStateCommitRequest::new(&prepared(1), &configured, &produced)
                    .with_edt_platform_designer(&edt),
            )
            .expect_err("injected crash");
            drop(lock);
            recover_designer_state(&context).expect("restart recovery");

            let observed = HashStorage::new(context.storage_path())
                .current_generation()
                .expect("generation after recovery");
            let next = StateGeneration::new(2);
            if phase == DumpCommitCrashPhase::AfterRedb {
                assert_eq!(observed, 2);
                assert!(context
                    .baseline(BaselineRole::ConfiguredSource, next)
                    .path()
                    .is_dir());
                assert!(context
                    .baseline(BaselineRole::EdtPlatformDesigner, next)
                    .path()
                    .is_dir());
                assert!(fs::read_to_string(context.private_cdfi_path())
                    .expect("new CDFI")
                    .contains("id=\"crash\""));
            } else {
                assert_eq!(observed, 1);
                assert!(!context
                    .baseline(BaselineRole::ConfiguredSource, next)
                    .path()
                    .exists());
                assert!(!context
                    .baseline(BaselineRole::EdtPlatformDesigner, next)
                    .path()
                    .exists());
                assert_eq!(
                    fs::read_to_string(context.private_cdfi_path()).unwrap(),
                    VALID
                );
            }
        }
    }

    #[test]
    fn dump_commit_redb_failure_rolls_back_all_previously_published_artifacts() {
        let dir = tempdir().expect("tempdir");
        let context = context(dir.path());
        seed_state(&context, VALID);
        let produced = dir.path().join("produced.xml");
        fs::write(
            &produced,
            r#"<ConfigDumpInfo version="2.17"><Metadata id="failed" configVersion="2"/></ConfigDumpInfo>"#,
        )
        .expect("produced CDFI");
        let (configured, edt) = dump_roots(dir.path());
        let lock = lock_designer_state(&context).expect("lock");
        FORCE_REDB_PUBLISH_FAILURE.with(|forced| forced.set(true));

        commit_dump_state_with_lock(
            &context,
            &lock,
            DumpStateCommitRequest::new(&prepared(1), &configured, &produced)
                .with_edt_platform_designer(&edt),
        )
        .expect_err("forced redb failure");

        assert_eq!(
            HashStorage::new(context.storage_path())
                .current_generation()
                .expect("old generation"),
            1
        );
        assert_eq!(
            fs::read_to_string(context.private_cdfi_path()).unwrap(),
            VALID
        );
        let next = StateGeneration::new(2);
        assert!(!context
            .baseline(BaselineRole::ConfiguredSource, next)
            .path()
            .exists());
        assert!(!context
            .baseline(BaselineRole::EdtPlatformDesigner, next)
            .path()
            .exists());
    }

    #[test]
    fn recovery_never_deletes_a_foreign_baseline() {
        let dir = tempdir().expect("tempdir");
        let context = context(dir.path());
        seed_state(&context, VALID);
        let produced = dir.path().join("produced.xml");
        fs::write(
            &produced,
            r#"<ConfigDumpInfo version="2.17"><Metadata id="foreign" configVersion="2"/></ConfigDumpInfo>"#,
        )
        .expect("produced CDFI");
        let (configured, _) = dump_roots(dir.path());
        let lock = lock_designer_state(&context).expect("lock");
        set_dump_commit_crash_phase(DumpCommitCrashPhase::AfterBaselines);
        commit_dump_state_with_lock(
            &context,
            &lock,
            DumpStateCommitRequest::new(&prepared(1), &configured, &produced),
        )
        .expect_err("injected crash");
        drop(lock);
        let baseline = context.baseline(BaselineRole::ConfiguredSource, StateGeneration::new(2));
        fs::write(
            baseline.path().join(BASELINE_OWNERSHIP_FILE),
            b"foreign-owner",
        )
        .expect("replace ownership marker");

        recover_designer_state(&context).expect_err("foreign baseline blocks rollback");

        assert!(baseline.path().is_dir());
        assert_eq!(
            fs::read(baseline.path().join(BASELINE_OWNERSHIP_FILE)).unwrap(),
            b"foreign-owner"
        );
        assert_eq!(
            HashStorage::new(context.storage_path())
                .current_generation()
                .expect("old generation"),
            1
        );
    }

    #[test]
    fn recovery_never_deletes_foreign_content_added_to_an_owned_baseline() {
        let dir = tempdir().expect("tempdir");
        let context = context(dir.path());
        seed_state(&context, VALID);
        let produced = dir.path().join("produced.xml");
        fs::write(&produced, VALID).expect("produced CDFI");
        let (configured, _) = dump_roots(dir.path());
        let lock = lock_designer_state(&context).expect("lock");
        set_dump_commit_crash_phase(DumpCommitCrashPhase::AfterBaselines);
        commit_dump_state_with_lock(
            &context,
            &lock,
            DumpStateCommitRequest::new(&prepared(1), &configured, &produced),
        )
        .expect_err("injected crash");
        drop(lock);
        let baseline = context.baseline(BaselineRole::ConfiguredSource, StateGeneration::new(2));
        fs::write(baseline.path().join("foreign.txt"), b"foreign").expect("foreign file");

        recover_designer_state(&context).expect_err("foreign baseline content blocks rollback");

        assert_eq!(
            fs::read(baseline.path().join("foreign.txt")).unwrap(),
            b"foreign"
        );
        assert!(baseline.path().is_dir());
    }

    #[test]
    fn recovery_never_deletes_an_owned_baseline_with_modified_managed_content() {
        let dir = tempdir().expect("tempdir");
        let context = context(dir.path());
        seed_state(&context, VALID);
        let produced = dir.path().join("produced.xml");
        fs::write(&produced, VALID).expect("produced CDFI");
        let (configured, _) = dump_roots(dir.path());
        let lock = lock_designer_state(&context).expect("lock");
        set_dump_commit_crash_phase(DumpCommitCrashPhase::AfterBaselines);
        commit_dump_state_with_lock(
            &context,
            &lock,
            DumpStateCommitRequest::new(&prepared(1), &configured, &produced),
        )
        .expect_err("injected crash");
        drop(lock);
        let baseline = context.baseline(BaselineRole::ConfiguredSource, StateGeneration::new(2));
        let managed = baseline.path().join("files/Configuration.xml");
        fs::write(&managed, b"foreign").expect("modify managed baseline file");

        recover_designer_state(&context).expect_err("modified baseline blocks rollback");

        assert_eq!(fs::read(managed).unwrap(), b"foreign");
        assert!(baseline.path().is_dir());
    }

    #[test]
    fn recovery_never_deletes_a_replaced_baseline_with_a_copied_marker() {
        let dir = tempdir().expect("tempdir");
        let context = context(dir.path());
        seed_state(&context, VALID);
        let produced = dir.path().join("produced.xml");
        fs::write(&produced, VALID).expect("produced CDFI");
        let (configured, _) = dump_roots(dir.path());
        let lock = lock_designer_state(&context).expect("lock");
        set_dump_commit_crash_phase(DumpCommitCrashPhase::AfterBaselines);
        commit_dump_state_with_lock(
            &context,
            &lock,
            DumpStateCommitRequest::new(&prepared(1), &configured, &produced),
        )
        .expect_err("injected crash");
        drop(lock);
        let baseline = context.baseline(BaselineRole::ConfiguredSource, StateGeneration::new(2));
        let displaced = baseline.path().with_extension("displaced");
        fs::rename(baseline.path(), &displaced).expect("displace owned baseline");
        fs::create_dir_all(baseline.path().join("files")).expect("replacement baseline");
        for relative in [
            "manifest.json",
            "files/Configuration.xml",
            BASELINE_OWNERSHIP_FILE,
        ] {
            fs::copy(displaced.join(relative), baseline.path().join(relative))
                .expect("copy baseline artifact");
        }

        recover_designer_state(&context).expect_err("replaced baseline blocks rollback");

        assert!(baseline.path().is_dir());
        assert!(baseline.path().join("files/Configuration.xml").is_file());
    }

    #[test]
    fn rollback_claims_owned_baseline_before_a_foreign_replacement_can_appear() {
        let dir = tempdir().expect("tempdir");
        let context = context(dir.path());
        seed_state(&context, VALID);
        let produced = dir.path().join("produced.xml");
        fs::write(&produced, VALID).expect("produced CDFI");
        let (configured, _) = dump_roots(dir.path());
        let lock = lock_designer_state(&context).expect("lock");
        set_dump_commit_crash_phase(DumpCommitCrashPhase::AfterBaselines);
        commit_dump_state_with_lock(
            &context,
            &lock,
            DumpStateCommitRequest::new(&prepared(1), &configured, &produced),
        )
        .expect_err("injected crash");
        drop(lock);
        let baseline = context.baseline(BaselineRole::ConfiguredSource, StateGeneration::new(2));
        let target = baseline.path().to_path_buf();
        let displaced = target.with_extension("hook-displaced");
        BEFORE_BASELINE_DESTRUCTIVE_HOOK.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                if target.exists() {
                    fs::rename(&target, displaced).expect("displace checked baseline");
                }
                fs::create_dir_all(&target).expect("foreign replacement");
                fs::write(target.join("foreign.txt"), b"foreign").expect("foreign content");
            }));
        });

        recover_designer_state(&context).expect("owned claim rollback");

        assert_eq!(
            fs::read(baseline.path().join("foreign.txt")).unwrap(),
            b"foreign"
        );
    }

    #[test]
    fn recovery_finishes_baseline_marker_removal_after_crash() {
        let dir = tempdir().expect("tempdir");
        let context = context(dir.path());
        seed_state(&context, VALID);
        let produced = dir.path().join("produced.xml");
        fs::write(&produced, VALID).expect("produced CDFI");
        let (configured, _) = dump_roots(dir.path());
        let lock = lock_designer_state(&context).expect("lock");
        set_dump_commit_crash_phase(DumpCommitCrashPhase::AfterBaselineMarkerRemoval);

        commit_dump_state_with_lock(
            &context,
            &lock,
            DumpStateCommitRequest::new(&prepared(1), &configured, &produced),
        )
        .expect_err("injected marker-removal crash");
        drop(lock);
        recover_designer_state(&context).expect("restart recovery");

        let baseline = context.baseline(BaselineRole::ConfiguredSource, StateGeneration::new(2));
        assert!(baseline.path().is_dir());
        assert!(!baseline.path().join(BASELINE_OWNERSHIP_FILE).exists());
    }

    #[test]
    fn recovery_finishes_partially_removed_owned_rollback_claim() {
        let dir = tempdir().expect("tempdir");
        let context = context(dir.path());
        seed_state(&context, VALID);
        let produced = dir.path().join("produced.xml");
        fs::write(&produced, VALID).expect("produced CDFI");
        let (configured, _) = dump_roots(dir.path());
        let lock = lock_designer_state(&context).expect("lock");
        set_dump_commit_crash_phase(DumpCommitCrashPhase::AfterBaselines);
        commit_dump_state_with_lock(
            &context,
            &lock,
            DumpStateCommitRequest::new(&prepared(1), &configured, &produced),
        )
        .expect_err("injected baseline crash");
        drop(lock);
        let baseline = context.baseline(BaselineRole::ConfiguredSource, StateGeneration::new(2));
        let token = String::from_utf8(
            fs::read(baseline.path().join(BASELINE_OWNERSHIP_FILE)).expect("ownership token"),
        )
        .expect("utf8 token");
        let claim = baseline
            .path()
            .with_file_name(format!(".baseline-rollback-{token}"));
        fs::rename(baseline.path(), &claim).expect("persist rollback claim");
        fs::remove_file(claim.join("files/Configuration.xml")).expect("partial cleanup");

        recover_designer_state(&context).expect("resume partial rollback cleanup");

        assert!(!claim.exists());
        assert!(!baseline.path().exists());
    }

    #[test]
    fn dump_commit_does_not_replace_an_empty_foreign_baseline_directory() {
        let dir = tempdir().expect("tempdir");
        let context = context(dir.path());
        seed_state(&context, VALID);
        let produced = dir.path().join("produced.xml");
        fs::write(&produced, VALID).expect("produced CDFI");
        let (configured, _) = dump_roots(dir.path());
        let foreign = context.baseline(BaselineRole::ConfiguredSource, StateGeneration::new(2));
        fs::create_dir_all(foreign.path()).expect("empty foreign baseline");
        let lock = lock_designer_state(&context).expect("lock");

        commit_dump_state_with_lock(
            &context,
            &lock,
            DumpStateCommitRequest::new(&prepared(1), &configured, &produced),
        )
        .expect_err("foreign target must reject publication");

        assert!(foreign.path().is_dir());
        assert_eq!(fs::read_dir(foreign.path()).unwrap().count(), 0);
        assert_eq!(
            HashStorage::new(context.storage_path())
                .current_generation()
                .expect("old generation"),
            1
        );
    }

    #[test]
    fn successful_commit_publishes_cdfi_and_hash_generation_together() {
        let dir = tempdir().expect("tempdir");
        let context = context(dir.path());
        seed_state(&context, VALID);
        let produced = dir.path().join("produced.xml");
        let next = r#"<ConfigDumpInfo version="2.17"><Metadata id="next" configVersion="2"/></ConfigDumpInfo>"#;
        fs::write(&produced, next).expect("produced");

        commit_designer_state(&context, &prepared(1), &produced).expect("commit");

        assert_eq!(
            HashStorage::new(context.storage_path())
                .load_snapshot()
                .expect("snapshot")
                .generation,
            2
        );
        assert_eq!(
            fs::read_to_string(context.private_cdfi_path()).expect("CDFI"),
            next
        );
    }

    #[test]
    fn recoverable_claim_preserves_concurrently_repaired_storage() {
        let dir = tempdir().expect("tempdir");
        let context = context(dir.path());
        let storage_path = context.storage_path();
        fs::create_dir_all(storage_path.parent().expect("state dir")).expect("state dir");
        fs::write(&storage_path, b"corrupt-a").expect("corrupt A");
        fs::write(context.private_cdfi_path(), VALID).expect("private CDFI");
        let observed = HashStorage::new(storage_path.clone())
            .recoverable_observation()
            .expect("recoverable token");

        let healthy_path = dir.path().join("healthy.redb");
        let mut snapshot = HashMap::new();
        snapshot.insert(
            "Configuration.xml".to_owned(),
            StoredFileState {
                mtime_ns: 7,
                hash: "c".repeat(64),
            },
        );
        HashStorage::create_replacement(healthy_path.clone(), &snapshot, 7, 9).expect("healthy B");
        let healthy_bytes = fs::read(&healthy_path).expect("healthy bytes");
        let target_for_hook = storage_path.clone();
        BEFORE_REDB_CLAIM_HOOK.with(|cell| {
            *cell.borrow_mut() = Some(Box::new(move || {
                fs::remove_file(&target_for_hook).expect("remove corrupt A");
                fs::rename(&healthy_path, &target_for_hook).expect("publish healthy B");
            }));
        });
        let produced = dir.path().join("produced.xml");
        fs::write(&produced, VALID).expect("produced CDFI");
        let mut update = prepared(0);
        update.observed_storage = observed;

        commit_designer_state(&context, &update, &produced)
            .expect_err("changed storage must reject publication");
        assert_eq!(
            fs::read(&storage_path).expect("healthy storage retained"),
            healthy_bytes
        );
        assert_eq!(
            HashStorage::new(storage_path)
                .load_snapshot()
                .expect("healthy storage")
                .generation,
            9
        );
    }

    #[test]
    fn initialized_claim_preserves_concurrent_in_place_commit() {
        let dir = tempdir().expect("tempdir");
        let context = context(dir.path());
        seed_state(&context, VALID);
        let target = context.storage_path();
        BEFORE_REDB_CLAIM_HOOK.with(|cell| {
            *cell.borrow_mut() = Some(Box::new(move || {
                HashStorage::new(target)
                    .commit_snapshot(&HashMap::new(), 2, 1)
                    .expect("concurrent in-place commit");
            }));
        });
        let produced = dir.path().join("produced.xml");
        fs::write(&produced, VALID).expect("produced CDFI");

        commit_designer_state(&context, &prepared(1), &produced)
            .expect_err("concurrent generation must win");

        assert_eq!(
            HashStorage::new(context.storage_path())
                .current_generation()
                .expect("concurrent generation retained"),
            2
        );
    }

    #[test]
    fn recoverable_publication_preserves_database_published_after_claim() {
        let dir = tempdir().expect("tempdir");
        let context = context(dir.path());
        let storage_path = context.storage_path();
        fs::create_dir_all(storage_path.parent().expect("state dir")).expect("state dir");
        fs::write(&storage_path, b"corrupt-a").expect("corrupt A");
        fs::write(context.private_cdfi_path(), VALID).expect("private CDFI");
        let observed = HashStorage::new(storage_path.clone())
            .recoverable_observation()
            .expect("recoverable token");
        let healthy = dir.path().join("healthy-after-claim.redb");
        HashStorage::create_replacement(healthy.clone(), &HashMap::new(), 7, 9).expect("healthy B");
        let healthy_bytes = fs::read(&healthy).expect("healthy bytes");
        let target = storage_path.clone();
        AFTER_REDB_CLAIM_HOOK.with(|cell| {
            *cell.borrow_mut() = Some(Box::new(move || {
                fs::hard_link(&healthy, &target).expect("publish B after claim");
            }));
        });
        let produced = dir.path().join("produced.xml");
        fs::write(&produced, VALID).expect("produced CDFI");
        let mut update = prepared(0);
        update.observed_storage = observed;

        commit_designer_state(&context, &update, &produced)
            .expect_err("concurrent B must reject publication");

        assert_eq!(fs::read(storage_path).expect("preserved B"), healthy_bytes);
    }

    #[test]
    fn recoverable_rollback_preserves_database_published_before_restore() {
        let dir = tempdir().expect("tempdir");
        let context = context(dir.path());
        let storage_path = context.storage_path();
        fs::create_dir_all(storage_path.parent().expect("state dir")).expect("state dir");
        fs::write(&storage_path, b"corrupt-a").expect("corrupt A");
        fs::write(context.private_cdfi_path(), VALID).expect("private CDFI");
        let observed = HashStorage::new(storage_path.clone())
            .recoverable_observation()
            .expect("recoverable token");
        let healthy = dir.path().join("healthy-before-rollback.redb");
        HashStorage::create_replacement(healthy.clone(), &HashMap::new(), 7, 9).expect("healthy B");
        let healthy_bytes = fs::read(&healthy).expect("healthy bytes");
        let target = storage_path.clone();
        FORCE_REDB_PUBLISH_FAILURE.with(|forced| forced.set(true));
        BEFORE_REDB_ROLLBACK_HOOK.with(|cell| {
            *cell.borrow_mut() = Some(Box::new(move || {
                fs::hard_link(&healthy, &target).expect("publish B before rollback");
            }));
        });
        let produced = dir.path().join("produced.xml");
        fs::write(&produced, VALID).expect("produced CDFI");
        let mut update = prepared(0);
        update.observed_storage = observed;

        commit_designer_state(&context, &update, &produced)
            .expect_err("forced publication failure");

        assert_eq!(fs::read(storage_path).expect("preserved B"), healthy_bytes);
    }

    #[test]
    fn recovery_waits_while_another_owner_holds_source_state_lock() {
        let dir = tempdir().expect("tempdir");
        let context = context(dir.path());
        let lock = lock_designer_state(&context).expect("first lock");
        let (sender, receiver) = mpsc::channel();
        let worker_context = context.clone();
        let worker = std::thread::spawn(move || {
            let result = recover_designer_state(&worker_context);
            sender.send(result).expect("send recovery result");
        });

        assert!(receiver.recv_timeout(Duration::from_millis(150)).is_err());
        drop(lock);
        receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("recovery unblocked")
            .expect("recovery succeeds");
        worker.join().expect("worker");
    }

    #[test]
    fn stale_generation_and_invalid_output_publish_nothing() {
        let dir = tempdir().expect("tempdir");
        let context = context(dir.path());
        seed_state(&context, VALID);
        let old_redb = fs::read(context.storage_path()).expect("old redb");
        let old_cdfi = fs::read(context.private_cdfi_path()).expect("old CDFI");
        let valid = dir.path().join("valid.xml");
        fs::write(&valid, VALID).expect("valid output");

        commit_designer_state(&context, &prepared(0), &valid).expect_err("stale generation");
        assert_eq!(fs::read(context.storage_path()).expect("redb"), old_redb);
        assert_eq!(
            fs::read(context.private_cdfi_path()).expect("CDFI"),
            old_cdfi
        );

        let invalid = dir.path().join("invalid.xml");
        fs::write(&invalid, "<broken>").expect("invalid output");
        commit_designer_state(&context, &prepared(1), &invalid).expect_err("invalid produced CDFI");
        assert_eq!(fs::read(context.storage_path()).expect("redb"), old_redb);
        assert_eq!(
            fs::read(context.private_cdfi_path()).expect("CDFI"),
            old_cdfi
        );
    }

    #[test]
    fn restart_recovery_rolls_back_incomplete_pair_publication() {
        let dir = tempdir().expect("tempdir");
        let context = context(dir.path());
        seed_state(&context, VALID);
        let old_redb = fs::read(context.storage_path()).expect("old redb");
        let old_cdfi = fs::read(context.private_cdfi_path()).expect("old CDFI");
        let transaction = context.transactions_dir().join("state-interrupted");
        fs::create_dir_all(&transaction).expect("transaction");
        fs::write(transaction.join(BACKUP_REDB), &old_redb).expect("redb backup");
        fs::write(transaction.join(BACKUP_CDFI), &old_cdfi).expect("CDFI backup");
        fs::write(transaction.join(STAGED_REDB), b"new partial redb").expect("staged redb");
        fs::write(transaction.join(STAGED_CDFI), b"new partial CDFI").expect("staged CDFI");
        fs::write(
            transaction.join(format!("{STAGED_REDB}.original.claimed")),
            &old_redb,
        )
        .expect("claimed old redb");
        fs::write(
            transaction.join(format!("{STAGED_CDFI}.original.claimed")),
            &old_cdfi,
        )
        .expect("claimed old CDFI");
        fs::remove_file(context.storage_path()).expect("remove old redb");
        fs::hard_link(transaction.join(STAGED_REDB), context.storage_path())
            .expect("publish partial redb");
        fs::remove_file(context.private_cdfi_path()).expect("remove old CDFI");
        fs::hard_link(transaction.join(STAGED_CDFI), context.private_cdfi_path())
            .expect("publish partial CDFI");
        fs::write(
            transaction.join(JOURNAL_FILE),
            serde_json::to_vec(&prepared_journal(&transaction, true, true)).expect("journal"),
        )
        .expect("journal file");

        recover_designer_state(&context).expect("recover");

        assert_eq!(fs::read(context.storage_path()).expect("redb"), old_redb);
        assert_eq!(
            fs::read(context.private_cdfi_path()).expect("CDFI"),
            old_cdfi
        );
        assert!(!transaction.exists());
    }

    #[cfg(unix)]
    #[test]
    fn recovery_preserves_journal_when_storage_lookup_hard_fails() {
        let dir = tempdir().expect("tempdir");
        let context = context(dir.path());
        let transaction = context.transactions_dir().join("state-hard-lookup");
        fs::create_dir_all(&transaction).expect("transaction");
        fs::write(transaction.join(STAGED_REDB), b"staged-redb").expect("staged redb");
        fs::write(transaction.join(STAGED_CDFI), b"staged-cdfi").expect("staged cdfi");
        fs::write(
            transaction.join(JOURNAL_FILE),
            serde_json::to_vec(&prepared_journal(&transaction, false, false)).expect("journal"),
        )
        .expect("journal file");
        std::os::unix::fs::symlink(context.storage_path(), context.storage_path())
            .expect("self symlink");

        let error = recover_designer_state(&context).expect_err("hard lookup must fail closed");

        assert!(matches!(
            error,
            RuntimeStateError::Storage(
                crate::change_detection::hash_storage::StorageError::Hard { .. }
            )
        ));
        assert!(transaction.exists());
        assert!(transaction.join(JOURNAL_FILE).exists());
        assert!(transaction.join(STAGED_REDB).exists());
        assert!(transaction.join(STAGED_CDFI).exists());
    }

    #[test]
    fn redb_backup_does_not_alias_the_claimed_live_database() {
        let dir = tempdir().expect("tempdir");
        let target = dir.path().join("hash-storage.redb");
        let backup = dir.path().join(BACKUP_REDB);
        let staged = dir.path().join(STAGED_REDB);
        HashStorage::create_replacement(target.clone(), &HashMap::new(), 1, 1)
            .expect("seed storage");
        fs::write(&staged, b"new staged redb").expect("staged redb");
        let original_bytes = fs::read(&target).expect("original bytes");

        prepare_redb_backup(
            &target,
            &backup,
            &staged,
            &ObservedStorageState::Initialized { generation: 1 },
        )
        .expect("prepare independent backup");
        let original_claim = transaction_original_claim_path(&staged).expect("claim path");
        fs::write(&original_claim, b"mutated claimed database").expect("mutate claim");

        assert_eq!(fs::read(backup).expect("backup bytes"), original_bytes);
    }

    #[test]
    fn restart_recovery_restores_concurrent_file_from_persisted_rollback_claim() {
        let dir = tempdir().expect("tempdir");
        let context = context(dir.path());
        seed_state(&context, VALID);
        let old_redb = fs::read(context.storage_path()).expect("old redb");
        let transaction = context.transactions_dir().join("state-interrupted-claim");
        fs::create_dir_all(&transaction).expect("transaction");
        fs::write(transaction.join(BACKUP_REDB), old_redb).expect("redb backup");
        fs::write(transaction.join(STAGED_REDB), b"our staged redb").expect("staged redb");
        fs::write(transaction.join(STAGED_CDFI), b"our staged CDFI").expect("staged CDFI");
        let concurrent = b"concurrent healthy B";
        fs::write(
            transaction.join(format!("{STAGED_REDB}.original.claimed")),
            b"old claimed A",
        )
        .expect("persisted original claim");
        fs::write(context.storage_path(), concurrent).expect("publish concurrent B");
        fs::write(
            transaction.join(JOURNAL_FILE),
            serde_json::to_vec(&prepared_journal(&transaction, true, false)).expect("journal"),
        )
        .expect("journal file");

        recover_designer_state(&context).expect("recover concurrent claim");

        assert_eq!(
            fs::read(context.storage_path()).expect("restored concurrent B"),
            concurrent
        );
    }

    #[test]
    fn restart_recovery_restores_foreign_file_claimed_before_validation() {
        let dir = tempdir().expect("tempdir");
        let context = context(dir.path());
        seed_state(&context, VALID);
        let old_redb = fs::read(context.storage_path()).expect("old redb");
        let old_cdfi = fs::read(context.private_cdfi_path()).expect("old CDFI");
        let transaction = context.transactions_dir().join("state-foreign-claim");
        fs::create_dir_all(&transaction).expect("transaction");
        fs::write(transaction.join(BACKUP_REDB), old_redb).expect("redb backup");
        fs::write(transaction.join(BACKUP_CDFI), old_cdfi).expect("CDFI backup");
        fs::write(transaction.join(STAGED_REDB), b"our staged redb").expect("staged redb");
        fs::write(transaction.join(STAGED_CDFI), b"our staged CDFI").expect("staged CDFI");
        let concurrent = b"concurrent CDFI B";
        fs::write(
            transaction.join(format!("{STAGED_CDFI}.original.claimed")),
            concurrent,
        )
        .expect("persisted foreign claim");
        fs::remove_file(context.private_cdfi_path()).expect("crash after CDFI claim");
        fs::write(
            transaction.join(JOURNAL_FILE),
            serde_json::to_vec(&prepared_journal(&transaction, true, true)).expect("journal"),
        )
        .expect("journal file");

        recover_designer_state(&context).expect("recover foreign claim");

        assert_eq!(
            fs::read(context.private_cdfi_path()).expect("restored foreign CDFI"),
            concurrent
        );
    }

    #[test]
    fn restart_recovery_preserves_same_bytes_foreign_original_claims() {
        let dir = tempdir().expect("tempdir");
        let context = context(dir.path());
        seed_state(&context, VALID);
        let old_redb = fs::read(context.storage_path()).expect("old redb");
        let old_cdfi = fs::read(context.private_cdfi_path()).expect("old CDFI");
        let transaction = context
            .transactions_dir()
            .join("state-same-bytes-originals");
        fs::create_dir_all(&transaction).expect("transaction");
        fs::write(transaction.join(BACKUP_REDB), &old_redb).expect("redb backup");
        fs::write(transaction.join(BACKUP_CDFI), &old_cdfi).expect("CDFI backup");
        fs::write(transaction.join(STAGED_REDB), b"new redb").expect("staged redb");
        fs::write(transaction.join(STAGED_CDFI), b"new CDFI").expect("staged CDFI");
        let redb_claim = transaction.join(format!("{STAGED_REDB}.original.claimed"));
        let cdfi_claim = transaction.join(format!("{STAGED_CDFI}.original.claimed"));
        fs::copy(transaction.join(BACKUP_REDB), &redb_claim)
            .expect("same-bytes foreign redb claim");
        fs::copy(transaction.join(BACKUP_CDFI), &cdfi_claim)
            .expect("same-bytes foreign CDFI claim");
        let redb_identity = file_fingerprint(&redb_claim)
            .expect("redb claim fingerprint")
            .identity;
        let cdfi_identity = file_fingerprint(&cdfi_claim)
            .expect("CDFI claim fingerprint")
            .identity;
        fs::remove_file(context.storage_path()).expect("crash after redb claim");
        fs::remove_file(context.private_cdfi_path()).expect("crash after CDFI claim");
        fs::write(
            transaction.join(JOURNAL_FILE),
            serde_json::to_vec(&prepared_journal(&transaction, true, true)).expect("journal"),
        )
        .expect("journal file");

        recover_designer_state(&context).expect("recover same-bytes original claims");

        assert_eq!(
            file_fingerprint(&context.storage_path())
                .expect("restored redb fingerprint")
                .identity,
            redb_identity
        );
        assert_eq!(
            file_fingerprint(&context.private_cdfi_path())
                .expect("restored CDFI fingerprint")
                .identity,
            cdfi_identity
        );
    }

    #[test]
    fn restart_recovery_preserves_same_bytes_foreign_redb_and_cdfi() {
        let dir = tempdir().expect("tempdir");
        let context = context(dir.path());
        seed_state(&context, VALID);
        let old_redb = fs::read(context.storage_path()).expect("old redb");
        let old_cdfi = fs::read(context.private_cdfi_path()).expect("old CDFI");
        let transaction = context.transactions_dir().join("state-same-bytes-foreign");
        fs::create_dir_all(&transaction).expect("transaction");
        fs::write(transaction.join(BACKUP_REDB), &old_redb).expect("redb backup");
        fs::write(transaction.join(BACKUP_CDFI), &old_cdfi).expect("CDFI backup");
        let new_redb = b"same bytes redb";
        let new_cdfi = b"same bytes CDFI";
        fs::write(transaction.join(STAGED_REDB), new_redb).expect("staged redb");
        fs::write(transaction.join(STAGED_CDFI), new_cdfi).expect("staged CDFI");
        fs::write(
            transaction.join(format!("{STAGED_REDB}.original.claimed")),
            &old_redb,
        )
        .expect("redb original claim");
        fs::write(
            transaction.join(format!("{STAGED_CDFI}.original.claimed")),
            &old_cdfi,
        )
        .expect("CDFI original claim");
        fs::remove_file(context.storage_path()).expect("remove old redb");
        fs::copy(transaction.join(STAGED_REDB), context.storage_path())
            .expect("independent same-bytes redb B");
        fs::remove_file(context.private_cdfi_path()).expect("remove old CDFI");
        fs::copy(transaction.join(STAGED_CDFI), context.private_cdfi_path())
            .expect("independent same-bytes CDFI B");
        fs::write(
            transaction.join(JOURNAL_FILE),
            serde_json::to_vec(&prepared_journal(&transaction, true, true)).expect("journal"),
        )
        .expect("journal file");

        recover_designer_state(&context).expect("recover same-bytes foreign files");

        assert_eq!(
            fs::read(context.storage_path()).expect("foreign redb"),
            new_redb
        );
        assert_eq!(
            fs::read(context.private_cdfi_path()).expect("foreign CDFI"),
            new_cdfi
        );
    }

    #[test]
    fn restart_recovery_rolls_back_staged_target_when_original_claim_persisted() {
        let dir = tempdir().expect("tempdir");
        let context = context(dir.path());
        seed_state(&context, VALID);
        let old_redb = fs::read(context.storage_path()).expect("old redb");
        let old_cdfi = fs::read(context.private_cdfi_path()).expect("old CDFI");
        let transaction = context.transactions_dir().join("state-after-redb-publish");
        fs::create_dir_all(&transaction).expect("transaction");
        fs::write(transaction.join(BACKUP_REDB), &old_redb).expect("redb backup");
        fs::write(transaction.join(BACKUP_CDFI), &old_cdfi).expect("CDFI backup");
        fs::write(transaction.join(STAGED_REDB), b"new staged redb").expect("staged redb");
        fs::write(transaction.join(STAGED_CDFI), b"new staged CDFI").expect("staged CDFI");
        fs::write(
            transaction.join(format!("{STAGED_REDB}.original.claimed")),
            &old_redb,
        )
        .expect("persisted original claim");
        fs::remove_file(context.storage_path()).expect("remove old redb");
        fs::hard_link(transaction.join(STAGED_REDB), context.storage_path())
            .expect("publish staged redb");
        fs::write(
            transaction.join(JOURNAL_FILE),
            serde_json::to_vec(&prepared_journal(&transaction, true, true)).expect("journal"),
        )
        .expect("journal file");

        recover_designer_state(&context).expect("recover after staged publication");

        assert_eq!(
            fs::read(context.storage_path()).expect("old redb"),
            old_redb
        );
        assert_eq!(
            fs::read_to_string(context.private_cdfi_path()).expect("old CDFI"),
            VALID
        );
    }
}
