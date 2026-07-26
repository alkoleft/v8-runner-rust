use redb::{
    CommitError, Database, DatabaseError, ReadableTable, StorageError as RedbStorageError,
    TableDefinition, TableError, TransactionError,
};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use thiserror::Error;
use uuid::Uuid;

use crate::domain::runtime_state::DumpTransactionId;

#[cfg(test)]
thread_local! {
    static BEFORE_RECOVERY_CLAIM_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static BEFORE_MISSING_PUBLISH_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static AFTER_RECOVERY_CLAIM_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static BEFORE_RECOVERY_ROLLBACK_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static FORCE_RECOVERY_PUBLISH_FAILURE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static BEFORE_EXACT_OBSERVATION_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

/// `redb` table with per-file mtimes keyed by relative path.
pub const FILES_MTIME: TableDefinition<&str, u64> = TableDefinition::new("files_mtime");
/// `redb` table with per-file hashes keyed by relative path.
pub const FILES_HASH: TableDefinition<&str, &str> = TableDefinition::new("files_hash");
/// `redb` table with storage metadata.
pub const META: TableDefinition<&str, u64> = TableDefinition::new("meta");
/// Metadata key storing the latest scan watermark.
pub const META_KEY_WATERMARK: &str = "watermark";
/// Metadata key storing optimistic-lock generation.
pub const META_KEY_GENERATION: &str = "generation";
const META_KEY_DUMP_TRANSACTION_HI: &str = "dump_transaction_hi";
const META_KEY_DUMP_TRANSACTION_LO: &str = "dump_transaction_lo";

/// Persisted state for one file entry inside the storage snapshot.
#[derive(Debug, Clone)]
pub struct StoredFileState {
    pub mtime_ns: u64,
    pub hash: String,
}

/// Full snapshot loaded from storage, including metadata used by the scanner.
#[derive(Debug, Clone, Default)]
pub struct StorageSnapshot {
    pub entries: HashMap<String, StoredFileState>,
    pub watermark: Option<u64>,
    pub generation: u64,
}

/// Distinguishes a never-initialized scoped storage from a committed snapshot.
#[derive(Debug, Clone)]
pub enum HashStorageLoad {
    MissingPath,
    ExistingUninitialized,
    Initialized(StorageSnapshot),
}

#[derive(Debug, Clone)]
pub enum ObservedHashStorage {
    MissingPath,
    ExistingUninitialized(ObservedStorageState),
    Initialized(StorageSnapshot),
    Recoverable(ObservedStorageState),
}

/// Exact storage observation attached to a prepared source snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservedStorageState {
    MissingPath,
    ExistingUninitialized {
        identity: StorageFileIdentity,
        sha256: String,
    },
    Initialized {
        generation: u64,
    },
    Recoverable {
        generation: u64,
        identity: StorageFileIdentity,
        sha256: String,
    },
}

impl ObservedStorageState {
    pub fn generation(&self) -> u64 {
        match self {
            Self::MissingPath | Self::ExistingUninitialized { .. } => 0,
            Self::Initialized { generation } | Self::Recoverable { generation, .. } => *generation,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageFileIdentity {
    pub len: u64,
    #[cfg(unix)]
    pub device: u64,
    #[cfg(unix)]
    pub inode: u64,
    #[cfg(windows)]
    pub volume_serial: u32,
    #[cfg(windows)]
    pub file_index: u64,
}

/// Storage-layer failures split into recoverable and hard categories.
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("recoverable storage problem for '{path}': {reason}")]
    Recoverable { path: PathBuf, reason: String },

    #[error("hard storage problem for '{path}': {reason}")]
    Hard { path: PathBuf, reason: String },

    #[error("storage state changed concurrently for '{path}': expected generation {expected}, found {actual:?}")]
    ConcurrentStateModified {
        path: PathBuf,
        expected: u64,
        actual: Option<u64>,
    },
}

impl StorageError {
    /// Whether the caller may ignore the storage state and rebuild it from disk.
    pub fn is_recoverable(&self) -> bool {
        matches!(self, Self::Recoverable { .. })
    }
}

/// `redb`-backed hash storage for one logical source-set.
#[derive(Debug, Clone)]
pub struct HashStorage {
    path: PathBuf,
}

impl HashStorage {
    /// Create a storage handle for the given `redb` file path.
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Return the underlying storage file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Capture the exact bytes and file identity of a recoverable storage file.
    pub fn recoverable_observation(&self) -> Result<ObservedStorageState, StorageError> {
        let generation = match self.recoverable_generation() {
            Ok(generation) => generation,
            Err(error) if error.is_recoverable() => 0,
            Err(error) => return Err(error),
        };
        let (identity, sha256) = self.raw_file_token()?;
        let observed = ObservedStorageState::Recoverable {
            generation,
            identity,
            sha256,
        };
        self.verify_observation_class(&observed)?;
        Ok(observed)
    }

    pub(crate) fn uninitialized_observation(&self) -> Result<ObservedStorageState, StorageError> {
        let (identity, sha256) = self.raw_file_token()?;
        let observed = ObservedStorageState::ExistingUninitialized { identity, sha256 };
        self.verify_observation_class(&observed)?;
        Ok(observed)
    }

    /// Classify storage and capture any exact-file token as one validated observation.
    pub fn observe_state(&self) -> Result<ObservedHashStorage, StorageError> {
        let classified = self.load_state();
        #[cfg(test)]
        run_test_hook(&BEFORE_EXACT_OBSERVATION_HOOK);
        match classified {
            Ok(HashStorageLoad::MissingPath) => Ok(ObservedHashStorage::MissingPath),
            Ok(HashStorageLoad::ExistingUninitialized) => self
                .uninitialized_observation()
                .map(ObservedHashStorage::ExistingUninitialized),
            Ok(HashStorageLoad::Initialized(snapshot)) => {
                Ok(ObservedHashStorage::Initialized(snapshot))
            }
            Err(error) if error.is_recoverable() => self
                .recoverable_observation()
                .map(ObservedHashStorage::Recoverable),
            Err(error) => Err(error),
        }
    }

    fn verify_observation_class(
        &self,
        expected: &ObservedStorageState,
    ) -> Result<(), StorageError> {
        let still_same_class = match (expected, self.load_state()) {
            (
                ObservedStorageState::ExistingUninitialized { .. },
                Ok(HashStorageLoad::ExistingUninitialized),
            ) => true,
            (ObservedStorageState::Recoverable { .. }, Err(error)) if error.is_recoverable() => {
                true
            }
            (_, Err(error)) if !error.is_recoverable() => return Err(error),
            _ => false,
        };
        if still_same_class {
            Ok(())
        } else {
            Err(StorageError::ConcurrentStateModified {
                path: self.path.clone(),
                expected: expected.generation(),
                actual: self.observed_generation_for_conflict()?,
            })
        }
    }

    fn raw_file_token(&self) -> Result<(StorageFileIdentity, String), StorageError> {
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
        let mut file = options
            .open(&self.path)
            .map_err(|error| StorageError::Hard {
                path: self.path.clone(),
                reason: format!("open storage without following links: {error}"),
            })?;
        let before = file.metadata().map_err(|error| StorageError::Hard {
            path: self.path.clone(),
            reason: format!("inspect opened storage: {error}"),
        })?;
        if !before.file_type().is_file() || storage_metadata_is_reparse_point(&before) {
            return Err(StorageError::Hard {
                path: self.path.clone(),
                reason: "recoverable storage is not a regular file".to_owned(),
            });
        }
        let identity = storage_file_identity(&file, &before, &self.path)?;
        let mut digest = Sha256::new();
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer).map_err(|error| StorageError::Hard {
                path: self.path.clone(),
                reason: format!("read recoverable storage bytes: {error}"),
            })?;
            if read == 0 {
                break;
            }
            digest.update(&buffer[..read]);
        }
        let after = file.metadata().map_err(|error| StorageError::Hard {
            path: self.path.clone(),
            reason: format!("re-inspect opened storage: {error}"),
        })?;
        if storage_file_identity(&file, &after, &self.path)? != identity {
            return Err(StorageError::ConcurrentStateModified {
                path: self.path.clone(),
                expected: 0,
                actual: None,
            });
        }
        Ok((identity, format!("{:x}", digest.finalize())))
    }

    /// Load typed initialization state without a separate filesystem existence check.
    pub fn load_state(&self) -> Result<HashStorageLoad, StorageError> {
        match self.path.try_exists() {
            Ok(false) => return Ok(HashStorageLoad::MissingPath),
            Ok(true) => {}
            Err(error) => return Err(map_filesystem_lookup_error(&self.path, error)),
        }
        let db = match Database::open(&self.path) {
            Ok(database) => database,
            Err(database_error) => match self.path.try_exists() {
                Ok(false) => return Ok(HashStorageLoad::MissingPath),
                Ok(true) => return Err(map_database_error(&self.path, database_error)),
                Err(error) => return Err(map_filesystem_lookup_error(&self.path, error)),
            },
        };
        let tx = db
            .begin_read()
            .map_err(|e| map_tx_error(&self.path, e, "begin read"))?;

        let mtime_tbl = match tx.open_table(FILES_MTIME) {
            Ok(t) => Some(t),
            Err(TableError::TableDoesNotExist(_)) => None,
            Err(e) => return Err(map_table_error(&self.path, e)),
        };
        let hash_tbl = match tx.open_table(FILES_HASH) {
            Ok(t) => Some(t),
            Err(TableError::TableDoesNotExist(_)) => None,
            Err(e) => return Err(map_table_error(&self.path, e)),
        };
        let meta_tbl = match tx.open_table(META) {
            Ok(t) => Some(t),
            Err(TableError::TableDoesNotExist(_)) => None,
            Err(e) => return Err(map_table_error(&self.path, e)),
        };

        let mtime_exists = mtime_tbl.is_some();
        let hash_exists = hash_tbl.is_some();
        if !mtime_exists || !hash_exists {
            if !mtime_exists && !hash_exists {
                return if meta_tbl.is_none() {
                    Ok(HashStorageLoad::ExistingUninitialized)
                } else {
                    Err(StorageError::Recoverable {
                        path: self.path.clone(),
                        reason: "metadata exists without snapshot tables".to_owned(),
                    })
                };
            }
            return Err(StorageError::Recoverable {
                path: self.path.clone(),
                reason: "mtime/hash tables are inconsistent".to_owned(),
            });
        }
        let (Some(mtime_tbl), Some(hash_tbl)) = (mtime_tbl, hash_tbl) else {
            return Err(StorageError::Recoverable {
                path: self.path.clone(),
                reason: "snapshot tables disappeared while loading storage".to_owned(),
            });
        };

        let mut entries = HashMap::new();
        for item in mtime_tbl
            .iter()
            .map_err(|e| map_storage_error(&self.path, "iterate mtime table", e))?
        {
            let (k, mtime) =
                item.map_err(|e| map_storage_error(&self.path, "read mtime row", e))?;
            let rel = k.value().to_owned();
            let hash = hash_tbl
                .get(rel.as_str())
                .map_err(|e| map_storage_error(&self.path, "read hash row", e))?
                .map(|v| v.value().to_owned())
                .ok_or_else(|| StorageError::Recoverable {
                    path: self.path.clone(),
                    reason: format!("missing hash for key '{rel}'"),
                })?;
            entries.insert(
                rel,
                StoredFileState {
                    mtime_ns: mtime.value(),
                    hash,
                },
            );
        }

        // Detect hash-only orphan rows.
        for item in hash_tbl
            .iter()
            .map_err(|e| map_storage_error(&self.path, "iterate hash table", e))?
        {
            let (k, _) = item.map_err(|e| map_storage_error(&self.path, "read hash row", e))?;
            let rel = k.value().to_owned();
            if !entries.contains_key(&rel) {
                return Err(StorageError::Recoverable {
                    path: self.path.clone(),
                    reason: format!("missing mtime for key '{rel}'"),
                });
            }
        }

        Ok(HashStorageLoad::Initialized(StorageSnapshot {
            entries,
            watermark: read_watermark(meta_tbl.as_ref(), &self.path)?,
            generation: read_generation(meta_tbl.as_ref(), &self.path)?,
        }))
    }

    #[cfg(test)]
    pub fn load_snapshot(&self) -> Result<StorageSnapshot, StorageError> {
        match self.load_state()? {
            HashStorageLoad::MissingPath | HashStorageLoad::ExistingUninitialized => {
                Ok(StorageSnapshot::default())
            }
            HashStorageLoad::Initialized(snapshot) => Ok(snapshot),
        }
    }

    /// Persist a full snapshot if the caller still owns the expected generation.
    pub fn commit_snapshot(
        &self,
        snapshot: &HashMap<String, StoredFileState>,
        watermark: u64,
        expected_generation: u64,
    ) -> Result<(), StorageError> {
        self.ensure_parent_dir()?;
        let db = Database::create(&self.path).map_err(|e| map_database_error(&self.path, e))?;
        let tx = db
            .begin_write()
            .map_err(|e| map_tx_error(&self.path, e, "begin write"))?;

        {
            let mut meta = tx
                .open_table(META)
                .map_err(|e| map_table_error(&self.path, e))?;
            let current_generation = meta
                .get(META_KEY_GENERATION)
                .map_err(|e| map_storage_error(&self.path, "read generation", e))?
                .map(|v| v.value())
                .unwrap_or(0);
            if current_generation != expected_generation {
                return Err(StorageError::ConcurrentStateModified {
                    path: self.path.clone(),
                    expected: expected_generation,
                    actual: Some(current_generation),
                });
            }

            let mut mtime = tx
                .open_table(FILES_MTIME)
                .map_err(|e| map_table_error(&self.path, e))?;
            let mut hash = tx
                .open_table(FILES_HASH)
                .map_err(|e| map_table_error(&self.path, e))?;
            sync_file_tables(&self.path, &mut mtime, &mut hash, snapshot)?;

            meta.insert(META_KEY_WATERMARK, watermark)
                .map_err(|e| map_storage_error(&self.path, "write watermark", e))?;
            meta.insert(META_KEY_GENERATION, expected_generation + 1)
                .map_err(|e| map_storage_error(&self.path, "write generation", e))?;
            meta.remove(META_KEY_DUMP_TRANSACTION_HI)
                .map_err(|e| map_storage_error(&self.path, "clear dump transaction", e))?;
            meta.remove(META_KEY_DUMP_TRANSACTION_LO)
                .map_err(|e| map_storage_error(&self.path, "clear dump transaction", e))?;
        }

        tx.commit().map_err(|e| map_commit_error(&self.path, e))?;
        Ok(())
    }

    /// Create a complete replacement database with an explicit committed generation.
    ///
    /// The caller must create this at a private staging path and publish it only after
    /// independently verifying ownership of the live generation.
    pub(crate) fn create_replacement(
        path: PathBuf,
        snapshot: &HashMap<String, StoredFileState>,
        watermark: u64,
        generation: u64,
    ) -> Result<(), StorageError> {
        Self::create_replacement_with_dump_transaction(path, snapshot, watermark, generation, None)
    }

    pub(crate) fn create_dump_replacement(
        path: PathBuf,
        snapshot: &HashMap<String, StoredFileState>,
        watermark: u64,
        generation: u64,
        transaction_id: &DumpTransactionId,
    ) -> Result<(), StorageError> {
        Self::create_replacement_with_dump_transaction(
            path,
            snapshot,
            watermark,
            generation,
            Some(transaction_id),
        )
    }

    fn create_replacement_with_dump_transaction(
        path: PathBuf,
        snapshot: &HashMap<String, StoredFileState>,
        watermark: u64,
        generation: u64,
        dump_transaction_id: Option<&DumpTransactionId>,
    ) -> Result<(), StorageError> {
        let storage = Self::new(path);
        storage.ensure_parent_dir()?;
        let db = Database::create(&storage.path)
            .map_err(|error| map_database_error(&storage.path, error))?;
        let transaction = db
            .begin_write()
            .map_err(|error| map_tx_error(&storage.path, error, "begin replacement write"))?;
        {
            let mut metadata = transaction
                .open_table(META)
                .map_err(|error| map_table_error(&storage.path, error))?;
            let mut mtimes = transaction
                .open_table(FILES_MTIME)
                .map_err(|error| map_table_error(&storage.path, error))?;
            let mut hashes = transaction
                .open_table(FILES_HASH)
                .map_err(|error| map_table_error(&storage.path, error))?;
            sync_file_tables(&storage.path, &mut mtimes, &mut hashes, snapshot)?;
            metadata
                .insert(META_KEY_WATERMARK, watermark)
                .map_err(|error| map_storage_error(&storage.path, "write watermark", error))?;
            metadata
                .insert(META_KEY_GENERATION, generation)
                .map_err(|error| map_storage_error(&storage.path, "write generation", error))?;
            if let Some(transaction_id) = dump_transaction_id {
                let value = transaction_id.as_u128();
                metadata
                    .insert(META_KEY_DUMP_TRANSACTION_HI, (value >> 64) as u64)
                    .map_err(|error| {
                        map_storage_error(&storage.path, "write dump transaction", error)
                    })?;
                metadata
                    .insert(META_KEY_DUMP_TRANSACTION_LO, value as u64)
                    .map_err(|error| {
                        map_storage_error(&storage.path, "write dump transaction", error)
                    })?;
            }
        }
        transaction
            .commit()
            .map_err(|error| map_commit_error(&storage.path, error))
    }

    /// Read the current optimistic-lock generation.
    pub fn current_generation(&self) -> Result<u64, StorageError> {
        Ok(match self.load_state()? {
            HashStorageLoad::MissingPath | HashStorageLoad::ExistingUninitialized => 0,
            HashStorageLoad::Initialized(snapshot) => snapshot.generation,
        })
    }

    pub(crate) fn current_dump_transaction_id(
        &self,
    ) -> Result<Option<DumpTransactionId>, StorageError> {
        let database = match Database::open(&self.path) {
            Ok(database) => database,
            Err(database_error) => match self.path.try_exists() {
                Ok(false) => return Ok(None),
                Ok(true) => return Err(map_database_error(&self.path, database_error)),
                Err(error) => return Err(map_filesystem_lookup_error(&self.path, error)),
            },
        };
        let transaction = database
            .begin_read()
            .map_err(|error| map_tx_error(&self.path, error, "begin dump transaction read"))?;
        let metadata = match transaction.open_table(META) {
            Ok(metadata) => metadata,
            Err(TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(error) => return Err(map_table_error(&self.path, error)),
        };
        let high = metadata
            .get(META_KEY_DUMP_TRANSACTION_HI)
            .map_err(|error| map_storage_error(&self.path, "read dump transaction", error))?
            .map(|value| value.value());
        let low = metadata
            .get(META_KEY_DUMP_TRANSACTION_LO)
            .map_err(|error| map_storage_error(&self.path, "read dump transaction", error))?
            .map(|value| value.value());
        match (high, low) {
            (None, None) => Ok(None),
            (Some(high), Some(low)) => Ok(Some(DumpTransactionId::from_u128(
                (u128::from(high) << 64) | u128::from(low),
            ))),
            _ => Err(StorageError::Recoverable {
                path: self.path.clone(),
                reason: "dump transaction metadata is incomplete".to_owned(),
            }),
        }
    }

    /// Read the optimistic-lock generation even when snapshot tables are recoverably incomplete.
    pub fn recoverable_generation(&self) -> Result<u64, StorageError> {
        let database =
            Database::open(&self.path).map_err(|error| map_database_error(&self.path, error))?;
        let transaction = database
            .begin_read()
            .map_err(|error| map_tx_error(&self.path, error, "begin recovery read"))?;
        let meta = match transaction.open_table(META) {
            Ok(table) => Some(table),
            Err(TableError::TableDoesNotExist(_)) => None,
            Err(error) => return Err(map_table_error(&self.path, error)),
        };
        read_generation(meta.as_ref(), &self.path)
    }

    /// Commit against the exact storage state observed while preparing the snapshot.
    pub fn commit_observed_snapshot(
        &self,
        snapshot: &HashMap<String, StoredFileState>,
        watermark: u64,
        observed: &ObservedStorageState,
    ) -> Result<(), StorageError> {
        match observed {
            ObservedStorageState::MissingPath => {
                if self
                    .path
                    .try_exists()
                    .map_err(|error| map_filesystem_lookup_error(&self.path, error))?
                {
                    return Err(StorageError::ConcurrentStateModified {
                        path: self.path.clone(),
                        expected: 0,
                        actual: self.observed_generation_for_conflict()?,
                    });
                }
                self.commit_missing_observation(snapshot, watermark)
            }
            ObservedStorageState::ExistingUninitialized { .. } => {
                self.commit_claimed_observation(snapshot, watermark, observed, 0)
            }
            ObservedStorageState::Initialized { generation } => {
                self.commit_snapshot(snapshot, watermark, *generation)
            }
            ObservedStorageState::Recoverable { generation, .. } => {
                self.commit_claimed_observation(snapshot, watermark, observed, *generation)
            }
        }
    }

    fn commit_missing_observation(
        &self,
        snapshot: &HashMap<String, StoredFileState>,
        watermark: u64,
    ) -> Result<(), StorageError> {
        self.ensure_parent_dir()?;
        let staged = self
            .path
            .with_extension(format!("missing-{}.redb", Uuid::new_v4()));
        Self::create_replacement(staged.clone(), snapshot, watermark, 1)?;
        #[cfg(test)]
        BEFORE_MISSING_PUBLISH_HOOK.with(|cell| {
            if let Some(hook) = cell.borrow_mut().take() {
                hook();
            }
        });
        match fs::hard_link(&staged, &self.path) {
            Ok(()) => sync_parent_namespace(&self.path)?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                cleanup_staged_file(&staged, "concurrent storage was preserved")?;
                return Err(StorageError::ConcurrentStateModified {
                    path: self.path.clone(),
                    expected: 0,
                    actual: self.observed_generation_for_conflict()?,
                });
            }
            Err(error) => {
                cleanup_staged_file(&staged, &format!("atomic publication failed ({error})"))?;
                return Err(StorageError::Hard {
                    path: self.path.clone(),
                    reason: format!("failed to publish missing-state snapshot atomically: {error}"),
                });
            }
        }
        cleanup_staged_file(&staged, "snapshot was published")
    }

    fn commit_claimed_observation(
        &self,
        snapshot: &HashMap<String, StoredFileState>,
        watermark: u64,
        expected: &ObservedStorageState,
        expected_generation: u64,
    ) -> Result<(), StorageError> {
        if matches!(
            expected,
            ObservedStorageState::MissingPath | ObservedStorageState::Initialized { .. }
        ) {
            return Err(StorageError::Hard {
                path: self.path.clone(),
                reason: "claim requires an exact existing-file observation".to_owned(),
            });
        }
        self.ensure_parent_dir()?;
        let suffix = Uuid::new_v4();
        let staged = self
            .path
            .with_extension(format!("replacement-{suffix}.redb"));
        let claimed = self.path.with_extension(format!("corrupt-{suffix}.redb"));
        Self::create_replacement(
            staged.clone(),
            snapshot,
            watermark,
            expected_generation
                .checked_add(1)
                .ok_or_else(|| StorageError::Hard {
                    path: self.path.clone(),
                    reason: "storage generation cannot advance beyond u64::MAX".to_owned(),
                })?,
        )?;

        #[cfg(test)]
        BEFORE_RECOVERY_CLAIM_HOOK.with(|cell| {
            if let Some(hook) = cell.borrow_mut().take() {
                hook();
            }
        });
        if let Err(claim_error) = fs::rename(&self.path, &claimed) {
            let claim_error = StorageError::Hard {
                path: self.path.clone(),
                reason: format!("failed to claim observed recoverable storage: {claim_error}"),
            };
            return match cleanup_staged_file(&staged, "storage claim failed") {
                Ok(()) => Err(claim_error),
                Err(cleanup_error) => Err(combine_storage_errors(
                    &self.path,
                    "storage claim and staging cleanup both failed",
                    claim_error,
                    cleanup_error,
                )),
            };
        }
        if let Err(sync_error) = sync_parent_namespace(&self.path) {
            let recovery = restore_and_cleanup(&claimed, &self.path, &staged);
            return match recovery {
                Ok(_) => Err(sync_error),
                Err(recovery_error) => Err(combine_storage_errors(
                    &self.path,
                    "claim directory sync failed",
                    sync_error,
                    recovery_error,
                )),
            };
        }
        let claimed_storage = Self::new(claimed.clone());
        let actual = match expected {
            ObservedStorageState::ExistingUninitialized { .. } => {
                claimed_storage.uninitialized_observation()
            }
            ObservedStorageState::Recoverable { .. } => claimed_storage.recoverable_observation(),
            ObservedStorageState::MissingPath | ObservedStorageState::Initialized { .. } => {
                restore_and_cleanup(&claimed, &self.path, &staged)?;
                return Err(StorageError::Hard {
                    path: self.path.clone(),
                    reason: "invalid storage observation reached claim validation".to_owned(),
                });
            }
        };
        let actual_generation = actual.as_ref().ok().map(ObservedStorageState::generation);
        if actual.as_ref().ok() != Some(expected) {
            restore_and_cleanup(&claimed, &self.path, &staged)?;
            return Err(StorageError::ConcurrentStateModified {
                path: self.path.clone(),
                expected: expected_generation,
                actual: actual_generation,
            });
        }

        #[cfg(test)]
        AFTER_RECOVERY_CLAIM_HOOK.with(|cell| {
            if let Some(hook) = cell.borrow_mut().take() {
                hook();
            }
        });
        let publication = publish_claimed_replacement(&staged, &self.path);
        match publication {
            Ok(()) => {
                sync_parent_namespace(&self.path)?;
                cleanup_staged_file(&staged, "replacement was published")?;
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                cleanup_staged_file(&staged, "concurrent storage was preserved")?;
                Err(StorageError::ConcurrentStateModified {
                    path: self.path.clone(),
                    expected: expected_generation,
                    actual: self.observed_generation_for_conflict()?,
                })
            }
            Err(error) => {
                #[cfg(test)]
                BEFORE_RECOVERY_ROLLBACK_HOOK.with(|cell| {
                    if let Some(hook) = cell.borrow_mut().take() {
                        hook();
                    }
                });
                if restore_and_cleanup(&claimed, &self.path, &staged)? {
                    return Err(StorageError::ConcurrentStateModified {
                        path: self.path.clone(),
                        expected: expected_generation,
                        actual: self.observed_generation_for_conflict()?,
                    });
                }
                Err(StorageError::Hard {
                    path: self.path.clone(),
                    reason: format!("failed to publish replacement: {error}"),
                })
            }
        }
    }

    fn observed_generation_for_conflict(&self) -> Result<Option<u64>, StorageError> {
        match self.current_generation() {
            Ok(generation) => Ok(Some(generation)),
            Err(error) if error.is_recoverable() => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn ensure_parent_dir(&self) -> Result<(), StorageError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| StorageError::Hard {
                path: parent.to_path_buf(),
                reason: format!("failed to create parent dir: {e}"),
            })?;
        }
        Ok(())
    }
}

fn storage_file_identity(
    file: &fs::File,
    metadata: &fs::Metadata,
    path: &Path,
) -> Result<StorageFileIdentity, StorageError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let _ = file;
        let _ = path;
        return Ok(StorageFileIdentity {
            len: metadata.len(),
            device: metadata.dev(),
            inode: metadata.ino(),
        });
    }
    #[cfg(windows)]
    {
        use std::mem::MaybeUninit;
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        };
        let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
        // SAFETY: the handle is borrowed from a live File; the API initializes
        // the output structure on success.
        let succeeded = unsafe {
            GetFileInformationByHandle(file.as_raw_handle().cast(), information.as_mut_ptr())
        };
        if succeeded == 0 {
            return Err(StorageError::Hard {
                path: path.to_path_buf(),
                reason: format!(
                    "query Windows storage file identity: {}",
                    std::io::Error::last_os_error()
                ),
            });
        }
        // SAFETY: guarded by the successful Win32 call above.
        let information = unsafe { information.assume_init() };
        return Ok(StorageFileIdentity {
            len: metadata.len(),
            volume_serial: information.dwVolumeSerialNumber,
            file_index: (u64::from(information.nFileIndexHigh) << 32)
                | u64::from(information.nFileIndexLow),
        });
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        let _ = path;
        Ok(StorageFileIdentity {
            len: metadata.len(),
        })
    }
}

#[cfg(windows)]
fn storage_metadata_is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn storage_metadata_is_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
}

#[cfg(test)]
fn run_test_hook(
    hook: &'static std::thread::LocalKey<std::cell::RefCell<Option<Box<dyn FnOnce()>>>>,
) {
    hook.with(|cell| {
        if let Some(hook) = cell.borrow_mut().take() {
            hook();
        }
    });
}

fn cleanup_staged_file(path: &Path, context: &str) -> Result<(), StorageError> {
    fs::remove_file(path).map_err(|error| StorageError::Hard {
        path: path.to_path_buf(),
        reason: format!("{context}, but staging cleanup failed: {error}"),
    })?;
    sync_parent_namespace(path)
}

/// Restore the claimed file without replacing a concurrently published target.
/// Returns `true` when a concurrent target was found and the claim was retained.
fn restore_claim_no_clobber(claimed: &Path, target: &Path) -> Result<bool, StorageError> {
    match fs::hard_link(claimed, target) {
        Ok(()) => {
            sync_parent_namespace(target)?;
            fs::remove_file(claimed).map_err(|error| StorageError::Hard {
                path: claimed.to_path_buf(),
                reason: format!("claimed storage was restored but claim cleanup failed: {error}"),
            })?;
            sync_parent_namespace(claimed)?;
            Ok(false)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(true),
        Err(error) => Err(StorageError::Hard {
            path: target.to_path_buf(),
            reason: format!("failed to restore claimed storage without overwriting: {error}"),
        }),
    }
}

fn restore_and_cleanup(claimed: &Path, target: &Path, staged: &Path) -> Result<bool, StorageError> {
    let restore = restore_claim_no_clobber(claimed, target);
    let cleanup = cleanup_staged_file(staged, "claimed storage recovery was attempted");
    match (restore, cleanup) {
        (Ok(concurrent), Ok(())) => Ok(concurrent),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(restore_error), Err(cleanup_error)) => Err(combine_storage_errors(
            target,
            "claim restoration and staging cleanup both failed",
            restore_error,
            cleanup_error,
        )),
    }
}

fn combine_storage_errors(
    path: &Path,
    context: &str,
    first: StorageError,
    second: StorageError,
) -> StorageError {
    StorageError::Hard {
        path: path.to_path_buf(),
        reason: format!("{context}: {first}; additionally: {second}"),
    }
}

fn publish_claimed_replacement(staged: &Path, target: &Path) -> std::io::Result<()> {
    #[cfg(test)]
    if FORCE_RECOVERY_PUBLISH_FAILURE.with(|forced| forced.replace(false)) {
        return Err(std::io::Error::other("forced recovery publication failure"));
    }
    fs::hard_link(staged, target)
}

#[cfg(unix)]
fn sync_parent_namespace(path: &Path) -> Result<(), StorageError> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| StorageError::Hard {
            path: parent.to_path_buf(),
            reason: format!("failed to sync storage directory: {error}"),
        })
}

#[cfg(not(unix))]
fn sync_parent_namespace(_path: &Path) -> Result<(), StorageError> {
    Ok(())
}

fn read_watermark(
    meta: Option<&redb::ReadOnlyTable<&str, u64>>,
    path: &Path,
) -> Result<Option<u64>, StorageError> {
    let Some(meta) = meta else {
        return Ok(None);
    };
    meta.get(META_KEY_WATERMARK)
        .map_err(|e| map_storage_error(path, "read watermark", e))
        .map(|opt| opt.map(|v| v.value()))
}

fn read_generation(
    meta: Option<&redb::ReadOnlyTable<&str, u64>>,
    path: &Path,
) -> Result<u64, StorageError> {
    let Some(meta) = meta else {
        return Ok(0);
    };
    meta.get(META_KEY_GENERATION)
        .map_err(|e| map_storage_error(path, "read generation", e))
        .map(|opt| opt.map(|v| v.value()).unwrap_or(0))
}

fn sync_file_tables(
    path: &Path,
    mtime: &mut redb::Table<&str, u64>,
    hash: &mut redb::Table<&str, &str>,
    snapshot: &HashMap<String, StoredFileState>,
) -> Result<(), StorageError> {
    let keys: Vec<String> = mtime
        .iter()
        .map_err(|e| map_storage_error(path, "iterate mtime table", e))?
        .map(|row| {
            row.map(|(k, _)| k.value().to_owned())
                .map_err(|e| map_storage_error(path, "read mtime row", e))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let target_keys: HashSet<&str> = snapshot.keys().map(String::as_str).collect();
    for key in keys {
        if !target_keys.contains(key.as_str()) {
            mtime
                .remove(key.as_str())
                .map_err(|e| map_storage_error(path, "remove stale mtime key", e))?;
            hash.remove(key.as_str())
                .map_err(|e| map_storage_error(path, "remove stale hash key", e))?;
        }
    }

    for (key, state) in snapshot {
        mtime
            .insert(key.as_str(), state.mtime_ns)
            .map_err(|e| map_storage_error(path, "insert mtime", e))?;
        hash.insert(key.as_str(), state.hash.as_str())
            .map_err(|e| map_storage_error(path, "insert hash", e))?;
    }
    Ok(())
}

fn map_database_error(path: &Path, err: DatabaseError) -> StorageError {
    match err {
        DatabaseError::Storage(RedbStorageError::Corrupted(msg)) => StorageError::Recoverable {
            path: path.to_path_buf(),
            reason: msg,
        },
        DatabaseError::Storage(RedbStorageError::PreviousIo) => StorageError::Recoverable {
            path: path.to_path_buf(),
            reason: "previous I/O error in database".to_owned(),
        },
        DatabaseError::Storage(RedbStorageError::Io(e)) => map_io_error(path, "I/O error", e),
        DatabaseError::DatabaseAlreadyOpen => StorageError::Hard {
            path: path.to_path_buf(),
            reason: "database is already open".to_owned(),
        },
        other => StorageError::Recoverable {
            path: path.to_path_buf(),
            reason: other.to_string(),
        },
    }
}

fn map_table_error(path: &Path, err: TableError) -> StorageError {
    match err {
        TableError::Storage(RedbStorageError::Io(e)) => map_io_error(path, "table I/O error", e),
        TableError::Storage(RedbStorageError::Corrupted(msg)) => StorageError::Recoverable {
            path: path.to_path_buf(),
            reason: msg,
        },
        other => StorageError::Recoverable {
            path: path.to_path_buf(),
            reason: other.to_string(),
        },
    }
}

fn map_tx_error(path: &Path, err: TransactionError, context: &str) -> StorageError {
    match err {
        TransactionError::Storage(RedbStorageError::Io(e)) => map_io_error(path, context, e),
        other => StorageError::Recoverable {
            path: path.to_path_buf(),
            reason: format!("{context}: {other}"),
        },
    }
}

fn map_storage_error(path: &Path, context: &str, err: RedbStorageError) -> StorageError {
    match err {
        RedbStorageError::Io(error) => map_io_error(path, context, error),
        other => StorageError::Recoverable {
            path: path.to_path_buf(),
            reason: format!("{context}: {other}"),
        },
    }
}

fn map_io_error(path: &Path, context: &str, error: std::io::Error) -> StorageError {
    let reason = format!("{context}: {error}");
    if error.kind() == std::io::ErrorKind::InvalidData {
        StorageError::Recoverable {
            path: path.to_path_buf(),
            reason,
        }
    } else {
        StorageError::Hard {
            path: path.to_path_buf(),
            reason,
        }
    }
}

fn map_commit_error(path: &Path, error: CommitError) -> StorageError {
    match error {
        CommitError::Storage(error) => map_storage_error(path, "commit transaction", error),
        other => StorageError::Recoverable {
            path: path.to_path_buf(),
            reason: format!("commit transaction: {other}"),
        },
    }
}

fn map_filesystem_lookup_error(path: &Path, error: std::io::Error) -> StorageError {
    StorageError::Hard {
        path: path.to_path_buf(),
        reason: format!("failed to inspect storage path: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        HashStorage, HashStorageLoad, ObservedStorageState, StorageError, StoredFileState,
        AFTER_RECOVERY_CLAIM_HOOK, BEFORE_EXACT_OBSERVATION_HOOK, BEFORE_MISSING_PUBLISH_HOOK,
        BEFORE_RECOVERY_CLAIM_HOOK, BEFORE_RECOVERY_ROLLBACK_HOOK, FORCE_RECOVERY_PUBLISH_FAILURE,
        META, META_KEY_GENERATION,
    };
    use redb::Database;
    use std::collections::HashMap;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn missing_file_has_typed_missing_state() {
        let dir = tempdir().expect("tempdir");
        let storage = HashStorage::new(dir.path().join("missing.redb"));

        assert!(matches!(
            storage.load_state(),
            Ok(HashStorageLoad::MissingPath)
        ));
    }

    #[test]
    fn existing_empty_database_has_typed_missing_state() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("empty.redb");
        Database::create(&path).expect("empty database");

        assert!(matches!(
            HashStorage::new(path).load_state(),
            Ok(HashStorageLoad::ExistingUninitialized)
        ));
    }

    #[test]
    fn existing_empty_database_bootstraps_from_exact_observation() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("empty.redb");
        let database = Database::create(&path).expect("empty database");
        drop(database);
        let storage = HashStorage::new(path);
        let observed = storage
            .uninitialized_observation()
            .expect("empty observation");

        storage
            .commit_observed_snapshot(&sample_snapshot("initialized"), 10, &observed)
            .expect("bootstrap existing empty database");

        let state = storage.load_state().expect("load initialized state");
        assert!(matches!(
            state,
            HashStorageLoad::Initialized(snapshot) if snapshot.generation == 1
        ));
    }

    #[test]
    fn empty_observation_rejects_concurrent_healthy_replacement() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("state.redb");
        let database = Database::create(&path).expect("empty database");
        drop(database);
        let storage = HashStorage::new(path.clone());
        assert!(matches!(
            storage.load_state(),
            Ok(HashStorageLoad::ExistingUninitialized)
        ));
        let healthy = dir.path().join("healthy.redb");
        HashStorage::create_replacement(healthy.clone(), &sample_snapshot("healthy"), 9, 9)
            .expect("healthy database");
        let healthy_bytes = fs::read(&healthy).expect("healthy bytes");
        let target = path.clone();
        BEFORE_EXACT_OBSERVATION_HOOK.with(|cell| {
            *cell.borrow_mut() = Some(Box::new(move || {
                fs::remove_file(&target).expect("remove empty database");
                fs::rename(&healthy, &target).expect("publish healthy database");
            }));
        });

        let error = storage
            .observe_state()
            .expect_err("reclassification must reject concurrent repair");

        assert!(matches!(
            error,
            StorageError::ConcurrentStateModified { .. }
        ));
        assert_eq!(
            fs::read(path).expect("healthy database retained"),
            healthy_bytes
        );
    }

    #[test]
    fn metadata_without_snapshot_tables_is_recoverable() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("metadata-only.redb");
        let database = Database::create(&path).expect("database");
        let write = database.begin_write().expect("write");
        {
            let mut meta = write.open_table(META).expect("meta");
            meta.insert(META_KEY_GENERATION, 1).expect("generation");
        }
        write.commit().expect("commit");
        drop(database);

        assert!(matches!(
            HashStorage::new(path).load_state(),
            Err(StorageError::Recoverable { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn filesystem_lookup_errors_are_hard_instead_of_missing() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("loop.redb");
        std::os::unix::fs::symlink(&path, &path).expect("self symlink");

        assert!(matches!(
            HashStorage::new(path).load_state(),
            Err(StorageError::Hard { .. })
        ));
    }

    #[test]
    fn dump_transaction_lookup_errors_are_hard_instead_of_missing() {
        let dir = tempdir().expect("tempdir");
        let parent_file = dir.path().join("not-a-directory");
        fs::write(&parent_file, b"file").expect("parent file");
        let path = parent_file.join("state.redb");

        assert!(matches!(
            HashStorage::new(path).current_dump_transaction_id(),
            Err(StorageError::Hard { .. })
        ));
    }

    #[test]
    fn exact_recovery_claim_preserves_concurrently_repaired_database() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("state.redb");
        fs::write(&path, b"corrupt-a").expect("corrupt A");
        let storage = HashStorage::new(path.clone());
        let observed = storage.recoverable_observation().expect("observation A");
        let healthy = dir.path().join("healthy-b.redb");
        let snapshot = sample_snapshot("healthy");
        HashStorage::create_replacement(healthy.clone(), &snapshot, 9, 9).expect("healthy B");
        let healthy_bytes = fs::read(&healthy).expect("healthy bytes");
        let target = path.clone();
        BEFORE_RECOVERY_CLAIM_HOOK.with(|cell| {
            *cell.borrow_mut() = Some(Box::new(move || {
                fs::remove_file(&target).expect("remove A");
                fs::rename(&healthy, &target).expect("publish B");
            }));
        });

        let error = storage
            .commit_observed_snapshot(&sample_snapshot("new"), 10, &observed)
            .expect_err("concurrent repair must win");

        assert!(matches!(
            error,
            StorageError::ConcurrentStateModified { .. }
        ));
        assert_eq!(fs::read(path).expect("preserved B"), healthy_bytes);
    }

    #[test]
    fn exact_recovery_claim_rejects_in_place_byte_change() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("state.redb");
        fs::write(&path, b"corrupt-a").expect("corrupt A");
        let storage = HashStorage::new(path.clone());
        let observed = storage.recoverable_observation().expect("observation A");
        let target = path.clone();
        BEFORE_RECOVERY_CLAIM_HOOK.with(|cell| {
            *cell.borrow_mut() = Some(Box::new(move || {
                fs::write(&target, b"corrupt-b").expect("mutate A in place");
            }));
        });

        let error = storage
            .commit_observed_snapshot(&sample_snapshot("new"), 10, &observed)
            .expect_err("changed bytes must reject recovery");

        assert!(matches!(
            error,
            StorageError::ConcurrentStateModified { .. }
        ));
        assert_eq!(
            fs::read(path).expect("changed bytes retained"),
            b"corrupt-b"
        );
    }

    #[test]
    fn missing_observation_does_not_overwrite_concurrent_initialization() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("state.redb");
        let storage = HashStorage::new(path.clone());
        let healthy = dir.path().join("healthy-b.redb");
        HashStorage::create_replacement(healthy.clone(), &sample_snapshot("healthy"), 9, 9)
            .expect("healthy B");
        let healthy_bytes = fs::read(&healthy).expect("healthy bytes");
        let target = path.clone();
        BEFORE_MISSING_PUBLISH_HOOK.with(|cell| {
            *cell.borrow_mut() = Some(Box::new(move || {
                fs::rename(&healthy, &target).expect("publish concurrent B");
            }));
        });

        let error = storage
            .commit_observed_snapshot(
                &sample_snapshot("new"),
                10,
                &ObservedStorageState::MissingPath,
            )
            .expect_err("concurrent initialization must win");

        assert!(matches!(
            error,
            StorageError::ConcurrentStateModified { .. }
        ));
        assert_eq!(fs::read(path).expect("preserved B"), healthy_bytes);
    }

    #[test]
    fn recovery_publication_does_not_overwrite_database_published_after_claim() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("state.redb");
        fs::write(&path, b"corrupt-a").expect("corrupt A");
        let storage = HashStorage::new(path.clone());
        let observed = storage.recoverable_observation().expect("observation A");
        let healthy = dir.path().join("healthy-b.redb");
        HashStorage::create_replacement(healthy.clone(), &sample_snapshot("healthy"), 9, 9)
            .expect("healthy B");
        let healthy_bytes = fs::read(&healthy).expect("healthy bytes");
        let target = path.clone();
        AFTER_RECOVERY_CLAIM_HOOK.with(|cell| {
            *cell.borrow_mut() = Some(Box::new(move || {
                fs::hard_link(&healthy, &target).expect("publish B after claim");
            }));
        });

        let error = storage
            .commit_observed_snapshot(&sample_snapshot("new"), 10, &observed)
            .expect_err("concurrent publication must win");

        assert!(matches!(
            error,
            StorageError::ConcurrentStateModified { .. }
        ));
        assert_eq!(fs::read(path).expect("preserved B"), healthy_bytes);
    }

    #[test]
    fn recovery_rollback_does_not_overwrite_database_published_before_restore() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("state.redb");
        fs::write(&path, b"corrupt-a").expect("corrupt A");
        let storage = HashStorage::new(path.clone());
        let observed = storage.recoverable_observation().expect("observation A");
        let healthy = dir.path().join("healthy-b.redb");
        HashStorage::create_replacement(healthy.clone(), &sample_snapshot("healthy"), 9, 9)
            .expect("healthy B");
        let healthy_bytes = fs::read(&healthy).expect("healthy bytes");
        let target = path.clone();
        FORCE_RECOVERY_PUBLISH_FAILURE.with(|forced| forced.set(true));
        BEFORE_RECOVERY_ROLLBACK_HOOK.with(|cell| {
            *cell.borrow_mut() = Some(Box::new(move || {
                fs::hard_link(&healthy, &target).expect("publish B before rollback");
            }));
        });

        let error = storage
            .commit_observed_snapshot(&sample_snapshot("new"), 10, &observed)
            .expect_err("rollback must preserve concurrent publication");

        assert!(matches!(
            error,
            StorageError::ConcurrentStateModified { .. }
        ));
        assert_eq!(fs::read(path).expect("preserved B"), healthy_bytes);
    }

    fn sample_snapshot(hash: &str) -> HashMap<String, StoredFileState> {
        HashMap::from([(
            "Configuration.xml".to_owned(),
            StoredFileState {
                mtime_ns: 1,
                hash: hash.to_owned(),
            },
        )])
    }
}
