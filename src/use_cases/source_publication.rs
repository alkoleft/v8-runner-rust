use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::change_detection::scanner::is_always_ignored_relative_path;
use crate::domain::runtime_state::DumpTransactionId;
use crate::use_cases::shadow_merge::{FileVersion, ManifestMergePlan, MergeAction, RawFileHash};

const JOURNAL_FILE: &str = "journal.json";

#[cfg(test)]
thread_local! {
    static BEFORE_FIRST_MUTATION: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static BEFORE_ACTION_INSTALL: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static AFTER_SAFE_PARENT_OPEN: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static FAIL_JOURNAL_STATUS: std::cell::Cell<Option<JournalStatus>> =
        const { std::cell::Cell::new(None) };
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct TargetIdentity(String);

impl TargetIdentity {
    pub(crate) fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ObservedStateGeneration {
    generation: u64,
    dump_transaction_id: Option<DumpTransactionId>,
}

impl ObservedStateGeneration {
    pub(crate) const fn new(value: u64) -> Self {
        Self {
            generation: value,
            dump_transaction_id: None,
        }
    }

    pub(crate) fn with_dump_transaction(
        generation: u64,
        dump_transaction_id: DumpTransactionId,
    ) -> Self {
        Self {
            generation,
            dump_transaction_id: Some(dump_transaction_id),
        }
    }
}

#[derive(Debug, Error)]
pub(crate) enum PublicationError {
    #[cfg(not(any(unix, windows)))]
    #[error("safe descriptor-relative source publication is unavailable on this platform")]
    UnsupportedSafeFilesystem,
    #[error("manifest merge contains a conflict")]
    Conflict,
    #[error("publication request is missing required field '{field}'")]
    MissingField { field: &'static str },
    #[error("invalid managed publication path '{path}'")]
    InvalidPath { path: String },
    #[error("managed snapshot changed at '{path}' before publication")]
    SnapshotMismatch { path: String },
    #[error("managed path '{path}' is not a regular file")]
    UnsafeFile { path: PathBuf },
    #[error("publication journal target does not match this source or target identity")]
    ForeignTarget,
    #[error("foreign content at '{path}' prevents safe publication recovery")]
    ForeignContent { path: PathBuf },
    #[error("invalid publication journal '{path}': {reason}")]
    InvalidJournal { path: PathBuf, reason: String },
    #[error("failed to {operation} '{path}': {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    #[error("source publication failed ({publication}); rollback also failed ({rollback})")]
    PublicationAndRollback {
        publication: Box<PublicationError>,
        rollback: Box<PublicationError>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum JournalStatus {
    Prepared,
    SourceApplied,
    StateVisible,
    Committed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ExpectedFile {
    sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalAction {
    path: String,
    before: ExpectedFile,
    after: ExpectedFile,
    backup: Option<String>,
    payload: Option<String>,
    claim: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicationJournal {
    generation: u64,
    target_identity: TargetIdentity,
    source_root: PathBuf,
    status: JournalStatus,
    actions: Vec<JournalAction>,
    dump_transaction_id: DumpTransactionId,
}

pub(crate) struct PublicationRequest<'a> {
    source_root: &'a Path,
    dump_root: &'a Path,
    plan: &'a ManifestMergePlan,
    transaction_root: &'a Path,
    generation: u64,
    target_identity: TargetIdentity,
    dump_transaction_id: DumpTransactionId,
}

impl<'a> PublicationRequest<'a> {
    pub(crate) fn builder(
        source_root: &'a Path,
        dump_root: &'a Path,
        plan: &'a ManifestMergePlan,
    ) -> PublicationRequestBuilder<'a> {
        PublicationRequestBuilder {
            source_root,
            dump_root,
            plan,
            transaction_root: None,
            generation: None,
            target_identity: None,
            dump_transaction_id: None,
        }
    }

    pub(crate) fn prepare(self) -> Result<PreparedPublication, PublicationError> {
        #[cfg(not(any(unix, windows)))]
        return Err(PublicationError::UnsupportedSafeFilesystem);
        #[cfg(any(unix, windows))]
        {
            if self.plan.has_conflicts() {
                return Err(PublicationError::Conflict);
            }
            let source_root = canonical_directory(self.source_root)?;
            let dump_root = canonical_directory(self.dump_root)?;
            let mut observed_source = BTreeMap::new();
            let mut observed_dump = BTreeMap::new();

            // This is deliberately one complete validation pass before transaction creation or
            // source mutation. Opened files are no-follow and their bytes provide the payloads.
            for entry in self.plan.entries() {
                validate_relative(entry.path())?;
                let source = read_expected(&source_root, entry.path(), entry.source())?;
                observed_source.insert(entry.path().to_owned(), source);
                if entry.action() == MergeAction::Apply {
                    let dump = read_expected(&dump_root, entry.path(), entry.dump())?;
                    observed_dump.insert(entry.path().to_owned(), dump);
                }
            }

            if self
                .transaction_root
                .try_exists()
                .map_err(|source| io_error("inspect transaction", self.transaction_root, source))?
            {
                return Err(PublicationError::InvalidJournal {
                    path: self.transaction_root.to_path_buf(),
                    reason: "transaction already exists and must be recovered first".to_owned(),
                });
            }
            fs::create_dir(self.transaction_root)
                .map_err(|source| io_error("create transaction", self.transaction_root, source))?;
            let payload_root = self.transaction_root.join("payloads");
            let backup_root = self.transaction_root.join("backups");
            fs::create_dir(&payload_root)
                .map_err(|source| io_error("create payload directory", &payload_root, source))?;
            fs::create_dir(&backup_root)
                .map_err(|source| io_error("create backup directory", &backup_root, source))?;

            let mut actions = Vec::new();
            for entry in self
                .plan
                .entries()
                .iter()
                .filter(|entry| entry.action() == MergeAction::Apply)
            {
                let index = actions.len();
                let before_bytes = observed_source.remove(entry.path()).flatten();
                let after_bytes = observed_dump.remove(entry.path()).flatten();
                let backup = write_blob(&backup_root, index, before_bytes.as_deref())?;
                let payload = write_blob(&payload_root, index, after_bytes.as_deref())?;
                actions.push(JournalAction {
                    path: entry.path().to_owned(),
                    before: expected(before_bytes.as_deref()),
                    after: expected(after_bytes.as_deref()),
                    backup,
                    payload,
                    claim: format!(".v8-runner-publication-claim-{}", uuid::Uuid::new_v4()),
                });
            }
            sync_directory(&payload_root)?;
            sync_directory(&backup_root)?;
            let journal = PublicationJournal {
                generation: self.generation,
                target_identity: self.target_identity,
                source_root,
                status: JournalStatus::Prepared,
                actions,
                dump_transaction_id: self.dump_transaction_id,
            };
            write_journal(self.transaction_root, &journal)?;
            sync_directory(self.transaction_root)?;
            if let Some(parent) = self.transaction_root.parent() {
                sync_directory(parent)?;
            }
            Ok(PreparedPublication {
                transaction_root: self.transaction_root.to_path_buf(),
                journal,
            })
        }
    }
}

pub(crate) struct PublicationRequestBuilder<'a> {
    source_root: &'a Path,
    dump_root: &'a Path,
    plan: &'a ManifestMergePlan,
    transaction_root: Option<&'a Path>,
    generation: Option<u64>,
    target_identity: Option<TargetIdentity>,
    dump_transaction_id: Option<DumpTransactionId>,
}

impl<'a> PublicationRequestBuilder<'a> {
    pub(crate) fn transaction_root(mut self, value: &'a Path) -> Self {
        self.transaction_root = Some(value);
        self
    }

    pub(crate) fn generation(mut self, value: u64) -> Self {
        self.generation = Some(value);
        self
    }

    pub(crate) fn target_identity(mut self, value: TargetIdentity) -> Self {
        self.target_identity = Some(value);
        self
    }

    pub(crate) fn dump_transaction_id(mut self, value: DumpTransactionId) -> Self {
        self.dump_transaction_id = Some(value);
        self
    }

    pub(crate) fn prepare(self) -> Result<PreparedPublication, PublicationError> {
        PublicationRequest {
            source_root: self.source_root,
            dump_root: self.dump_root,
            plan: self.plan,
            transaction_root: self
                .transaction_root
                .ok_or(PublicationError::MissingField {
                    field: "transaction_root",
                })?,
            generation: self.generation.ok_or(PublicationError::MissingField {
                field: "generation",
            })?,
            target_identity: self.target_identity.ok_or(PublicationError::MissingField {
                field: "target_identity",
            })?,
            dump_transaction_id: self.dump_transaction_id.ok_or(
                PublicationError::MissingField {
                    field: "dump_transaction_id",
                },
            )?,
        }
        .prepare()
    }
}

#[derive(Debug)]
pub(crate) struct PreparedPublication {
    transaction_root: PathBuf,
    journal: PublicationJournal,
}

impl PreparedPublication {
    pub(crate) fn apply(mut self) -> Result<SourceAppliedPublication, PublicationError> {
        if let Err(error) = apply_forward(&self.transaction_root, &self.journal) {
            return match rollback(&self.transaction_root, &self.journal) {
                Ok(()) => Err(error),
                Err(rollback) => Err(PublicationError::PublicationAndRollback {
                    publication: Box::new(error),
                    rollback: Box::new(rollback),
                }),
            };
        }
        self.journal.status = JournalStatus::SourceApplied;
        if let Err(publication) = write_journal(&self.transaction_root, &self.journal) {
            return match rollback(&self.transaction_root, &self.journal) {
                Ok(()) => Err(publication),
                Err(rollback) => Err(PublicationError::PublicationAndRollback {
                    publication: Box::new(publication),
                    rollback: Box::new(rollback),
                }),
            };
        }
        Ok(SourceAppliedPublication {
            transaction_root: self.transaction_root,
            journal: self.journal,
        })
    }

    #[cfg(test)]
    fn leave_for_recovery(self) {}
}

#[derive(Debug)]
pub(crate) struct SourceAppliedPublication {
    transaction_root: PathBuf,
    journal: PublicationJournal,
}

impl SourceAppliedPublication {
    /// Call only after the corresponding private state generation is durably visible.
    pub(crate) fn mark_state_visible(
        mut self,
        observed: ObservedStateGeneration,
    ) -> Result<StateVisiblePublication, PublicationError> {
        if observed.generation != self.journal.generation
            || observed.dump_transaction_id.as_ref() != Some(&self.journal.dump_transaction_id)
        {
            return Err(PublicationError::InvalidJournal {
                path: self.transaction_root.join(JOURNAL_FILE),
                reason: format!(
                    "state generation {} is visible, expected {}",
                    observed.generation, self.journal.generation
                ),
            });
        }
        self.journal.status = JournalStatus::StateVisible;
        write_journal(&self.transaction_root, &self.journal)?;
        Ok(StateVisiblePublication {
            transaction_root: self.transaction_root,
            journal: self.journal,
        })
    }

    #[cfg(test)]
    fn leave_for_recovery(self) {}
}

#[derive(Debug)]
pub(crate) struct StateVisiblePublication {
    transaction_root: PathBuf,
    journal: PublicationJournal,
}

impl StateVisiblePublication {
    pub(crate) fn commit(mut self) -> Result<(), PublicationError> {
        self.journal.status = JournalStatus::Committed;
        write_journal(&self.transaction_root, &self.journal)?;
        cleanup_transaction(&self.transaction_root)
    }

    #[cfg(test)]
    fn leave_for_recovery(self) {}

    #[cfg(test)]
    fn mark_committed_for_recovery(mut self) {
        self.journal.status = JournalStatus::Committed;
        write_journal(&self.transaction_root, &self.journal).unwrap();
    }
}

pub(crate) fn recover_publication(
    transaction_root: &Path,
    source_root: &Path,
    target_identity: &TargetIdentity,
    observed: ObservedStateGeneration,
) -> Result<(), PublicationError> {
    #[cfg(not(any(unix, windows)))]
    return Err(PublicationError::UnsupportedSafeFilesystem);
    #[cfg(any(unix, windows))]
    {
        if !transaction_root
            .try_exists()
            .map_err(|source| io_error("inspect transaction", transaction_root, source))?
        {
            return Ok(());
        }
        let journal = read_journal(transaction_root)?;
        let source_root = canonical_directory(source_root)?;
        if journal.source_root != source_root || &journal.target_identity != target_identity {
            return Err(PublicationError::ForeignTarget);
        }
        match journal.status {
            JournalStatus::Prepared | JournalStatus::SourceApplied => {
                if observed.generation == journal.generation
                    && observed.dump_transaction_id.as_ref() == Some(&journal.dump_transaction_id)
                {
                    let mut committed = journal;
                    committed.status = JournalStatus::Committed;
                    write_journal(transaction_root, &committed)?;
                } else {
                    rollback(transaction_root, &journal)?;
                }
            }
            JournalStatus::StateVisible => {
                if observed.generation != journal.generation
                    || observed.dump_transaction_id.as_ref() != Some(&journal.dump_transaction_id)
                {
                    return Err(PublicationError::ForeignTarget);
                }
                let mut committed = journal;
                committed.status = JournalStatus::Committed;
                write_journal(transaction_root, &committed)?;
            }
            JournalStatus::Committed => {}
        }
        cleanup_transaction(transaction_root)
    }
}

fn apply_forward(
    transaction_root: &Path,
    journal: &PublicationJournal,
) -> Result<(), PublicationError> {
    #[cfg(windows)]
    reconcile_windows_scratch(journal, Direction::Forward)?;
    for action in &journal.actions {
        if inspect_file(&journal.source_root, &action.path)? != action.before {
            return Err(PublicationError::ForeignContent {
                path: journal.source_root.join(&action.path),
            });
        }
    }
    #[cfg(test)]
    BEFORE_FIRST_MUTATION.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
    for action in &journal.actions {
        let current = inspect_file(&journal.source_root, &action.path)?;
        if current != action.before {
            return Err(PublicationError::ForeignContent {
                path: journal.source_root.join(&action.path),
            });
        }
        #[cfg(test)]
        BEFORE_ACTION_INSTALL.with(|hook| {
            if let Some(hook) = hook.borrow_mut().take() {
                hook();
            }
        });
        set_file(transaction_root, journal, action, Direction::Forward)?;
    }
    sync_directory(&journal.source_root)
}

fn rollback(transaction_root: &Path, journal: &PublicationJournal) -> Result<(), PublicationError> {
    #[cfg(windows)]
    reconcile_windows_scratch(journal, Direction::Backward)?;
    let mut foreign_path = None;
    for action in journal.actions.iter().rev() {
        let current = inspect_file(&journal.source_root, &action.path)?;
        if current == action.before {
            continue;
        }
        if current != action.after {
            foreign_path.get_or_insert_with(|| journal.source_root.join(&action.path));
            continue;
        }
        set_file(transaction_root, journal, action, Direction::Backward)?;
    }
    sync_directory(&journal.source_root)?;
    if let Some(path) = foreign_path {
        Err(PublicationError::ForeignContent { path })
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn reconcile_windows_scratch(
    journal: &PublicationJournal,
    direction: Direction,
) -> Result<(), PublicationError> {
    use std::ffi::OsStr;

    use crate::support::windows_fs;

    for action in &journal.actions {
        let Some(parent) = windows_fs::open_parent(&journal.source_root, &action.path, false)
            .map_err(|source| {
                io_error(
                    "open managed parent for recovery",
                    &journal.source_root.join(&action.path),
                    source,
                )
            })?
        else {
            continue;
        };
        let display = journal.source_root.join(&action.path);
        let claim_name = OsStr::new(&action.claim);
        let install_name = format!("{}.new", action.claim);
        let install_name = OsStr::new(&install_name);
        let target = windows_fs::read_optional(&parent)
            .map_err(|source| io_error("inspect managed target for recovery", &display, source))?;
        let claim = windows_fs::read_named_optional(&parent.directory, claim_name)
            .map_err(|source| io_error("inspect managed claim for recovery", &display, source))?;
        let install =
            windows_fs::read_named_optional(&parent.directory, install_name).map_err(|source| {
                io_error(
                    "inspect managed install file for recovery",
                    &display,
                    source,
                )
            })?;
        let known = |bytes: &[u8]| {
            let hash = Some(hash_hex(bytes));
            hash == action.before.sha256 || hash == action.after.sha256
        };
        let desired_hash = match direction {
            Direction::Forward => &action.after.sha256,
            Direction::Backward => &action.before.sha256,
        };
        if claim.as_deref().is_some_and(|bytes| !known(bytes)) {
            return Err(PublicationError::ForeignContent { path: display });
        }
        if install.as_deref().is_some_and(|bytes| !known(bytes)) {
            return Err(PublicationError::ForeignContent { path: display });
        }
        if let Some(target) = target {
            if !known(&target) {
                return Err(PublicationError::ForeignContent { path: display });
            }
            remove_windows_scratch(&parent.directory, claim_name, &display)?;
            remove_windows_scratch(&parent.directory, install_name, &display)?;
            continue;
        }
        match install {
            Some(bytes)
                if desired_hash
                    .as_ref()
                    .is_some_and(|hash| hash == &hash_hex(&bytes)) =>
            {
                let install =
                    windows_fs::open_named_existing(&parent.directory, install_name, true)
                        .map_err(|source| {
                            io_error("open managed install file for recovery", &display, source)
                        })?;
                windows_fs::rename_to(&install, &parent.directory, &parent.file_name).map_err(
                    |source| io_error("finish managed install recovery", &display, source),
                )?;
                remove_windows_scratch(&parent.directory, claim_name, &display)?;
            }
            Some(_) => {
                remove_windows_scratch(&parent.directory, install_name, &display)?;
                if claim.is_some() {
                    let claim =
                        windows_fs::open_named_existing(&parent.directory, claim_name, true)
                            .map_err(|source| {
                                io_error("open managed claim for recovery", &display, source)
                            })?;
                    windows_fs::rename_to(&claim, &parent.directory, &parent.file_name).map_err(
                        |source| {
                            io_error("restore managed claim during recovery", &display, source)
                        },
                    )?;
                }
            }
            None if claim.is_some() => {
                let claim = windows_fs::open_named_existing(&parent.directory, claim_name, true)
                    .map_err(|source| {
                        io_error("open managed claim for recovery", &display, source)
                    })?;
                windows_fs::rename_to(&claim, &parent.directory, &parent.file_name).map_err(
                    |source| io_error("restore managed claim during recovery", &display, source),
                )?;
            }
            None => {}
        }
        windows_fs::flush(&parent.directory)
            .map_err(|source| io_error("sync managed parent after recovery", &display, source))?;
    }
    Ok(())
}

#[cfg(windows)]
fn remove_windows_scratch(
    parent: &File,
    name: &std::ffi::OsStr,
    display: &Path,
) -> Result<(), PublicationError> {
    use crate::support::windows_fs;

    match windows_fs::open_named_existing(parent, name, true) {
        Ok(file) => windows_fs::delete_on_close(&file)
            .map_err(|source| io_error("remove managed recovery scratch", display, source)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(io_error("open managed recovery scratch", display, source)),
    }
}

#[derive(Clone, Copy)]
enum Direction {
    Forward,
    Backward,
}

fn set_file(
    transaction_root: &Path,
    journal: &PublicationJournal,
    action: &JournalAction,
    direction: Direction,
) -> Result<(), PublicationError> {
    #[cfg(unix)]
    return set_file_unix(transaction_root, journal, action, direction);
    #[cfg(windows)]
    return set_file_windows(transaction_root, journal, action, direction);
    #[cfg(not(any(unix, windows)))]
    Err(PublicationError::UnsupportedSafeFilesystem)
}

#[cfg(windows)]
fn set_file_windows(
    transaction_root: &Path,
    journal: &PublicationJournal,
    action: &JournalAction,
    direction: Direction,
) -> Result<(), PublicationError> {
    use std::ffi::OsStr;

    use crate::support::windows_fs;

    let parent = windows_fs::open_parent(&journal.source_root, &action.path, true)
        .map_err(|source| {
            io_error(
                "open managed parent by handle",
                &journal.source_root.join(&action.path),
                source,
            )
        })?
        .ok_or_else(|| PublicationError::InvalidPath {
            path: action.path.clone(),
        })?;
    #[cfg(test)]
    AFTER_SAFE_PARENT_OPEN.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
    let rebound =
        windows_fs::open_parent(&journal.source_root, &action.path, false).map_err(|source| {
            io_error(
                "reopen managed parent by handle",
                &journal.source_root.join(&action.path),
                source,
            )
        })?;
    if !rebound.as_ref().is_some_and(|rebound| {
        windows_fs::same_directory(&parent.directory, &rebound.directory).unwrap_or(false)
    }) {
        return Err(PublicationError::ForeignContent {
            path: journal.source_root.join(&action.path),
        });
    }

    let (expected, current, blob) = match direction {
        Direction::Forward => (
            &action.after,
            &action.before,
            action
                .payload
                .as_deref()
                .map(|name| transaction_root.join("payloads").join(name)),
        ),
        Direction::Backward => (
            &action.before,
            &action.after,
            action
                .backup
                .as_deref()
                .map(|name| transaction_root.join("backups").join(name)),
        ),
    };
    let display = journal.source_root.join(&action.path);
    let claim_name = OsStr::new(&action.claim);
    let install_name = format!("{}.new", action.claim);
    let install_name = OsStr::new(&install_name);
    let expected_bytes = match (&expected.sha256, blob) {
        (None, None) => None,
        (Some(expected_hash), Some(blob)) => {
            let bytes = read_regular_no_follow(&blob)?;
            if hash_hex(&bytes) != *expected_hash {
                return Err(PublicationError::InvalidJournal {
                    path: blob,
                    reason: "payload hash mismatch".to_owned(),
                });
            }
            Some(bytes)
        }
        _ => {
            return Err(PublicationError::InvalidJournal {
                path: transaction_root.join(JOURNAL_FILE),
                reason: "file state and payload disagree".to_owned(),
            })
        }
    };

    // Deterministic transaction-owned scratch names may survive a process crash. A claim must
    // always contain one of the journaled versions; an install file is safe to discard because it
    // is never considered published until its handle-relative rename succeeds.
    if let Some(bytes) = windows_fs::read_named_optional(&parent.directory, claim_name)
        .map_err(|source| io_error("inspect managed publication claim", &display, source))?
    {
        let claim_hash = Some(hash_hex(&bytes));
        if claim_hash != action.before.sha256 && claim_hash != action.after.sha256 {
            return Err(PublicationError::ForeignContent { path: display });
        }
        let claim = windows_fs::open_named_existing(&parent.directory, claim_name, true)
            .map_err(|source| io_error("open recoverable publication claim", &display, source))?;
        windows_fs::delete_on_close(&claim)
            .map_err(|source| io_error("remove recoverable publication claim", &display, source))?;
    }
    if let Some(bytes) = windows_fs::read_named_optional(&parent.directory, install_name)
        .map_err(|source| io_error("inspect managed publication install file", &display, source))?
    {
        let install_hash = Some(hash_hex(&bytes));
        if install_hash != action.before.sha256 && install_hash != action.after.sha256 {
            return Err(PublicationError::ForeignContent { path: display });
        }
        let install = windows_fs::open_named_existing(&parent.directory, install_name, true)
            .map_err(|source| {
                io_error(
                    "open recoverable publication install file",
                    &display,
                    source,
                )
            })?;
        windows_fs::delete_on_close(&install).map_err(|source| {
            io_error(
                "remove recoverable publication install file",
                &display,
                source,
            )
        })?;
    }

    let target = match &current.sha256 {
        Some(current_hash) => {
            let mut target = windows_fs::open_regular_existing(&parent, true)
                .map_err(|source| io_error("open managed file by handle", &display, source))?;
            let actual = windows_fs::read_all(&mut target)
                .map_err(|source| io_error("verify managed file by handle", &display, source))?;
            if hash_hex(&actual) != *current_hash {
                return Err(PublicationError::ForeignContent { path: display });
            }
            windows_fs::rename_to(&target, &parent.directory, claim_name)
                .map_err(|source| io_error("claim managed file by handle", &display, source))?;
            Some(target)
        }
        None => None,
    };

    let result = (|| -> Result<(), PublicationError> {
        match expected_bytes {
            None => Ok(()),
            Some(bytes) => {
                let install = windows_fs::create_named(&parent.directory, install_name).map_err(
                    |source| io_error("create managed publication install file", &display, source),
                )?;
                let install = windows_fs::write_synced(install, &bytes).map_err(|source| {
                    io_error("write managed publication install file", &display, source)
                })?;
                windows_fs::rename_to(&install, &parent.directory, &parent.file_name)
                    .map_err(|source| io_error("publish managed file by handle", &display, source))
            }
        }
    })();

    if let Err(error) = result {
        if let Some(target) = target.as_ref() {
            let _ = windows_fs::rename_to(target, &parent.directory, &parent.file_name);
        }
        return Err(error);
    }
    if let Some(target) = target.as_ref() {
        windows_fs::delete_on_close(target)
            .map_err(|source| io_error("remove replaced managed file", &display, source))?;
    }
    windows_fs::flush(&parent.directory)
        .map_err(|source| io_error("sync managed parent directory", &display, source))
}

#[cfg(unix)]
struct ManagedParent {
    directory: File,
    file_name: std::ffi::CString,
}

#[cfg(unix)]
fn set_file_unix(
    transaction_root: &Path,
    journal: &PublicationJournal,
    action: &JournalAction,
    direction: Direction,
) -> Result<(), PublicationError> {
    use std::os::fd::AsRawFd;

    let parent =
        open_managed_parent_unix(&journal.source_root, &action.path, true)?.ok_or_else(|| {
            PublicationError::InvalidPath {
                path: action.path.clone(),
            }
        })?;
    #[cfg(test)]
    AFTER_SAFE_PARENT_OPEN.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
    let rebound = open_managed_parent_unix(&journal.source_root, &action.path, false)?;
    if !rebound
        .as_ref()
        .is_some_and(|rebound| same_directory(&parent.directory, &rebound.directory))
    {
        return Err(PublicationError::ForeignContent {
            path: journal.source_root.join(&action.path),
        });
    }

    let (expected, current, blob) = match direction {
        Direction::Forward => (
            &action.after,
            &action.before,
            action
                .payload
                .as_deref()
                .map(|name| transaction_root.join("payloads").join(name)),
        ),
        Direction::Backward => (
            &action.before,
            &action.after,
            action
                .backup
                .as_deref()
                .map(|name| transaction_root.join("backups").join(name)),
        ),
    };
    let claim = c_string_component(&action.claim, &action.path)?;
    clean_known_claim_unix(&parent.directory, &claim, action)?;
    match (&expected.sha256, blob) {
        (None, None) => remove_regular_claimed_unix(
            &parent.directory,
            &parent.file_name,
            current,
            &claim,
            &journal.source_root.join(&action.path),
        )?,
        (Some(hash), Some(blob)) => replace_from_blob_unix(
            &parent.directory,
            &parent.file_name,
            &blob,
            hash,
            current,
            &claim,
            &journal.source_root.join(&action.path),
        )?,
        _ => {
            return Err(PublicationError::InvalidJournal {
                path: transaction_root.join(JOURNAL_FILE),
                reason: "file state and payload disagree".to_owned(),
            })
        }
    }
    parent.directory.sync_all().map_err(|source| {
        io_error(
            "sync managed parent directory",
            &journal.source_root,
            source,
        )
    })?;
    let _ = parent.directory.as_raw_fd();
    Ok(())
}

#[cfg(unix)]
fn open_managed_parent_unix(
    root: &Path,
    relative: &str,
    create: bool,
) -> Result<Option<ManagedParent>, PublicationError> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::OpenOptionsExt;

    validate_relative(relative)?;
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let mut directory = options
        .open(root)
        .map_err(|source| io_error("open managed root without following links", root, source))?;
    let components = Path::new(relative).components().collect::<Vec<_>>();
    let Some((file_component, parents)) = components.split_last() else {
        return Err(PublicationError::InvalidPath {
            path: relative.to_owned(),
        });
    };
    for component in parents {
        let Component::Normal(name) = component else {
            return Err(PublicationError::InvalidPath {
                path: relative.to_owned(),
            });
        };
        let name = c_string_os_component(name, relative)?;
        let mut descriptor = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if descriptor < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ELOOP)
                || error.kind() == io::ErrorKind::NotADirectory
            {
                return Err(PublicationError::UnsafeFile {
                    path: root.join(relative),
                });
            }
            if error.kind() == io::ErrorKind::NotFound && !create {
                return Ok(None);
            }
            if error.kind() != io::ErrorKind::NotFound || !create {
                return Err(io_error(
                    "open managed parent without following links",
                    root,
                    error,
                ));
            }
            let created = unsafe { libc::mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o755) };
            if created != 0 {
                let source = io::Error::last_os_error();
                if source.kind() != io::ErrorKind::AlreadyExists {
                    return Err(io_error("create managed parent directory", root, source));
                }
            }
            directory
                .sync_all()
                .map_err(|source| io_error("sync managed parent directory", root, source))?;
            descriptor = unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if descriptor < 0 {
                return Err(io_error(
                    "open created managed parent directory",
                    root,
                    io::Error::last_os_error(),
                ));
            }
        }
        directory = unsafe { File::from_raw_fd(descriptor) };
    }
    let Component::Normal(file_name) = file_component else {
        return Err(PublicationError::InvalidPath {
            path: relative.to_owned(),
        });
    };
    Ok(Some(ManagedParent {
        directory,
        file_name: c_string_os_component(file_name, relative)?,
    }))
}

#[cfg(unix)]
fn same_directory(left: &File, right: &File) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (left.metadata(), right.metadata()) {
        (Ok(left), Ok(right)) => left.dev() == right.dev() && left.ino() == right.ino(),
        (Err(_), _) | (_, Err(_)) => false,
    }
}

#[cfg(unix)]
fn c_string_os_component(
    value: &std::ffi::OsStr,
    relative: &str,
) -> Result<std::ffi::CString, PublicationError> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(value.as_bytes()).map_err(|_| PublicationError::InvalidPath {
        path: relative.to_owned(),
    })
}

#[cfg(unix)]
fn c_string_component(value: &str, relative: &str) -> Result<std::ffi::CString, PublicationError> {
    std::ffi::CString::new(value).map_err(|_| PublicationError::InvalidPath {
        path: relative.to_owned(),
    })
}

#[cfg(unix)]
fn read_at_optional(
    directory: &File,
    name: &std::ffi::CStr,
    display: &Path,
) -> Result<Option<Vec<u8>>, PublicationError> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        let source = io::Error::last_os_error();
        return if source.kind() == io::ErrorKind::NotFound {
            Ok(None)
        } else {
            Err(io_error(
                "open managed file without following links",
                display,
                source,
            ))
        };
    }
    let mut file = unsafe { File::from_raw_fd(descriptor) };
    if !file
        .metadata()
        .map_err(|source| io_error("inspect managed file", display, source))?
        .file_type()
        .is_file()
    {
        return Err(PublicationError::UnsafeFile {
            path: display.to_path_buf(),
        });
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|source| io_error("read managed file", display, source))?;
    Ok(Some(bytes))
}

#[cfg(unix)]
fn unlink_at(
    directory: &File,
    name: &std::ffi::CStr,
    flags: libc::c_int,
    display: &Path,
) -> Result<(), PublicationError> {
    use std::os::fd::AsRawFd;
    if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), flags) } == 0 {
        Ok(())
    } else {
        Err(io_error(
            "remove managed entry",
            display,
            io::Error::last_os_error(),
        ))
    }
}

#[cfg(unix)]
fn link_at(
    directory: &File,
    source: &std::ffi::CStr,
    target: &std::ffi::CStr,
    display: &Path,
) -> Result<(), PublicationError> {
    use std::os::fd::AsRawFd;
    let result = unsafe {
        libc::linkat(
            directory.as_raw_fd(),
            source.as_ptr(),
            directory.as_raw_fd(),
            target.as_ptr(),
            0,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        let source = io::Error::last_os_error();
        if source.kind() == io::ErrorKind::AlreadyExists {
            Err(PublicationError::ForeignContent {
                path: display.to_path_buf(),
            })
        } else {
            Err(io_error("link managed entry", display, source))
        }
    }
}

#[cfg(unix)]
fn clean_known_claim_unix(
    directory: &File,
    claim: &std::ffi::CStr,
    action: &JournalAction,
) -> Result<(), PublicationError> {
    let display = Path::new(&action.claim);
    if let Some(bytes) = read_at_optional(directory, claim, display)? {
        let hash = Some(hash_hex(&bytes));
        if hash != action.before.sha256 && hash != action.after.sha256 {
            return Err(PublicationError::ForeignContent {
                path: display.to_path_buf(),
            });
        }
        unlink_at(directory, claim, 0, display)?;
        directory
            .sync_all()
            .map_err(|source| io_error("sync managed parent directory", display, source))?;
    }
    Ok(())
}

#[cfg(unix)]
fn remove_regular_claimed_unix(
    directory: &File,
    target: &std::ffi::CStr,
    current: &ExpectedFile,
    claim: &std::ffi::CStr,
    display: &Path,
) -> Result<(), PublicationError> {
    let Some(current_hash) = &current.sha256 else {
        return Ok(());
    };
    rename_at_no_replace(directory, target, claim, display)?;
    let claimed = read_at_optional(directory, claim, display)?.ok_or_else(|| {
        PublicationError::ForeignContent {
            path: display.to_path_buf(),
        }
    })?;
    if hash_hex(&claimed) != *current_hash {
        rename_at_no_replace(directory, claim, target, display)?;
        return Err(PublicationError::ForeignContent {
            path: display.to_path_buf(),
        });
    }
    unlink_at(directory, claim, 0, display)
}

#[cfg(target_os = "macos")]
fn rename_at_no_replace(
    directory: &File,
    left: &std::ffi::CStr,
    right: &std::ffi::CStr,
    display: &Path,
) -> Result<(), PublicationError> {
    rename_at_with_flags(directory, left, right, libc::RENAME_EXCL, display)
}

#[cfg(target_os = "linux")]
fn rename_at_no_replace(
    directory: &File,
    left: &std::ffi::CStr,
    right: &std::ffi::CStr,
    display: &Path,
) -> Result<(), PublicationError> {
    rename_at_with_flags(directory, left, right, libc::RENAME_NOREPLACE, display)
}

#[cfg(target_os = "macos")]
fn exchange_at(
    directory: &File,
    left: &std::ffi::CStr,
    right: &std::ffi::CStr,
    display: &Path,
) -> Result<(), PublicationError> {
    rename_at_with_flags(directory, left, right, libc::RENAME_SWAP, display)
}

#[cfg(target_os = "linux")]
fn exchange_at(
    directory: &File,
    left: &std::ffi::CStr,
    right: &std::ffi::CStr,
    display: &Path,
) -> Result<(), PublicationError> {
    rename_at_with_flags(directory, left, right, libc::RENAME_EXCHANGE, display)
}

#[cfg(target_os = "macos")]
fn rename_at_with_flags(
    directory: &File,
    left: &std::ffi::CStr,
    right: &std::ffi::CStr,
    flags: libc::c_uint,
    display: &Path,
) -> Result<(), PublicationError> {
    use std::os::fd::AsRawFd;
    let result = unsafe {
        libc::renameatx_np(
            directory.as_raw_fd(),
            left.as_ptr(),
            directory.as_raw_fd(),
            right.as_ptr(),
            flags,
        )
    };
    classify_rename_at(result, display)
}

#[cfg(target_os = "linux")]
fn rename_at_with_flags(
    directory: &File,
    left: &std::ffi::CStr,
    right: &std::ffi::CStr,
    flags: libc::c_uint,
    display: &Path,
) -> Result<(), PublicationError> {
    use std::os::fd::AsRawFd;
    let result = unsafe {
        libc::renameat2(
            directory.as_raw_fd(),
            left.as_ptr(),
            directory.as_raw_fd(),
            right.as_ptr(),
            flags,
        )
    };
    classify_rename_at(result, display)
}

#[cfg(unix)]
fn classify_rename_at(result: libc::c_int, display: &Path) -> Result<(), PublicationError> {
    if result == 0 {
        Ok(())
    } else {
        let source = io::Error::last_os_error();
        if matches!(
            source.kind(),
            io::ErrorKind::AlreadyExists | io::ErrorKind::NotFound
        ) {
            Err(PublicationError::ForeignContent {
                path: display.to_path_buf(),
            })
        } else {
            Err(io_error("atomically claim managed file", display, source))
        }
    }
}

#[cfg(unix)]
fn replace_from_blob_unix(
    directory: &File,
    target: &std::ffi::CStr,
    blob: &Path,
    expected_hash: &str,
    current: &ExpectedFile,
    claim: &std::ffi::CStr,
    display: &Path,
) -> Result<(), PublicationError> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let bytes = read_regular_no_follow(blob)?;
    if hash_hex(&bytes) != expected_hash {
        return Err(PublicationError::InvalidJournal {
            path: blob.to_path_buf(),
            reason: "payload hash mismatch".to_owned(),
        });
    }
    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            claim.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if descriptor < 0 {
        return Err(io_error(
            "create managed publication claim",
            display,
            io::Error::last_os_error(),
        ));
    }
    let mut output = unsafe { File::from_raw_fd(descriptor) };
    output
        .write_all(&bytes)
        .map_err(|source| io_error("write managed publication claim", display, source))?;
    output
        .sync_all()
        .map_err(|source| io_error("sync managed publication claim", display, source))?;
    drop(output);
    match &current.sha256 {
        None => {
            link_at(directory, claim, target, display)?;
            unlink_at(directory, claim, 0, display)?;
        }
        Some(current_hash) => {
            exchange_at(directory, claim, target, display)?;
            let swapped = read_at_optional(directory, claim, display)?.ok_or_else(|| {
                PublicationError::ForeignContent {
                    path: display.to_path_buf(),
                }
            })?;
            if hash_hex(&swapped) != *current_hash {
                exchange_at(directory, claim, target, display)?;
                let _ = unlink_at(directory, claim, 0, display);
                return Err(PublicationError::ForeignContent {
                    path: display.to_path_buf(),
                });
            }
            unlink_at(directory, claim, 0, display)?;
        }
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn replace_from_blob(
    target: &Path,
    blob: &Path,
    expected_hash: &str,
    current: &ExpectedFile,
    claim_name: &str,
) -> Result<(), PublicationError> {
    let bytes = read_regular_no_follow(blob)?;
    if hash_hex(&bytes) != expected_hash {
        return Err(PublicationError::InvalidJournal {
            path: blob.to_path_buf(),
            reason: "payload hash mismatch".to_owned(),
        });
    }
    let parent = target
        .parent()
        .ok_or_else(|| PublicationError::InvalidPath {
            path: target.display().to_string(),
        })?;
    let temp = parent.join(claim_name);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options
        .open(&temp)
        .map_err(|source| io_error("create publication file", &temp, source))?;
    file.write_all(&bytes)
        .map_err(|source| io_error("write publication file", &temp, source))?;
    file.sync_all()
        .map_err(|source| io_error("sync publication file", &temp, source))?;
    drop(file);
    match &current.sha256 {
        None => {
            fs::hard_link(&temp, target).map_err(|source| {
                if source.kind() == io::ErrorKind::AlreadyExists {
                    PublicationError::ForeignContent {
                        path: target.to_path_buf(),
                    }
                } else {
                    io_error("claim absent managed path", target, source)
                }
            })?;
            fs::remove_file(&temp)
                .map_err(|source| io_error("remove publication temporary file", &temp, source))?;
        }
        Some(current_hash) => {
            exchange_paths(&temp, target)?;
            let swapped = read_regular_no_follow(&temp)?;
            if hash_hex(&swapped) != *current_hash {
                exchange_paths(&temp, target)?;
                let _ = fs::remove_file(&temp);
                return Err(PublicationError::ForeignContent {
                    path: target.to_path_buf(),
                });
            }
            fs::remove_file(&temp)
                .map_err(|source| io_error("remove replaced managed file", &temp, source))?;
        }
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn clean_known_claim(target: &Path, action: &JournalAction) -> Result<(), PublicationError> {
    let parent = target
        .parent()
        .ok_or_else(|| PublicationError::InvalidPath {
            path: target.display().to_string(),
        })?;
    let claim = parent.join(&action.claim);
    match read_regular_no_follow(&claim) {
        Ok(bytes) => {
            let hash = Some(hash_hex(&bytes));
            if hash != action.before.sha256 && hash != action.after.sha256 {
                return Err(PublicationError::ForeignContent { path: claim });
            }
            fs::remove_file(&claim).map_err(|source| {
                io_error("remove recoverable publication claim", &claim, source)
            })?;
            sync_directory(parent)
        }
        Err(PublicationError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

#[cfg(not(any(unix, windows)))]
fn remove_regular_claimed(
    path: &Path,
    current: &ExpectedFile,
    claim_name: &str,
) -> Result<(), PublicationError> {
    let Some(current_hash) = &current.sha256 else {
        return Ok(());
    };
    let parent = path.parent().ok_or_else(|| PublicationError::InvalidPath {
        path: path.display().to_string(),
    })?;
    let claim = parent.join(claim_name);
    rename_no_replace(path, &claim)?;
    let claimed = read_regular_no_follow(&claim)?;
    if hash_hex(&claimed) != *current_hash {
        rename_no_replace(&claim, path)?;
        return Err(PublicationError::ForeignContent {
            path: path.to_path_buf(),
        });
    }
    fs::remove_file(&claim)
        .map_err(|source| io_error("remove claimed managed file", &claim, source))
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn exchange_paths(left: &Path, right: &Path) -> Result<(), PublicationError> {
    let claim = left.with_extension("exchange");
    fs::rename(right, &claim).map_err(|source| io_error("claim managed file", right, source))?;
    if let Err(source) = fs::rename(left, right) {
        let _ = fs::rename(&claim, right);
        return Err(io_error("publish managed file", right, source));
    }
    fs::rename(&claim, left).map_err(|source| io_error("finish managed exchange", left, source))
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn rename_no_replace(left: &Path, right: &Path) -> Result<(), PublicationError> {
    if right
        .try_exists()
        .map_err(|source| io_error("inspect claim path", right, source))?
    {
        return Err(PublicationError::ForeignContent {
            path: right.to_path_buf(),
        });
    }
    fs::rename(left, right).map_err(|source| io_error("claim managed file", left, source))
}

fn read_expected(
    root: &Path,
    relative: &str,
    expected: FileVersion,
) -> Result<Option<Vec<u8>>, PublicationError> {
    let actual = read_optional(root, relative)?;
    let actual_version = actual.as_deref().map_or(FileVersion::Absent, |bytes| {
        FileVersion::Present(hash_raw(bytes))
    });
    if actual_version != expected {
        return Err(PublicationError::SnapshotMismatch {
            path: relative.to_owned(),
        });
    }
    Ok(actual)
}

fn read_optional(root: &Path, relative: &str) -> Result<Option<Vec<u8>>, PublicationError> {
    #[cfg(unix)]
    {
        let Some(parent) = open_managed_parent_unix(root, relative, false)? else {
            return Ok(None);
        };
        return read_at_optional(&parent.directory, &parent.file_name, &root.join(relative));
    }
    #[cfg(windows)]
    {
        validate_relative(relative)?;
        let Some(parent) =
            crate::support::windows_fs::open_parent(root, relative, false).map_err(|source| {
                io_error(
                    "open managed parent by handle",
                    &root.join(relative),
                    source,
                )
            })?
        else {
            return Ok(None);
        };
        return crate::support::windows_fs::read_optional(&parent).map_err(|source| {
            io_error("read managed file by handle", &root.join(relative), source)
        });
    }
    #[cfg(not(any(unix, windows)))]
    Err(PublicationError::UnsupportedSafeFilesystem)
}

fn read_regular_no_follow(path: &Path) -> Result<Vec<u8>, PublicationError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options
        .open(path)
        .map_err(|source| io_error("open regular file without following links", path, source))?;
    let metadata = file
        .metadata()
        .map_err(|source| io_error("inspect opened file", path, source))?;
    if !metadata.file_type().is_file() {
        return Err(PublicationError::UnsafeFile {
            path: path.to_path_buf(),
        });
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|source| io_error("read regular file", path, source))?;
    Ok(bytes)
}

fn inspect_file(root: &Path, relative: &str) -> Result<ExpectedFile, PublicationError> {
    Ok(expected(read_optional(root, relative)?.as_deref()))
}

fn expected(bytes: Option<&[u8]>) -> ExpectedFile {
    ExpectedFile {
        sha256: bytes.map(hash_hex),
    }
}
fn hash_raw(bytes: &[u8]) -> RawFileHash {
    Sha256::digest(bytes).into()
}
fn hash_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn write_blob(
    root: &Path,
    index: usize,
    bytes: Option<&[u8]>,
) -> Result<Option<String>, PublicationError> {
    let Some(bytes) = bytes else {
        return Ok(None);
    };
    let name = format!("{index:08}.bin");
    let path = root.join(&name);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|source| io_error("create transaction blob", &path, source))?;
    file.write_all(bytes)
        .map_err(|source| io_error("write transaction blob", &path, source))?;
    file.sync_all()
        .map_err(|source| io_error("sync transaction blob", &path, source))?;
    Ok(Some(name))
}

fn write_journal(root: &Path, journal: &PublicationJournal) -> Result<(), PublicationError> {
    #[cfg(test)]
    if FAIL_JOURNAL_STATUS.with(|slot| {
        let should_fail = slot.get() == Some(journal.status);
        if should_fail {
            slot.set(None);
        }
        should_fail
    }) {
        return Err(io_error(
            "write injected publication journal",
            &root.join(JOURNAL_FILE),
            io::Error::other("injected journal failure"),
        ));
    }
    let path = root.join(JOURNAL_FILE);
    let temp = root.join("journal.next");
    let bytes =
        serde_json::to_vec_pretty(journal).map_err(|error| PublicationError::InvalidJournal {
            path: path.clone(),
            reason: error.to_string(),
        })?;
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temp)
        .map_err(|source| io_error("create publication journal", &temp, source))?;
    file.write_all(&bytes)
        .map_err(|source| io_error("write publication journal", &temp, source))?;
    file.sync_all()
        .map_err(|source| io_error("sync publication journal", &temp, source))?;
    drop(file);
    fs::rename(&temp, &path)
        .map_err(|source| io_error("publish publication journal", &path, source))?;
    sync_directory(root)
}

fn read_journal(root: &Path) -> Result<PublicationJournal, PublicationError> {
    let path = root.join(JOURNAL_FILE);
    let bytes = read_regular_no_follow(&path)?;
    let journal =
        serde_json::from_slice(&bytes).map_err(|error| PublicationError::InvalidJournal {
            path,
            reason: error.to_string(),
        })?;
    validate_journal(root, &journal)?;
    Ok(journal)
}

fn validate_journal(root: &Path, journal: &PublicationJournal) -> Result<(), PublicationError> {
    let invalid = |reason: &str| PublicationError::InvalidJournal {
        path: root.join(JOURNAL_FILE),
        reason: reason.to_owned(),
    };
    if journal.generation == 0 {
        return Err(invalid("generation must be positive"));
    }
    if journal.target_identity.0.is_empty() || !journal.source_root.is_absolute() {
        return Err(invalid(
            "target identity and absolute source root are required",
        ));
    }
    let mut previous_path: Option<&str> = None;
    let private_root_component = root
        .strip_prefix(&journal.source_root)
        .ok()
        .and_then(|relative| relative.components().next())
        .and_then(|component| match component {
            Component::Normal(name) => Some(name),
            Component::Prefix(_)
            | Component::RootDir
            | Component::CurDir
            | Component::ParentDir => None,
        });
    for action in &journal.actions {
        validate_relative(&action.path)?;
        if is_ignored_journal_path(&action.path, private_root_component) {
            return Err(invalid(
                "action path belongs to ignored or private inventory",
            ));
        }
        if previous_path.is_some_and(|previous| previous >= action.path.as_str()) {
            return Err(invalid("action paths must be strictly sorted and unique"));
        }
        previous_path = Some(&action.path);
        validate_expected_hash(&action.before, "before", &invalid)?;
        validate_expected_hash(&action.after, "after", &invalid)?;
        validate_blob_name(
            action.backup.as_deref(),
            action.before.sha256.is_some(),
            &invalid,
        )?;
        validate_blob_name(
            action.payload.as_deref(),
            action.after.sha256.is_some(),
            &invalid,
        )?;
        if !is_single_normal_component(&action.claim)
            || !action.claim.starts_with(".v8-runner-publication-claim-")
        {
            return Err(invalid("claim name must be one owned normal component"));
        }
    }
    Ok(())
}

fn is_ignored_journal_path(path: &str, private_root: Option<&std::ffi::OsStr>) -> bool {
    is_always_ignored_relative_path(Path::new(path))
        || Path::new(path)
            .components()
            .any(|component| match component {
                Component::Normal(name) => {
                    let text = name.to_string_lossy();
                    text.starts_with(".v8-runner-publication-claim-")
                        || private_root.is_some_and(|private| name == private)
                }
                Component::Prefix(_)
                | Component::RootDir
                | Component::CurDir
                | Component::ParentDir => true,
            })
}

fn validate_expected_hash(
    value: &ExpectedFile,
    label: &str,
    invalid: &impl Fn(&str) -> PublicationError,
) -> Result<(), PublicationError> {
    if value
        .sha256
        .as_ref()
        .is_some_and(|hash| hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err(invalid(&format!("{label} hash must be a raw SHA-256")));
    }
    Ok(())
}

fn validate_blob_name(
    name: Option<&str>,
    required: bool,
    invalid: &impl Fn(&str) -> PublicationError,
) -> Result<(), PublicationError> {
    if name.is_some() != required {
        return Err(invalid(
            "blob presence must match the corresponding file state",
        ));
    }
    if name.is_some_and(|name| !is_single_normal_component(name)) {
        return Err(invalid("blob name must be one normal path component"));
    }
    Ok(())
}

fn is_single_normal_component(value: &str) -> bool {
    let mut components = Path::new(value).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

fn cleanup_transaction(root: &Path) -> Result<(), PublicationError> {
    let parent = root.parent().map(Path::to_path_buf);
    fs::remove_dir_all(root)
        .map_err(|source| io_error("remove committed transaction", root, source))?;
    if let Some(parent) = parent {
        sync_directory(&parent)?;
    }
    Ok(())
}

fn canonical_directory(path: &Path) -> Result<PathBuf, PublicationError> {
    let canonical = fs::canonicalize(path)
        .map_err(|source| io_error("canonicalize directory", path, source))?;
    let metadata = fs::symlink_metadata(&canonical)
        .map_err(|source| io_error("inspect canonical directory", &canonical, source))?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(PublicationError::UnsafeFile { path: canonical });
    }
    Ok(canonical)
}

fn validate_relative(path: &str) -> Result<(), PublicationError> {
    let candidate = Path::new(path);
    if path.is_empty()
        || path.contains('\\')
        || path.eq_ignore_ascii_case("ConfigDumpInfo.xml")
        || candidate
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(PublicationError::InvalidPath {
            path: path.to_owned(),
        });
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn ensure_safe_parents(root: &Path, relative: &str, create: bool) -> Result<(), PublicationError> {
    let mut current = root.to_path_buf();
    let parent = Path::new(relative)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    for component in parent.components() {
        let Component::Normal(name) = component else {
            return Err(PublicationError::InvalidPath {
                path: relative.to_owned(),
            });
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() => {
            }
            Ok(_) => return Err(PublicationError::UnsafeFile { path: current }),
            Err(error) if error.kind() == io::ErrorKind::NotFound && create => {
                fs::create_dir(&current).map_err(|source| {
                    io_error("create managed parent directory", &current, source)
                })?;
                if let Some(parent) = current.parent() {
                    sync_directory(parent)?;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(source) => {
                return Err(io_error(
                    "inspect managed parent directory",
                    &current,
                    source,
                ))
            }
        }
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), PublicationError> {
    #[cfg(windows)]
    {
        let directory = crate::support::windows_fs::open_root(path)
            .map_err(|source| io_error("open directory for sync", path, source))?;
        return crate::support::windows_fs::flush(&directory)
            .map_err(|source| io_error("sync directory", path, source));
    }
    #[cfg(not(windows))]
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| io_error("sync directory", path, source))
}

fn io_error(operation: &'static str, path: &Path, source: io::Error) -> PublicationError {
    PublicationError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;

    use sha2::{Digest, Sha256};

    use crate::use_cases::shadow_merge::plan_manifest_merge;

    use super::{
        recover_publication, DumpTransactionId, ObservedStateGeneration, PublicationError,
        PublicationRequest, TargetIdentity, AFTER_SAFE_PARENT_OPEN, BEFORE_ACTION_INSTALL,
        BEFORE_FIRST_MUTATION, FAIL_JOURNAL_STATUS,
    };

    #[test]
    fn conflict_rejects_the_whole_plan_before_any_write() {
        let fixture = Fixture::new();
        fixture.write_source("a.txt", b"local");
        fixture.write_source("keep.txt", b"old");
        fixture.write_dump("a.txt", b"dump");
        fixture.write_dump("keep.txt", b"new");
        let baseline = manifest(&[("a.txt", b"base"), ("keep.txt", b"old")]);
        let source = fixture.source_manifest(&["a.txt", "keep.txt"]);
        let dump = fixture.dump_manifest(&["a.txt", "keep.txt"]);
        let plan = plan_manifest_merge(&baseline, &source, &dump);

        let error = fixture.request(&plan).prepare().expect_err("conflict");

        assert!(matches!(error, PublicationError::Conflict));
        assert_eq!(fs::read(fixture.source.join("keep.txt")).unwrap(), b"old");
        assert!(!fixture.transaction.exists());
    }

    #[test]
    fn applies_only_manifest_paths_and_preserves_unmanaged_entries() {
        let fixture = Fixture::new();
        fixture.write_source("update.txt", b"old");
        fixture.write_source("delete.txt", b"old-delete");
        fixture.write_source(".git/index", b"git");
        fixture.write_source("work/nested.bin", b"work");
        fixture.write_dump("update.txt", b"new");
        fixture.write_dump("create.txt", b"created");
        let baseline = manifest(&[("update.txt", b"old"), ("delete.txt", b"old-delete")]);
        let source = fixture.source_manifest(&["update.txt", "delete.txt"]);
        let dump = fixture.dump_manifest(&["update.txt", "create.txt"]);
        let plan = plan_manifest_merge(&baseline, &source, &dump);

        fixture
            .request(&plan)
            .prepare()
            .unwrap()
            .apply()
            .unwrap()
            .mark_state_visible(fixture.observed(7))
            .unwrap()
            .commit()
            .unwrap();

        assert_eq!(fs::read(fixture.source.join("update.txt")).unwrap(), b"new");
        assert_eq!(
            fs::read(fixture.source.join("create.txt")).unwrap(),
            b"created"
        );
        assert!(!fixture.source.join("delete.txt").exists());
        assert_eq!(fs::read(fixture.source.join(".git/index")).unwrap(), b"git");
        assert_eq!(
            fs::read(fixture.source.join("work/nested.bin")).unwrap(),
            b"work"
        );
        assert!(!fixture.transaction.exists());
    }

    #[test]
    fn revalidates_every_source_version_before_the_first_mutation() {
        let fixture = Fixture::new();
        fixture.write_source("a.txt", b"old-a");
        fixture.write_source("b.txt", b"old-b");
        fixture.write_dump("a.txt", b"new-a");
        fixture.write_dump("b.txt", b"new-b");
        let baseline = fixture.source_manifest(&["a.txt", "b.txt"]);
        let source = baseline.clone();
        let dump = fixture.dump_manifest(&["a.txt", "b.txt"]);
        let plan = plan_manifest_merge(&baseline, &source, &dump);
        let prepared = fixture.request(&plan).prepare().unwrap();
        let changed = fixture.source.join("b.txt");
        BEFORE_FIRST_MUTATION.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                fs::write(changed, b"foreign").unwrap();
            }));
        });

        let error = prepared.apply().expect_err("TOCTOU");

        assert!(matches!(
            error,
            PublicationError::PublicationAndRollback { .. }
        ));
        assert_eq!(fs::read(fixture.source.join("a.txt")).unwrap(), b"old-a");
        assert!(fixture.transaction.exists());
    }

    #[test]
    fn create_does_not_clobber_a_file_installed_after_revalidation() {
        let fixture = Fixture::new();
        fixture.write_dump("new.txt", b"dump");
        let plan = plan_manifest_merge(
            &BTreeMap::new(),
            &BTreeMap::new(),
            &fixture.dump_manifest(&["new.txt"]),
        );
        let prepared = fixture.request(&plan).prepare().unwrap();
        let foreign = fixture.source.join("new.txt");
        BEFORE_ACTION_INSTALL.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                fs::write(foreign, b"foreign").unwrap();
            }));
        });

        let error = prepared.apply().expect_err("foreign create");

        assert!(matches!(
            error,
            PublicationError::PublicationAndRollback { .. }
        ));
        assert_eq!(
            fs::read(fixture.source.join("new.txt")).unwrap(),
            b"foreign"
        );
    }

    #[test]
    fn recovery_rolls_back_source_applied_and_finishes_state_visible() {
        for state_visible in [false, true] {
            let fixture = Fixture::new();
            fixture.write_source("file.txt", b"old");
            fixture.write_dump("file.txt", b"new");
            let baseline = fixture.source_manifest(&["file.txt"]);
            let plan =
                plan_manifest_merge(&baseline, &baseline, &fixture.dump_manifest(&["file.txt"]));
            let applied = fixture.request(&plan).prepare().unwrap().apply().unwrap();
            if state_visible {
                applied
                    .mark_state_visible(fixture.observed(7))
                    .unwrap()
                    .leave_for_recovery();
            } else {
                applied.leave_for_recovery();
            }

            recover_publication(
                &fixture.transaction,
                &fixture.source,
                &TargetIdentity::new("ib-a"),
                fixture.observed(if state_visible { 7 } else { 6 }),
            )
            .unwrap();

            let expected = if state_visible {
                b"new".as_slice()
            } else {
                b"old".as_slice()
            };
            assert_eq!(fs::read(fixture.source.join("file.txt")).unwrap(), expected);
            assert!(!fixture.transaction.exists());
        }
    }

    #[test]
    fn recovery_is_deterministic_after_every_durable_phase() {
        for phase in ["prepared", "source_applied", "state_visible", "committed"] {
            let fixture = Fixture::new();
            fixture.write_source("file.txt", b"old");
            fixture.write_dump("file.txt", b"new");
            let baseline = fixture.source_manifest(&["file.txt"]);
            let plan =
                plan_manifest_merge(&baseline, &baseline, &fixture.dump_manifest(&["file.txt"]));
            let prepared = fixture.request(&plan).prepare().unwrap();
            match phase {
                "prepared" => prepared.leave_for_recovery(),
                "source_applied" => prepared.apply().unwrap().leave_for_recovery(),
                "state_visible" => prepared
                    .apply()
                    .unwrap()
                    .mark_state_visible(fixture.observed(7))
                    .unwrap()
                    .leave_for_recovery(),
                "committed" => prepared
                    .apply()
                    .unwrap()
                    .mark_state_visible(fixture.observed(7))
                    .unwrap()
                    .mark_committed_for_recovery(),
                _ => unreachable!(),
            }

            let rolls_back = matches!(phase, "prepared" | "source_applied");
            recover_publication(
                &fixture.transaction,
                &fixture.source,
                &TargetIdentity::new("ib-a"),
                fixture.observed(if rolls_back { 6 } else { 7 }),
            )
            .unwrap();

            assert_eq!(
                fs::read(fixture.source.join("file.txt")).unwrap(),
                if rolls_back { b"old" } else { b"new" }
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_managed_parent_without_touching_its_target() {
        use std::os::unix::fs::symlink;

        let fixture = Fixture::new();
        let outside = fixture._temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("file.txt"), b"old").unwrap();
        symlink(&outside, fixture.source.join("linked")).unwrap();
        fixture.write_dump("linked/file.txt", b"dump");
        let plan = plan_manifest_merge(
            &BTreeMap::new(),
            &BTreeMap::new(),
            &fixture.dump_manifest(&["linked/file.txt"]),
        );

        let error = fixture.request(&plan).prepare().expect_err("symlink");

        assert!(matches!(error, PublicationError::UnsafeFile { .. }));
        assert_eq!(fs::read(outside.join("file.txt")).unwrap(), b"old");
    }

    #[test]
    fn recovery_never_clobbers_foreign_content() {
        let fixture = Fixture::new();
        fixture.write_source("file.txt", b"old");
        fixture.write_dump("file.txt", b"new");
        let baseline = fixture.source_manifest(&["file.txt"]);
        let plan = plan_manifest_merge(&baseline, &baseline, &fixture.dump_manifest(&["file.txt"]));
        fixture
            .request(&plan)
            .prepare()
            .unwrap()
            .apply()
            .unwrap()
            .leave_for_recovery();
        fs::write(fixture.source.join("file.txt"), b"foreign").unwrap();

        let error = recover_publication(
            &fixture.transaction,
            &fixture.source,
            &TargetIdentity::new("ib-a"),
            fixture.observed(6),
        )
        .expect_err("foreign");

        assert!(matches!(error, PublicationError::ForeignContent { .. }));
        assert_eq!(
            fs::read(fixture.source.join("file.txt")).unwrap(),
            b"foreign"
        );
        assert!(fixture.transaction.exists());
    }

    #[test]
    fn visible_generation_recovery_preserves_later_user_edit() {
        let fixture = Fixture::new();
        fixture.write_source("file.txt", b"old");
        fixture.write_dump("file.txt", b"new");
        let baseline = fixture.source_manifest(&["file.txt"]);
        let plan = plan_manifest_merge(&baseline, &baseline, &fixture.dump_manifest(&["file.txt"]));
        fixture
            .request(&plan)
            .prepare()
            .unwrap()
            .apply()
            .unwrap()
            .leave_for_recovery();
        fs::write(fixture.source.join("file.txt"), b"later-user-edit").unwrap();

        recover_publication(
            &fixture.transaction,
            &fixture.source,
            &TargetIdentity::new("ib-a"),
            fixture.observed(7),
        )
        .expect("visible generation proves publication committed");

        assert_eq!(
            fs::read(fixture.source.join("file.txt")).unwrap(),
            b"later-user-edit"
        );
        assert!(!fixture.transaction.exists());
    }

    #[test]
    fn same_generation_with_unrelated_transaction_rolls_back_source() {
        let fixture = Fixture::new();
        fixture.write_source("file.txt", b"old");
        fixture.write_dump("file.txt", b"new");
        let baseline = fixture.source_manifest(&["file.txt"]);
        let plan = plan_manifest_merge(&baseline, &baseline, &fixture.dump_manifest(&["file.txt"]));
        fixture
            .request(&plan)
            .prepare()
            .unwrap()
            .apply()
            .unwrap()
            .leave_for_recovery();

        recover_publication(
            &fixture.transaction,
            &fixture.source,
            &TargetIdentity::new("ib-a"),
            ObservedStateGeneration::with_dump_transaction(7, DumpTransactionId::new()),
        )
        .expect("unrelated state must not prove this source publication");

        assert_eq!(fs::read(fixture.source.join("file.txt")).unwrap(), b"old");
        assert!(!fixture.transaction.exists());
    }

    #[test]
    fn publication_journal_rejects_non_uuid_transaction_id() {
        let fixture = Fixture::new();
        fs::create_dir(&fixture.transaction).unwrap();
        fs::write(
            fixture.transaction.join(super::JOURNAL_FILE),
            serde_json::to_vec(&serde_json::json!({
                "generation": 7,
                "target_identity": "ib-a",
                "source_root": fs::canonicalize(&fixture.source).unwrap(),
                "status": "prepared",
                "actions": [],
                "dump_transaction_id": "../not-a-uuid"
            }))
            .unwrap(),
        )
        .unwrap();

        recover_publication(
            &fixture.transaction,
            &fixture.source,
            &TargetIdentity::new("ib-a"),
            fixture.observed(6),
        )
        .expect_err("transaction id must be validated before recovery");

        assert!(fixture.transaction.exists());
    }

    #[cfg(unix)]
    #[test]
    fn parent_swap_after_safe_open_never_mutates_the_symlink_target() {
        use std::os::unix::fs::symlink;

        let fixture = Fixture::new();
        fixture.write_source("managed/file.txt", b"old");
        fixture.write_dump("managed/file.txt", b"new");
        let outside = fixture._temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("file.txt"), b"old").unwrap();
        let baseline = fixture.source_manifest(&["managed/file.txt"]);
        let plan = plan_manifest_merge(
            &baseline,
            &baseline,
            &fixture.dump_manifest(&["managed/file.txt"]),
        );
        let prepared = fixture.request(&plan).prepare().unwrap();
        let managed = fixture.source.join("managed");
        let displaced = fixture.source.join("managed-displaced");
        let outside_for_hook = outside.clone();
        AFTER_SAFE_PARENT_OPEN.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                fs::rename(&managed, &displaced).unwrap();
                symlink(&outside_for_hook, &managed).unwrap();
            }));
        });

        prepared.apply().expect_err("swapped managed parent");

        assert_eq!(fs::read(outside.join("file.txt")).unwrap(), b"old");
    }

    #[cfg(unix)]
    #[test]
    fn publication_accepts_a_symlinked_configured_root() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let real_source = dir.path().join("real-source");
        let configured_source = dir.path().join("configured-source");
        let dump = dir.path().join("dump");
        fs::create_dir(&real_source).unwrap();
        fs::create_dir(&dump).unwrap();
        fs::write(real_source.join("file.txt"), b"old").unwrap();
        fs::write(dump.join("file.txt"), b"new").unwrap();
        symlink(&real_source, &configured_source).unwrap();
        let baseline = manifest(&[("file.txt", b"old")]);
        let source = baseline.clone();
        let dump_manifest = manifest(&[("file.txt", b"new")]);
        let plan = plan_manifest_merge(&baseline, &source, &dump_manifest);

        PublicationRequest::builder(&configured_source, &dump, &plan)
            .transaction_root(&dir.path().join("transaction"))
            .generation(7)
            .target_identity(TargetIdentity::new("ib-a"))
            .dump_transaction_id(DumpTransactionId::new())
            .prepare()
            .unwrap()
            .apply()
            .unwrap();

        assert_eq!(fs::read(real_source.join("file.txt")).unwrap(), b"new");
    }

    #[test]
    fn source_applied_journal_failure_rolls_back_live_source() {
        let fixture = Fixture::new();
        fixture.write_source("file.txt", b"old");
        fixture.write_dump("file.txt", b"new");
        let baseline = fixture.source_manifest(&["file.txt"]);
        let plan = plan_manifest_merge(&baseline, &baseline, &fixture.dump_manifest(&["file.txt"]));
        FAIL_JOURNAL_STATUS.with(|slot| slot.set(Some(super::JournalStatus::SourceApplied)));

        fixture
            .request(&plan)
            .prepare()
            .unwrap()
            .apply()
            .expect_err("journal write must fail");

        assert_eq!(fs::read(fixture.source.join("file.txt")).unwrap(), b"old");
    }

    #[test]
    fn apply_reports_both_publication_and_rollback_failures() {
        let fixture = Fixture::new();
        fixture.write_source("file.txt", b"old");
        fixture.write_dump("file.txt", b"new");
        let baseline = fixture.source_manifest(&["file.txt"]);
        let plan = plan_manifest_merge(&baseline, &baseline, &fixture.dump_manifest(&["file.txt"]));
        let source = fixture.source.join("file.txt");
        BEFORE_ACTION_INSTALL.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                fs::write(source, b"foreign").unwrap();
            }));
        });

        let error = fixture
            .request(&plan)
            .prepare()
            .unwrap()
            .apply()
            .expect_err("publication and rollback must both fail");

        assert!(matches!(
            error,
            PublicationError::PublicationAndRollback { .. }
        ));
        assert_eq!(
            fs::read(fixture.source.join("file.txt")).unwrap(),
            b"foreign"
        );
    }

    #[test]
    fn rollback_preserves_foreign_empty_directory_created_after_prepare() {
        let fixture = Fixture::new();
        fixture.write_dump("new/file.txt", b"new");
        let plan = plan_manifest_merge(
            &BTreeMap::new(),
            &BTreeMap::new(),
            &fixture.dump_manifest(&["new/file.txt"]),
        );
        fixture
            .request(&plan)
            .prepare()
            .unwrap()
            .leave_for_recovery();
        let foreign = fixture.source.join("new");
        fs::create_dir(&foreign).unwrap();

        recover_publication(
            &fixture.transaction,
            &fixture.source,
            &TargetIdentity::new("ib-a"),
            fixture.observed(6),
        )
        .expect("prepared publication rollback");

        assert!(foreign.is_dir());
    }

    #[test]
    fn recovery_rejects_ignored_action_path() {
        let fixture = Fixture::new();
        fixture.write_source(".git/config", b"keep");
        fs::create_dir(&fixture.transaction).unwrap();
        let before = super::hash_hex(b"keep");
        let journal = serde_json::json!({
            "generation": 7,
            "target_identity": "ib-a",
            "dump_transaction_id": fixture.dump_transaction_id.clone(),
            "source_root": fs::canonicalize(&fixture.source).unwrap(),
            "status": "prepared",
            "actions": [{
                "path": ".git/config",
                "before": {"sha256": before},
                "after": {"sha256": null},
                "backup": "00000000.bin",
                "payload": null,
                "claim": ".v8-runner-publication-claim-owned"
            }],
        });
        fs::write(
            fixture.transaction.join(super::JOURNAL_FILE),
            serde_json::to_vec(&journal).unwrap(),
        )
        .unwrap();

        recover_publication(
            &fixture.transaction,
            &fixture.source,
            &TargetIdentity::new("ib-a"),
            fixture.observed(6),
        )
        .expect_err("ignored path must be rejected");

        assert_eq!(
            fs::read(fixture.source.join(".git/config")).unwrap(),
            b"keep"
        );
    }

    struct Fixture {
        _temp: tempfile::TempDir,
        source: std::path::PathBuf,
        dump: std::path::PathBuf,
        transaction: std::path::PathBuf,
        dump_transaction_id: DumpTransactionId,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let source = temp.path().join("source");
            let dump = temp.path().join("dump");
            fs::create_dir_all(&source).unwrap();
            fs::create_dir_all(&dump).unwrap();
            Self {
                transaction: temp.path().join("transaction"),
                _temp: temp,
                source,
                dump,
                dump_transaction_id: DumpTransactionId::new(),
            }
        }

        fn write_source(&self, path: &str, bytes: &[u8]) {
            write(&self.source, path, bytes);
        }
        fn write_dump(&self, path: &str, bytes: &[u8]) {
            write(&self.dump, path, bytes);
        }
        fn source_manifest(&self, paths: &[&str]) -> BTreeMap<String, [u8; 32]> {
            disk_manifest(&self.source, paths)
        }
        fn dump_manifest(&self, paths: &[&str]) -> BTreeMap<String, [u8; 32]> {
            disk_manifest(&self.dump, paths)
        }
        fn request<'a>(
            &'a self,
            plan: &'a crate::use_cases::shadow_merge::ManifestMergePlan,
        ) -> super::PublicationRequestBuilder<'a> {
            PublicationRequest::builder(&self.source, &self.dump, plan)
                .transaction_root(&self.transaction)
                .generation(7)
                .target_identity(TargetIdentity::new("ib-a"))
                .dump_transaction_id(self.dump_transaction_id.clone())
        }

        fn observed(&self, generation: u64) -> ObservedStateGeneration {
            ObservedStateGeneration::with_dump_transaction(
                generation,
                self.dump_transaction_id.clone(),
            )
        }
    }

    fn write(root: &std::path::Path, path: &str, bytes: &[u8]) {
        let target = root.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, bytes).unwrap();
    }

    fn disk_manifest(root: &std::path::Path, paths: &[&str]) -> BTreeMap<String, [u8; 32]> {
        paths
            .iter()
            .map(|path| {
                (
                    (*path).to_owned(),
                    hash(&fs::read(root.join(path)).unwrap()),
                )
            })
            .collect()
    }

    fn manifest(entries: &[(&str, &[u8])]) -> BTreeMap<String, [u8; 32]> {
        entries
            .iter()
            .map(|(path, bytes)| ((*path).to_owned(), hash(bytes)))
            .collect()
    }

    fn hash(bytes: &[u8]) -> [u8; 32] {
        Sha256::digest(bytes).into()
    }
}
