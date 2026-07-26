use std::collections::{BTreeMap, BTreeSet};

use thiserror::Error;

use crate::domain::sync_receipt::{SyncReceipt, SyncReceiptError, SyncTarget};
use crate::use_cases::dump_execution::ShadowWriteSet;

/// Raw SHA-256 hash of one managed file version.
pub(crate) type RawFileHash = [u8; 32];

/// One file version observed at a point in the three-way merge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileVersion {
    Absent,
    Present(RawFileHash),
}

/// Publication decision for one managed path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MergeAction {
    /// Source and baseline match, so the dump version must be published.
    Apply,
    /// Source and dump independently reached the same version.
    Converged,
    /// The dump is unchanged, so the local source version must be preserved.
    RetainLocal,
    /// Source and dump diverged from the baseline and from each other.
    Conflict,
    /// All three versions are identical.
    NoOp,
}

/// One deterministic path-level decision in a complete manifest merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManifestMergeEntry {
    path: String,
    baseline: FileVersion,
    source: FileVersion,
    dump: FileVersion,
    action: MergeAction,
}

impl ManifestMergeEntry {
    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    pub(crate) const fn baseline(&self) -> FileVersion {
        self.baseline
    }

    pub(crate) const fn source(&self) -> FileVersion {
        self.source
    }

    pub(crate) const fn dump(&self) -> FileVersion {
        self.dump
    }

    pub(crate) const fn action(&self) -> MergeAction {
        self.action
    }
}

/// Sorted complete merge plan. Callers must reject the whole plan when it has conflicts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManifestMergePlan {
    entries: Vec<ManifestMergeEntry>,
}

impl ManifestMergePlan {
    pub(crate) fn entries(&self) -> &[ManifestMergeEntry] {
        &self.entries
    }

    pub(crate) fn has_conflicts(&self) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.action == MergeAction::Conflict)
    }

    pub(crate) fn applied_receipt(
        &self,
        writes: &ShadowWriteSet,
    ) -> Result<SyncReceipt, MergeReceiptError> {
        if self.has_conflicts() {
            return Err(MergeReceiptError::ConflictedPlan);
        }
        let requested = self.requested_targets()?;
        let processed = writes
            .paths()
            .iter()
            .map(|path| {
                let entry = self
                    .entries
                    .binary_search_by(|entry| entry.path.as_str().cmp(path))
                    .ok()
                    .map(|index| &self.entries[index])
                    .ok_or_else(|| MergeReceiptError::UnknownProcessedPath(path.clone()))?;
                sync_target(entry)?
                    .ok_or_else(|| MergeReceiptError::UnknownProcessedPath(path.clone()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut skipped = Vec::new();
        for entry in &self.entries {
            let Some(target) = sync_target(entry)? else {
                continue;
            };
            match entry.action {
                MergeAction::Apply | MergeAction::Converged => {}
                MergeAction::RetainLocal | MergeAction::NoOp => skipped.push(target),
                MergeAction::Conflict => return Err(MergeReceiptError::ConflictedPlan),
            }
        }
        Ok(SyncReceipt::applied(requested, processed, skipped)?)
    }

    pub(crate) fn failed_receipt(&self) -> Result<SyncReceipt, MergeReceiptError> {
        Ok(SyncReceipt::failed(self.requested_targets()?)?)
    }

    pub(crate) fn conflict_receipt(&self) -> Result<SyncReceipt, MergeReceiptError> {
        if !self.has_conflicts() {
            return Err(MergeReceiptError::ConflictFreePlan);
        }
        let requested = self.requested_targets()?;
        let mut skipped = Vec::new();
        let mut conflicted = Vec::new();
        for entry in &self.entries {
            let Some(target) = sync_target(entry)? else {
                continue;
            };
            match entry.action {
                MergeAction::Conflict => conflicted.push(target),
                MergeAction::Apply
                | MergeAction::Converged
                | MergeAction::RetainLocal
                | MergeAction::NoOp => skipped.push(target),
            }
        }
        Ok(SyncReceipt::conflict(requested, skipped, conflicted)?)
    }

    fn requested_targets(&self) -> Result<Vec<SyncTarget>, MergeReceiptError> {
        let mut targets = Vec::new();
        for entry in self
            .entries
            .iter()
            .filter(|entry| entry.source != entry.dump)
        {
            if let Some(target) = sync_target(entry)? {
                targets.push(target);
            }
        }
        Ok(targets)
    }
}

#[derive(Debug, Error)]
pub(crate) enum MergeReceiptError {
    #[error("cannot create an applied receipt for a conflicted merge plan")]
    ConflictedPlan,
    #[error("cannot create a conflict receipt for a conflict-free merge plan")]
    ConflictFreePlan,
    #[error("platform write evidence contains unknown managed path '{0}'")]
    UnknownProcessedPath(String),
    #[error("invalid merge receipt: {0}")]
    InvalidReceipt(#[from] SyncReceiptError),
}

fn sync_target(entry: &ManifestMergeEntry) -> Result<Option<SyncTarget>, SyncReceiptError> {
    let pre_hash = version_hash(entry.source);
    let post_hash = version_hash(entry.dump);
    if pre_hash.is_none() && post_hash.is_none() {
        return Ok(None);
    }
    SyncTarget::new(&entry.path, pre_hash, post_hash).map(Some)
}

fn version_hash(version: FileVersion) -> Option<String> {
    match version {
        FileVersion::Absent => None,
        FileVersion::Present(hash) => Some(
            hash.iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
        ),
    }
}

/// Plan all paths in the union of the three complete manifests.
#[must_use]
pub(crate) fn plan_manifest_merge(
    baseline: &BTreeMap<String, RawFileHash>,
    source: &BTreeMap<String, RawFileHash>,
    dump: &BTreeMap<String, RawFileHash>,
) -> ManifestMergePlan {
    let paths = baseline
        .keys()
        .chain(source.keys())
        .chain(dump.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let entries = paths
        .into_iter()
        .map(|path| {
            let baseline = file_version(baseline, &path);
            let source = file_version(source, &path);
            let dump = file_version(dump, &path);
            ManifestMergeEntry {
                action: plan_file_merge(baseline, source, dump),
                path,
                baseline,
                source,
                dump,
            }
        })
        .collect();
    ManifestMergePlan { entries }
}

fn file_version(manifest: &BTreeMap<String, RawFileHash>, path: &str) -> FileVersion {
    match manifest.get(path) {
        Some(hash) => FileVersion::Present(*hash),
        None => FileVersion::Absent,
    }
}

/// Plan publication for one managed path from baseline (`B`), source (`S`) and dump (`D`).
#[must_use]
pub(crate) fn plan_file_merge(
    baseline: FileVersion,
    source: FileVersion,
    dump: FileVersion,
) -> MergeAction {
    if baseline == FileVersion::Absent
        && source != FileVersion::Absent
        && dump == FileVersion::Absent
    {
        MergeAction::Conflict
    } else if source == baseline && dump == baseline {
        MergeAction::NoOp
    } else if source == baseline {
        MergeAction::Apply
    } else if dump == baseline {
        MergeAction::RetainLocal
    } else if source == dump {
        MergeAction::Converged
    } else {
        MergeAction::Conflict
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{plan_file_merge, plan_manifest_merge, FileVersion, MergeAction, RawFileHash};
    use crate::use_cases::dump_execution::ShadowWriteSet;

    const BASELINE: [u8; 32] = [0x11; 32];
    const SOURCE: [u8; 32] = [0x22; 32];
    const DUMP: [u8; 32] = [0x33; 32];

    #[test]
    fn plans_every_present_baseline_equality_combination() {
        let cases = [
            (
                "all versions equal",
                present(BASELINE),
                present(BASELINE),
                present(BASELINE),
                MergeAction::NoOp,
            ),
            (
                "dump modifies unchanged source",
                present(BASELINE),
                present(BASELINE),
                present(DUMP),
                MergeAction::Apply,
            ),
            (
                "dump deletes unchanged source",
                present(BASELINE),
                present(BASELINE),
                FileVersion::Absent,
                MergeAction::Apply,
            ),
            (
                "source modifies unchanged dump",
                present(BASELINE),
                present(SOURCE),
                present(BASELINE),
                MergeAction::RetainLocal,
            ),
            (
                "source deletes unchanged dump",
                present(BASELINE),
                FileVersion::Absent,
                present(BASELINE),
                MergeAction::RetainLocal,
            ),
            (
                "source and dump converge on content",
                present(BASELINE),
                present(SOURCE),
                present(SOURCE),
                MergeAction::Converged,
            ),
            (
                "source and dump converge on deletion",
                present(BASELINE),
                FileVersion::Absent,
                FileVersion::Absent,
                MergeAction::Converged,
            ),
            (
                "source and dump modify differently",
                present(BASELINE),
                present(SOURCE),
                present(DUMP),
                MergeAction::Conflict,
            ),
            (
                "local modification conflicts with dump deletion",
                present(BASELINE),
                present(SOURCE),
                FileVersion::Absent,
                MergeAction::Conflict,
            ),
            (
                "local deletion conflicts with dump modification",
                present(BASELINE),
                FileVersion::Absent,
                present(DUMP),
                MergeAction::Conflict,
            ),
        ];

        assert_cases(&cases);
    }

    #[test]
    fn plans_absent_baseline_bootstrap_without_overwriting_local_files() {
        let cases = [
            (
                "file remains absent",
                FileVersion::Absent,
                FileVersion::Absent,
                FileVersion::Absent,
                MergeAction::NoOp,
            ),
            (
                "dump adds a new file",
                FileVersion::Absent,
                FileVersion::Absent,
                present(DUMP),
                MergeAction::Apply,
            ),
            (
                "local-only file conflicts during bootstrap",
                FileVersion::Absent,
                present(SOURCE),
                FileVersion::Absent,
                MergeAction::Conflict,
            ),
            (
                "matching local and dump additions converge",
                FileVersion::Absent,
                present(SOURCE),
                present(SOURCE),
                MergeAction::Converged,
            ),
            (
                "different local and dump additions conflict",
                FileVersion::Absent,
                present(SOURCE),
                present(DUMP),
                MergeAction::Conflict,
            ),
        ];

        assert_cases(&cases);
    }

    #[test]
    fn plans_sorted_union_of_baseline_source_and_dump_manifests() {
        let baseline = manifest(&[
            ("apply.txt", BASELINE),
            ("delete.txt", BASELINE),
            ("gone.txt", BASELINE),
            ("local.txt", BASELINE),
            ("conflict.txt", BASELINE),
        ]);
        let source = manifest(&[
            ("apply.txt", BASELINE),
            ("delete.txt", BASELINE),
            ("local.txt", SOURCE),
            ("conflict.txt", SOURCE),
            ("local-only.txt", SOURCE),
        ]);
        let dump = manifest(&[
            ("apply.txt", DUMP),
            ("local.txt", BASELINE),
            ("conflict.txt", DUMP),
            ("created.txt", DUMP),
        ]);

        let plan = plan_manifest_merge(&baseline, &source, &dump);

        assert_eq!(
            plan.entries()
                .iter()
                .map(|entry| (entry.path(), entry.action()))
                .collect::<Vec<_>>(),
            vec![
                ("apply.txt", MergeAction::Apply),
                ("conflict.txt", MergeAction::Conflict),
                ("created.txt", MergeAction::Apply),
                ("delete.txt", MergeAction::Apply),
                ("gone.txt", MergeAction::Converged),
                ("local-only.txt", MergeAction::Conflict),
                ("local.txt", MergeAction::RetainLocal),
            ]
        );
        assert!(plan.has_conflicts());
    }

    #[test]
    fn builds_exact_applied_failed_and_conflict_receipts() {
        let clean = plan_manifest_merge(
            &manifest(&[
                ("apply.txt", BASELINE),
                ("converged.txt", BASELINE),
                ("local.txt", BASELINE),
                ("noop.txt", BASELINE),
            ]),
            &manifest(&[
                ("apply.txt", BASELINE),
                ("converged.txt", SOURCE),
                ("local.txt", SOURCE),
                ("noop.txt", BASELINE),
            ]),
            &manifest(&[
                ("apply.txt", DUMP),
                ("converged.txt", SOURCE),
                ("local.txt", BASELINE),
                ("noop.txt", BASELINE),
            ]),
        );

        let writes = ShadowWriteSet::from_paths_for_test(&[
            "apply.txt",
            "converged.txt",
            "local.txt",
            "noop.txt",
        ]);
        let applied =
            serde_json::to_value(clean.applied_receipt(&writes).expect("applied receipt"))
                .expect("serialize applied");
        assert_eq!(applied["status"], "applied");
        assert_eq!(
            receipt_paths(&applied, "requested"),
            vec!["apply.txt", "local.txt"]
        );
        assert_eq!(
            receipt_paths(&applied, "processed"),
            vec!["apply.txt", "converged.txt", "local.txt", "noop.txt"]
        );
        assert_eq!(
            receipt_paths(&applied, "skipped"),
            vec!["local.txt", "noop.txt"]
        );

        let failed = serde_json::to_value(clean.failed_receipt().expect("failed receipt"))
            .expect("serialize failed");
        assert_eq!(failed["status"], "failed");
        assert_eq!(
            receipt_paths(&failed, "requested"),
            vec!["apply.txt", "local.txt"]
        );
        assert_eq!(receipt_paths(&failed, "processed"), Vec::<String>::new());

        let conflicted = plan_manifest_merge(
            &manifest(&[("conflict.txt", BASELINE), ("safe.txt", BASELINE)]),
            &manifest(&[("conflict.txt", SOURCE), ("safe.txt", BASELINE)]),
            &manifest(&[("conflict.txt", DUMP), ("safe.txt", DUMP)]),
        );
        let conflict =
            serde_json::to_value(conflicted.conflict_receipt().expect("conflict receipt"))
                .expect("serialize conflict");
        assert_eq!(conflict["status"], "conflict");
        assert_eq!(
            receipt_paths(&conflict, "requested"),
            vec!["conflict.txt", "safe.txt"]
        );
        assert_eq!(receipt_paths(&conflict, "processed"), Vec::<String>::new());
        assert_eq!(receipt_paths(&conflict, "skipped"), vec!["safe.txt"]);
        assert_eq!(receipt_paths(&conflict, "conflicted"), vec!["conflict.txt"]);
    }

    fn present(hash: [u8; 32]) -> FileVersion {
        FileVersion::Present(hash)
    }

    fn assert_cases(cases: &[(&str, FileVersion, FileVersion, FileVersion, MergeAction)]) {
        for (name, baseline, source, dump, expected) in cases {
            assert_eq!(
                plan_file_merge(*baseline, *source, *dump),
                *expected,
                "case: {name}"
            );
        }
    }

    fn manifest(entries: &[(&str, RawFileHash)]) -> BTreeMap<String, RawFileHash> {
        entries
            .iter()
            .map(|(path, hash)| ((*path).to_owned(), *hash))
            .collect()
    }

    fn receipt_paths(value: &serde_json::Value, list: &str) -> Vec<String> {
        value[list]
            .as_array()
            .expect("receipt list")
            .iter()
            .map(|target| target["path"].as_str().expect("path").to_owned())
            .collect()
    }
}
