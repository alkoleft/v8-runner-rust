use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::change_detection::hash_storage::{
    HashStorage, HashStorageLoad, ObservedHashStorage, ObservedStorageState, StorageError,
    StoredFileState,
};
use crate::change_detection::scanner::{self, ScanError};
use crate::domain::source_set::SourceSetContext;

/// A single detected file change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: PathBuf,
    pub rel_path: String,
    pub kind: ChangeKind,
    pub pre_hash: Option<String>,
    pub post_hash: Option<String>,
}

/// How a file changed relative to the stored state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
}

/// File state prepared for the next successful storage commit.
#[derive(Debug, Clone)]
pub struct PreparedFileState {
    pub rel_path: String,
    pub mtime_ns: u64,
    pub hash: String,
}

/// Complete storage update payload produced by one analysis pass.
#[derive(Debug, Clone)]
pub struct PreparedStateUpdate {
    pub snapshot: Vec<PreparedFileState>,
    pub scan_started_at: u64,
    pub observed_storage: ObservedStorageState,
}

/// Deterministic current managed inventory plus its delta against persisted observation.
#[derive(Debug, Clone)]
pub struct ManagedInventory {
    pub requested: Vec<FileChange>,
    pub current: Vec<PreparedFileState>,
    pub prepared: PreparedStateUpdate,
}

/// Result of analyzing one source-set against its persisted snapshot.
#[derive(Debug, Clone)]
pub enum AnalysisOutcome {
    /// No scoped state exists yet; callers must perform a full bootstrap.
    Bootstrap,
    NoChanges,
    Changes {
        changes: Vec<FileChange>,
        prepared: PreparedStateUpdate,
    },
    Fallback,
}

/// Analysis result paired with the source-set context it belongs to.
#[derive(Debug, Clone)]
pub struct ContextAnalysis {
    pub context: SourceSetContext,
    pub outcome: Result<AnalysisOutcome, ChangeDetectionError>,
}

/// Hard failures that prevent normal change-detection flow.
#[derive(Debug, Clone, Error)]
pub enum ChangeDetectionError {
    #[error("invalid managed inventory for source-set '{source_set}': {reason}")]
    InvalidInventory { source_set: String, reason: String },

    #[error("hard storage error for source-set '{source_set}' at '{storage_path}': {reason}")]
    StorageHard {
        source_set: String,
        storage_path: PathBuf,
        reason: String,
    },

    #[error("concurrent state modification for source-set '{source_set}' at '{storage_path}': expected generation {expected}, found {actual:?}")]
    ConcurrentStateModified {
        source_set: String,
        storage_path: PathBuf,
        expected: u64,
        actual: Option<u64>,
    },
}

/// Analyze one source-set context and produce either concrete changes or a safe fallback.
pub fn analyze_context(context: &SourceSetContext) -> ContextAnalysis {
    let storage = HashStorage::new(context.storage_path());
    let snapshot = match storage.load_state() {
        Ok(HashStorageLoad::MissingPath | HashStorageLoad::ExistingUninitialized) => {
            return ContextAnalysis {
                context: context.clone(),
                outcome: Ok(AnalysisOutcome::Bootstrap),
            }
        }
        Ok(HashStorageLoad::Initialized(snapshot)) => snapshot,
        Err(e) => {
            if e.is_recoverable() {
                tracing::warn!(
                    source_set = %context.name(),
                    error = %e,
                    "recoverable storage problem, switching to fallback mode"
                );
                return ContextAnalysis {
                    context: context.clone(),
                    outcome: Ok(AnalysisOutcome::Fallback),
                };
            }
            return ContextAnalysis {
                context: context.clone(),
                outcome: Err(map_storage_hard(context, storage.path(), e)),
            };
        }
    };

    let stored_keys: HashSet<String> = snapshot.entries.keys().cloned().collect();
    let scan = match scanner::scan(
        context.path(),
        snapshot.watermark,
        &stored_keys,
        context.excluded_roots(),
    ) {
        Ok(scan) => scan,
        Err(e) => {
            tracing::warn!(
                source_set = %context.name(),
                error = %e,
                "scan failed, switching to fallback mode"
            );
            return ContextAnalysis {
                context: context.clone(),
                outcome: Ok(AnalysisOutcome::Fallback),
            };
        }
    };

    let seen_rel: HashSet<&str> = scan
        .seen_files
        .iter()
        .map(|f| f.rel_path.as_str())
        .collect();
    let changes = detect_changes(
        context.path(),
        &scan.candidates,
        &snapshot.entries,
        &seen_rel,
    );

    let prepared = build_prepared_state(&scan, &snapshot.entries, snapshot.generation);
    let outcome = if changes.is_empty() {
        AnalysisOutcome::NoChanges
    } else {
        AnalysisOutcome::Changes { changes, prepared }
    };

    ContextAnalysis {
        context: context.clone(),
        outcome: Ok(outcome),
    }
}

/// Analyze multiple source-set contexts using the same work directory.
pub fn analyze_contexts(contexts: &[SourceSetContext]) -> Vec<ContextAnalysis> {
    contexts.iter().map(analyze_context).collect()
}

pub fn managed_inventory(
    context: &SourceSetContext,
) -> Result<ManagedInventory, ChangeDetectionError> {
    let storage = HashStorage::new(context.storage_path());
    let (stored, observed_storage) = match storage
        .observe_state()
        .map_err(|error| map_storage_hard(context, storage.path(), error))?
    {
        ObservedHashStorage::MissingPath => (HashMap::new(), ObservedStorageState::MissingPath),
        ObservedHashStorage::ExistingUninitialized(observation)
        | ObservedHashStorage::Recoverable(observation) => (HashMap::new(), observation),
        ObservedHashStorage::Initialized(snapshot) => {
            let generation = snapshot.generation;
            (
                snapshot.entries,
                ObservedStorageState::Initialized { generation },
            )
        }
    };
    let scan = scanner::scan(
        context.path(),
        None,
        &HashSet::new(),
        context.excluded_roots(),
    )
    .map_err(|error| map_scan_error(context, error))?;
    let seen = scan
        .seen_files
        .iter()
        .map(|file| file.rel_path.as_str())
        .collect::<HashSet<_>>();
    let requested = detect_changes(context.path(), &scan.candidates, &stored, &seen);
    let mut current = scan
        .candidates
        .iter()
        .map(|candidate| PreparedFileState {
            rel_path: candidate.rel_path.clone(),
            mtime_ns: candidate.mtime_ns,
            hash: candidate.hash.clone(),
        })
        .collect::<Vec<_>>();
    current.sort_by(|left, right| left.rel_path.cmp(&right.rel_path));
    let prepared = PreparedStateUpdate {
        snapshot: current.clone(),
        scan_started_at: scan.scan_started_at,
        observed_storage,
    };
    Ok(ManagedInventory {
        requested,
        current,
        prepared,
    })
}

/// Commit exactly the full inventory observed before the platform operation.
pub fn commit_full_observation(
    context: &SourceSetContext,
    prepared: &PreparedStateUpdate,
) -> Result<(), ChangeDetectionError> {
    let storage = HashStorage::new(context.storage_path());
    let snapshot = to_storage_snapshot(&prepared.snapshot);
    storage
        .commit_observed_snapshot(
            &snapshot,
            prepared.scan_started_at,
            &prepared.observed_storage,
        )
        .map_err(|error| map_commit_error(context, storage.path(), error))
}

/// Persist a prepared snapshot after the corresponding build/load step succeeded.
pub fn commit_success(
    context: &SourceSetContext,
    prepared: &PreparedStateUpdate,
) -> Result<(), ChangeDetectionError> {
    let storage = HashStorage::new(context.storage_path());
    let snapshot = to_storage_snapshot(&prepared.snapshot);
    storage
        .commit_snapshot(
            &snapshot,
            prepared.scan_started_at,
            prepared.observed_storage.generation(),
        )
        .map_err(|e| map_commit_error(context, storage.path(), e))
}

/// Re-scan the source-set from scratch and replace the stored snapshot.
#[cfg(test)]
pub fn rescan_and_commit_full(context: &SourceSetContext) -> Result<(), ChangeDetectionError> {
    let storage = HashStorage::new(context.storage_path());
    let current_generation = match storage.current_generation() {
        Ok(generation) => generation,
        Err(e) if e.is_recoverable() => {
            let full = full_snapshot(context, &StorageSnapshotInputs::empty())?;
            let observed = storage
                .recoverable_observation()
                .map_err(|err| map_commit_error(context, storage.path(), err))?;
            return storage
                .commit_observed_snapshot(&full.snapshot, full.scan_started_at, &observed)
                .map_err(|err| map_commit_error(context, storage.path(), err));
        }
        Err(e) => return Err(map_storage_hard(context, storage.path(), e)),
    };

    let full = full_snapshot(
        context,
        &StorageSnapshotInputs {
            watermark: None,
            stored_keys: HashSet::new(),
            observed_generation: current_generation,
        },
    )?;
    storage
        .commit_snapshot(
            &full.snapshot,
            full.scan_started_at,
            full.observed_generation,
        )
        .map_err(|e| map_commit_error(context, storage.path(), e))
}

fn detect_changes(
    source_root: &Path,
    candidates: &[scanner::CandidateFile],
    stored: &HashMap<String, StoredFileState>,
    seen_rel: &HashSet<&str>,
) -> Vec<FileChange> {
    let mut changes = candidates
        .iter()
        .filter_map(|candidate| {
            let (kind, pre_hash) = match stored.get(&candidate.rel_path) {
                None => (ChangeKind::Added, None),
                Some(existing) if existing.hash != candidate.hash => {
                    (ChangeKind::Modified, Some(existing.hash.clone()))
                }
                Some(_) => return None,
            };
            Some(FileChange {
                path: candidate.path.clone(),
                rel_path: candidate.rel_path.clone(),
                kind,
                pre_hash,
                post_hash: Some(candidate.hash.clone()),
            })
        })
        .collect::<Vec<_>>();
    changes.extend(
        stored
            .iter()
            .filter(|(rel_path, _)| !seen_rel.contains(rel_path.as_str()))
            .map(|(rel_path, state)| FileChange {
                path: source_root.join(rel_path),
                rel_path: rel_path.clone(),
                kind: ChangeKind::Deleted,
                pre_hash: Some(state.hash.clone()),
                post_hash: None,
            }),
    );
    changes.sort_by(|left, right| left.rel_path.cmp(&right.rel_path));
    changes
}

fn build_prepared_state(
    scan: &scanner::ScanSnapshot,
    stored: &HashMap<String, StoredFileState>,
    observed_generation: u64,
) -> PreparedStateUpdate {
    let seen_rel: HashSet<&str> = scan
        .seen_files
        .iter()
        .map(|f| f.rel_path.as_str())
        .collect();
    let candidate_map: HashMap<&str, &scanner::CandidateFile> = scan
        .candidates
        .iter()
        .map(|candidate| (candidate.rel_path.as_str(), candidate))
        .collect();

    let mut merged = HashMap::<String, StoredFileState>::new();
    for file in &scan.seen_files {
        let state = if let Some(candidate) = candidate_map.get(file.rel_path.as_str()) {
            StoredFileState {
                mtime_ns: candidate.mtime_ns,
                hash: candidate.hash.clone(),
            }
        } else {
            stored
                .get(&file.rel_path)
                .cloned()
                .unwrap_or_else(|| StoredFileState {
                    mtime_ns: file.mtime_ns,
                    hash: String::new(),
                })
        };
        merged.insert(file.rel_path.clone(), state);
    }

    // Drop deleted entries.
    for rel in stored.keys() {
        if !seen_rel.contains(rel.as_str()) {
            merged.remove(rel);
        }
    }
    // Remove invalid placeholders introduced by missing stored state.
    merged.retain(|_, state| !state.hash.is_empty());

    let mut snapshot = merged
        .into_iter()
        .map(|(rel_path, state)| PreparedFileState {
            rel_path,
            mtime_ns: state.mtime_ns,
            hash: state.hash,
        })
        .collect::<Vec<_>>();
    snapshot.sort_by(|left, right| left.rel_path.cmp(&right.rel_path));
    PreparedStateUpdate {
        snapshot,
        scan_started_at: scan.scan_started_at,
        observed_storage: ObservedStorageState::Initialized {
            generation: observed_generation,
        },
    }
}

#[cfg(test)]
struct StorageSnapshotInputs {
    watermark: Option<u64>,
    stored_keys: HashSet<String>,
    observed_generation: u64,
}

#[cfg(test)]
impl StorageSnapshotInputs {
    fn empty() -> Self {
        Self {
            watermark: None,
            stored_keys: HashSet::new(),
            observed_generation: 0,
        }
    }
}

#[cfg(test)]
struct FullSnapshot {
    snapshot: HashMap<String, StoredFileState>,
    scan_started_at: u64,
    observed_generation: u64,
}

#[cfg(test)]
fn full_snapshot(
    context: &SourceSetContext,
    input: &StorageSnapshotInputs,
) -> Result<FullSnapshot, ChangeDetectionError> {
    let scan = scanner::scan(
        context.path(),
        input.watermark,
        &input.stored_keys,
        context.excluded_roots(),
    )
    .map_err(|e| map_scan_error(context, e))?;
    let mut snapshot = HashMap::new();
    for candidate in scan.candidates {
        snapshot.insert(
            candidate.rel_path,
            StoredFileState {
                mtime_ns: candidate.mtime_ns,
                hash: candidate.hash,
            },
        );
    }
    Ok(FullSnapshot {
        snapshot,
        scan_started_at: scan.scan_started_at,
        observed_generation: input.observed_generation,
    })
}

fn to_storage_snapshot(snapshot: &[PreparedFileState]) -> HashMap<String, StoredFileState> {
    snapshot
        .iter()
        .map(|entry| {
            (
                entry.rel_path.clone(),
                StoredFileState {
                    mtime_ns: entry.mtime_ns,
                    hash: entry.hash.clone(),
                },
            )
        })
        .collect()
}

fn map_storage_hard(
    context: &SourceSetContext,
    storage_path: &Path,
    err: StorageError,
) -> ChangeDetectionError {
    ChangeDetectionError::StorageHard {
        source_set: context.name().to_owned(),
        storage_path: storage_path.to_path_buf(),
        reason: err.to_string(),
    }
}

fn map_commit_error(
    context: &SourceSetContext,
    storage_path: &Path,
    err: StorageError,
) -> ChangeDetectionError {
    match err {
        StorageError::ConcurrentStateModified {
            expected, actual, ..
        } => ChangeDetectionError::ConcurrentStateModified {
            source_set: context.name().to_owned(),
            storage_path: storage_path.to_path_buf(),
            expected,
            actual,
        },
        other => map_storage_hard(context, storage_path, other),
    }
}

fn map_scan_error(context: &SourceSetContext, err: ScanError) -> ChangeDetectionError {
    ChangeDetectionError::StorageHard {
        source_set: context.name().to_owned(),
        storage_path: context.path().to_path_buf(),
        reason: format!("scan failed: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        analyze_context, commit_full_observation, detect_changes, managed_inventory,
        rescan_and_commit_full, AnalysisOutcome, ChangeDetectionError, ChangeKind, FileChange,
    };
    use crate::change_detection::hash_storage::{StoredFileState, META, META_KEY_GENERATION};
    use crate::change_detection::partial_load::decide;
    use crate::change_detection::scanner::CandidateFile;
    use crate::config::model::{BuilderBackend, InfobaseConfig, SourceFormat, SourceSetPurpose};
    use crate::domain::runtime_state::{
        InfobaseIdentity, LogicalSourceRole, RuntimeSourceDescriptor, RuntimeSourceIdentityInputs,
        RuntimeStateLayout,
    };
    use crate::domain::source_set::SourceSetContext;
    use std::collections::{HashMap, HashSet};
    use std::path::Path;
    use tempfile::tempdir;

    fn test_context(state_root: &Path, source_root: &Path) -> SourceSetContext {
        let identity = InfobaseIdentity::normalize(&InfobaseConfig::file(format!(
            "File={}",
            state_root.join("ib").display()
        )))
        .expect("identity");
        let layout = RuntimeStateLayout::new(state_root.join("work"), identity).expect("layout");
        let descriptor = RuntimeSourceDescriptor::new(RuntimeSourceIdentityInputs {
            configured_source_identity: Path::new("src"),
            source_root,
            purpose: SourceSetPurpose::Configuration,
            format: SourceFormat::Designer,
            backend: BuilderBackend::Designer,
            logical_role: LogicalSourceRole::DesignerSource,
        })
        .expect("descriptor");
        SourceSetContext::new(
            "main",
            source_root.to_path_buf(),
            layout.source_state("main", &descriptor),
        )
    }

    #[test]
    fn file_changes_reuse_candidate_and_stored_hashes_for_all_delta_kinds() {
        let root = Path::new("/source");
        let candidates = vec![
            CandidateFile {
                path: root.join("added.xml"),
                rel_path: "added.xml".to_owned(),
                mtime_ns: 1,
                hash: "added-post".to_owned(),
            },
            CandidateFile {
                path: root.join("modified.xml"),
                rel_path: "modified.xml".to_owned(),
                mtime_ns: 2,
                hash: "modified-post".to_owned(),
            },
        ];
        let stored = HashMap::from([
            (
                "modified.xml".to_owned(),
                StoredFileState {
                    mtime_ns: 1,
                    hash: "modified-pre".to_owned(),
                },
            ),
            (
                "deleted.xml".to_owned(),
                StoredFileState {
                    mtime_ns: 1,
                    hash: "deleted-pre".to_owned(),
                },
            ),
        ]);
        let seen = HashSet::from(["added.xml", "modified.xml"]);

        let changes = detect_changes(root, &candidates, &stored, &seen);

        assert_eq!(
            changes
                .iter()
                .map(|change| (
                    change.rel_path.as_str(),
                    change.kind.clone(),
                    change.pre_hash.as_deref(),
                    change.post_hash.as_deref(),
                ))
                .collect::<Vec<_>>(),
            vec![
                ("added.xml", ChangeKind::Added, None, Some("added-post")),
                (
                    "deleted.xml",
                    ChangeKind::Deleted,
                    Some("deleted-pre"),
                    None,
                ),
                (
                    "modified.xml",
                    ChangeKind::Modified,
                    Some("modified-pre"),
                    Some("modified-post"),
                ),
            ]
        );
    }

    #[test]
    fn missing_scoped_storage_requires_bootstrap_even_for_empty_source() {
        let dir = tempdir().expect("tempdir");
        let source_root = dir.path().join("src");
        std::fs::create_dir(&source_root).expect("source");
        let context = test_context(dir.path(), &source_root);

        let analysis = analyze_context(&context);

        assert!(matches!(analysis.outcome, Ok(AnalysisOutcome::Bootstrap)));
        assert!(!context.storage_path().exists());
    }

    #[test]
    fn existing_empty_scoped_database_requires_bootstrap() {
        let dir = tempdir().expect("tempdir");
        let source_root = dir.path().join("src");
        std::fs::create_dir(&source_root).expect("source");
        let context = test_context(dir.path(), &source_root);
        std::fs::create_dir_all(context.storage_path().parent().expect("storage parent"))
            .expect("storage parent");
        redb::Database::create(context.storage_path()).expect("empty database");

        assert!(matches!(
            analyze_context(&context).outcome,
            Ok(AnalysisOutcome::Bootstrap)
        ));
    }

    #[test]
    fn metadata_only_storage_falls_back_and_can_be_recovered() {
        let dir = tempdir().expect("tempdir");
        let source_root = dir.path().join("src");
        std::fs::create_dir(&source_root).expect("source");
        std::fs::write(source_root.join("Configuration.xml"), "<xml />").expect("config");
        let context = test_context(dir.path(), &source_root);
        std::fs::create_dir_all(context.storage_path().parent().expect("storage parent"))
            .expect("storage parent");
        let database = redb::Database::create(context.storage_path()).expect("database");
        let write = database.begin_write().expect("write");
        {
            let mut meta = write.open_table(META).expect("meta");
            meta.insert(META_KEY_GENERATION, 1).expect("generation");
        }
        write.commit().expect("commit");
        drop(database);

        assert!(matches!(
            analyze_context(&context).outcome,
            Ok(AnalysisOutcome::Fallback)
        ));
        rescan_and_commit_full(&context).expect("recover storage");
        assert!(matches!(
            analyze_context(&context).outcome,
            Ok(AnalysisOutcome::NoChanges)
        ));
    }

    #[test]
    fn directory_at_scoped_storage_path_is_a_hard_error_not_bootstrap() {
        let dir = tempdir().expect("tempdir");
        let source_root = dir.path().join("src");
        std::fs::create_dir(&source_root).expect("source");
        let context = test_context(dir.path(), &source_root);
        std::fs::create_dir_all(context.storage_path()).expect("invalid storage directory");

        let analysis = analyze_context(&context);

        assert!(matches!(
            analysis.outcome,
            Err(ChangeDetectionError::StorageHard { .. })
        ));
    }

    #[test]
    fn partial_load_contract_stays_compatible_with_file_change() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("src");
        let object_dir = root.join("Catalogs.Items");
        let module = object_dir.join("ObjectModule.bsl");
        std::fs::create_dir_all(&object_dir).expect("object dir");
        std::fs::write(&module, "module").expect("module");

        let changes = vec![FileChange {
            path: module,
            rel_path: "Catalogs.Items/ObjectModule.bsl".to_owned(),
            kind: ChangeKind::Modified,
            pre_hash: Some("old".to_owned()),
            post_hash: Some("new".to_owned()),
        }];
        let decision = decide(
            &changes,
            &root,
            crate::change_detection::partial_load::DEFAULT_PARTIAL_LOAD_THRESHOLD,
        );
        assert!(matches!(
            decision,
            crate::change_detection::partial_load::LoadDecision::Partial(_)
        ));
    }

    #[test]
    fn hard_storage_errors_stay_hard_during_full_rescan() {
        let dir = tempdir().expect("tempdir");
        let source_root = dir.path().join("src");
        std::fs::create_dir_all(&source_root).expect("source");
        std::fs::write(source_root.join("Configuration.xml"), "<xml />").expect("config");

        let context = test_context(dir.path(), &source_root);
        let storage_path = context.storage_path();
        std::fs::create_dir_all(&storage_path).expect("storage dir");
        let error = rescan_and_commit_full(&context).expect_err("expected hard error");

        assert!(matches!(error, ChangeDetectionError::StorageHard { .. }));
    }

    #[test]
    fn managed_inventory_reports_only_actual_delta_and_keeps_full_current_snapshot() {
        let dir = tempdir().expect("tempdir");
        let source_root = dir.path().join("src");
        std::fs::create_dir_all(&source_root).expect("source");
        std::fs::write(source_root.join("Configuration.xml"), "before").expect("config");
        std::fs::write(source_root.join("Languages.xml"), "stable").expect("language");
        let context = test_context(dir.path(), &source_root);
        rescan_and_commit_full(&context).expect("initialize");

        let unchanged = managed_inventory(&context).expect("unchanged inventory");
        assert!(unchanged.requested.is_empty());
        assert_eq!(unchanged.current.len(), 2);

        std::fs::write(source_root.join("Configuration.xml"), "after").expect("modify");
        let modified = managed_inventory(&context).expect("modified inventory");
        assert_eq!(modified.requested.len(), 1);
        assert_eq!(modified.requested[0].rel_path, "Configuration.xml");
        assert_eq!(modified.requested[0].kind, ChangeKind::Modified);
        assert_eq!(modified.current.len(), 2);
    }

    #[test]
    fn managed_inventory_propagates_hard_storage_errors() {
        let dir = tempdir().expect("tempdir");
        let source_root = dir.path().join("src");
        std::fs::create_dir_all(&source_root).expect("source");
        let context = test_context(dir.path(), &source_root);
        std::fs::create_dir_all(context.storage_path()).expect("invalid storage directory");

        let error = managed_inventory(&context).expect_err("hard error");
        assert!(matches!(error, ChangeDetectionError::StorageHard { .. }));
    }

    #[test]
    fn stale_full_observation_propagates_concurrent_state_modification() {
        let dir = tempdir().expect("tempdir");
        let source_root = dir.path().join("src");
        std::fs::create_dir_all(&source_root).expect("source");
        std::fs::write(source_root.join("Configuration.xml"), "source").expect("config");
        let context = test_context(dir.path(), &source_root);
        let stale = managed_inventory(&context).expect("observation").prepared;
        rescan_and_commit_full(&context).expect("concurrent commit");

        let error = commit_full_observation(&context, &stale).expect_err("stale commit");
        assert!(matches!(
            error,
            ChangeDetectionError::ConcurrentStateModified { .. }
        ));
    }

    #[test]
    fn recoverable_storage_commits_the_prepared_full_observation() {
        let dir = tempdir().expect("tempdir");
        let source_root = dir.path().join("src");
        std::fs::create_dir_all(&source_root).expect("source");
        std::fs::write(source_root.join("Configuration.xml"), "source").expect("config");
        let context = test_context(dir.path(), &source_root);
        std::fs::create_dir_all(context.storage_path().parent().expect("storage parent"))
            .expect("storage parent");
        let database = redb::Database::create(context.storage_path()).expect("database");
        let write = database.begin_write().expect("write");
        {
            let mut meta = write.open_table(META).expect("meta");
            meta.insert(META_KEY_GENERATION, 1).expect("generation");
        }
        write.commit().expect("commit");
        drop(database);

        let observation = managed_inventory(&context).expect("recoverable observation");
        assert_eq!(observation.requested.len(), 1);
        commit_full_observation(&context, &observation.prepared).expect("recover commit");
        assert!(matches!(
            analyze_context(&context).outcome,
            Ok(AnalysisOutcome::NoChanges)
        ));
    }
}
