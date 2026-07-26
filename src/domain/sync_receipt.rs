use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};
use thiserror::Error;

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum SyncReceiptError {
    #[error("sync target path must be a normalized relative path: '{0}'")]
    InvalidPath(String),
    #[error("sync target hash must be 64 lowercase hexadecimal characters: '{0}'")]
    InvalidHash(String),
    #[error("sync target '{0}' must contain a preHash or postHash")]
    MissingHashes(String),
    #[error("sync receipt list '{list}' contains contradictory entries for '{path}'")]
    ContradictoryDuplicate { list: &'static str, path: String },
    #[error("sync receipt outcome lists overlap at '{0}'")]
    OverlappingOutcome(String),
    #[error("sync receipt contains inconsistent hashes for '{0}'")]
    InconsistentTarget(String),
    #[error("sync receipt violates terminal status invariants for '{0:?}'")]
    InvalidTerminalState(SyncStatus),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SyncStatus {
    Applied,
    Skipped,
    Failed,
    Conflict,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase", try_from = "SyncTargetDto")]
pub struct SyncTarget {
    path: String,
    pre_hash: Option<String>,
    post_hash: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SyncTargetDto {
    path: String,
    pre_hash: Option<String>,
    post_hash: Option<String>,
}

impl SyncTarget {
    pub fn new(
        path: impl AsRef<str>,
        pre_hash: Option<String>,
        post_hash: Option<String>,
    ) -> Result<Self, SyncReceiptError> {
        let path = path.as_ref();
        let windows_absolute = path
            .as_bytes()
            .get(1)
            .is_some_and(|separator| *separator == b':');
        let normalized_segments = !path.is_empty()
            && !path.starts_with('/')
            && !path.contains('\\')
            && !windows_absolute
            && path
                .split('/')
                .all(|segment| !segment.is_empty() && segment != "." && segment != "..");
        let normal_components = Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
        if !normalized_segments || !normal_components {
            return Err(SyncReceiptError::InvalidPath(path.to_owned()));
        }
        if pre_hash.is_none() && post_hash.is_none() {
            return Err(SyncReceiptError::MissingHashes(path.to_owned()));
        }
        for hash in pre_hash.iter().chain(post_hash.iter()) {
            if hash.len() != 64
                || !hash
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(SyncReceiptError::InvalidHash(hash.clone()));
            }
        }
        Ok(Self {
            path: path.to_owned(),
            pre_hash,
            post_hash,
        })
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn pre_hash(&self) -> Option<&str> {
        self.pre_hash.as_deref()
    }

    pub fn post_hash(&self) -> Option<&str> {
        self.post_hash.as_deref()
    }
}

impl TryFrom<SyncTargetDto> for SyncTarget {
    type Error = SyncReceiptError;

    fn try_from(value: SyncTargetDto) -> Result<Self, Self::Error> {
        Self::new(value.path, value.pre_hash, value.post_hash)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", try_from = "SyncReceiptDto")]
pub struct SyncReceipt {
    status: SyncStatus,
    requested: Vec<SyncTarget>,
    processed: Vec<SyncTarget>,
    skipped: Vec<SyncTarget>,
    conflicted: Vec<SyncTarget>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SyncReceiptDto {
    status: SyncStatus,
    requested: Vec<SyncTarget>,
    processed: Vec<SyncTarget>,
    skipped: Vec<SyncTarget>,
    conflicted: Vec<SyncTarget>,
}

impl SyncReceipt {
    pub fn applied(
        requested: Vec<SyncTarget>,
        processed: Vec<SyncTarget>,
        skipped: Vec<SyncTarget>,
    ) -> Result<Self, SyncReceiptError> {
        Self::validated(SyncStatus::Applied, requested, processed, skipped, vec![])
    }

    pub fn skipped(
        requested: Vec<SyncTarget>,
        skipped: Vec<SyncTarget>,
    ) -> Result<Self, SyncReceiptError> {
        Self::validated(SyncStatus::Skipped, requested, vec![], skipped, vec![])
    }

    pub fn failed(requested: Vec<SyncTarget>) -> Result<Self, SyncReceiptError> {
        Self::validated(SyncStatus::Failed, requested, vec![], vec![], vec![])
    }

    pub fn conflict(
        requested: Vec<SyncTarget>,
        skipped: Vec<SyncTarget>,
        conflicted: Vec<SyncTarget>,
    ) -> Result<Self, SyncReceiptError> {
        Self::validated(SyncStatus::Conflict, requested, vec![], skipped, conflicted)
    }

    pub fn empty_skipped() -> Self {
        Self::empty(SyncStatus::Skipped)
    }

    pub fn empty_applied() -> Self {
        Self::empty(SyncStatus::Applied)
    }

    pub fn empty_failed() -> Self {
        Self::empty(SyncStatus::Failed)
    }

    fn empty(status: SyncStatus) -> Self {
        Self {
            status,
            requested: vec![],
            processed: vec![],
            skipped: vec![],
            conflicted: vec![],
        }
    }

    fn validated(
        status: SyncStatus,
        requested: Vec<SyncTarget>,
        processed: Vec<SyncTarget>,
        skipped: Vec<SyncTarget>,
        conflicted: Vec<SyncTarget>,
    ) -> Result<Self, SyncReceiptError> {
        let requested = unique("requested", requested)?;
        let processed = unique("processed", processed)?;
        let skipped = unique("skipped", skipped)?;
        let conflicted = unique("conflicted", conflicted)?;

        let processed_by_path = processed
            .iter()
            .map(|target| (target.path(), target))
            .collect::<BTreeMap<_, _>>();
        for target in &skipped {
            if processed_by_path
                .get(target.path())
                .is_some_and(|processed| status != SyncStatus::Applied || *processed != target)
            {
                return Err(SyncReceiptError::OverlappingOutcome(target.path.clone()));
            }
        }
        let conflicted_paths = conflicted
            .iter()
            .map(SyncTarget::path)
            .collect::<BTreeSet<_>>();
        for target in processed.iter().chain(skipped.iter()) {
            if conflicted_paths.contains(target.path()) {
                return Err(SyncReceiptError::OverlappingOutcome(target.path.clone()));
            }
        }
        let requested_by_path = requested
            .iter()
            .map(|target| (target.path(), target))
            .collect::<BTreeMap<_, _>>();
        for target in processed
            .iter()
            .chain(skipped.iter())
            .chain(conflicted.iter())
        {
            if requested_by_path
                .get(target.path())
                .is_some_and(|requested| *requested != target)
            {
                return Err(SyncReceiptError::InconsistentTarget(target.path.clone()));
            }
        }

        Ok(Self {
            status,
            requested,
            processed,
            skipped,
            conflicted,
        })
    }
}

fn unique(
    list: &'static str,
    mut targets: Vec<SyncTarget>,
) -> Result<Vec<SyncTarget>, SyncReceiptError> {
    targets.sort_by(|left, right| left.path.cmp(&right.path));
    for pair in targets.windows(2) {
        if pair[0].path == pair[1].path {
            return Err(SyncReceiptError::ContradictoryDuplicate {
                list,
                path: pair[0].path.clone(),
            });
        }
    }
    Ok(targets)
}

impl Default for SyncReceipt {
    fn default() -> Self {
        Self::empty_skipped()
    }
}

impl TryFrom<SyncReceiptDto> for SyncReceipt {
    type Error = SyncReceiptError;

    fn try_from(value: SyncReceiptDto) -> Result<Self, Self::Error> {
        let valid = match value.status {
            SyncStatus::Applied => value.conflicted.is_empty(),
            SyncStatus::Skipped => value.processed.is_empty() && value.conflicted.is_empty(),
            SyncStatus::Failed => {
                value.processed.is_empty()
                    && value.skipped.is_empty()
                    && value.conflicted.is_empty()
            }
            SyncStatus::Conflict => value.processed.is_empty(),
        };
        if !valid {
            return Err(SyncReceiptError::InvalidTerminalState(value.status));
        }
        match value.status {
            SyncStatus::Applied => Self::applied(value.requested, value.processed, value.skipped),
            SyncStatus::Skipped => Self::skipped(value.requested, value.skipped),
            SyncStatus::Failed => Self::failed(value.requested),
            SyncStatus::Conflict => {
                Self::conflict(value.requested, value.skipped, value.conflicted)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SyncReceipt, SyncTarget};

    const PRE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const POST: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn target(path: &str) -> SyncTarget {
        SyncTarget::new(path, Some(PRE.to_owned()), Some(POST.to_owned())).expect("target")
    }

    #[test]
    fn rejects_invalid_paths_hashes_and_empty_delta() {
        for path in [
            "",
            "/absolute.xml",
            "C:/absolute.xml",
            "a/../b.xml",
            "a//b",
            "a\\b",
        ] {
            assert!(
                SyncTarget::new(path, Some(PRE.to_owned()), None).is_err(),
                "accepted {path}"
            );
        }
        assert!(SyncTarget::new("a.xml", None, None).is_err());
        for hash in [
            "pre",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "gggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggg",
        ] {
            assert!(SyncTarget::new("a.xml", Some(hash.to_owned()), None).is_err());
        }
    }

    #[test]
    fn constructors_sort_and_reject_duplicate_or_incoherent_membership() {
        let receipt = SyncReceipt::applied(
            vec![target("z.xml"), target("a.xml")],
            vec![target("z.xml")],
            vec![],
        )
        .expect("receipt");
        let value = serde_json::to_value(receipt).expect("serialize");
        assert_eq!(value["requested"][0]["path"], "a.xml");
        assert_eq!(value["requested"][1]["path"], "z.xml");

        assert!(
            SyncReceipt::applied(vec![target("a.xml"), target("a.xml")], vec![], vec![]).is_err()
        );
        let changed = SyncTarget::new("a.xml", Some(PRE.to_owned()), None).expect("changed");
        assert!(SyncReceipt::applied(vec![target("a.xml")], vec![changed], vec![]).is_err());
        let orthogonal = SyncReceipt::applied(
            vec![target("a.xml")],
            vec![target("a.xml")],
            vec![target("a.xml")],
        )
        .expect("processed and retained-local evidence may overlap");
        let json = serde_json::to_value(&orthogonal).expect("serialize overlap");
        assert_eq!(json["processed"][0], json["skipped"][0]);
        assert_eq!(
            serde_json::from_value::<SyncReceipt>(json).expect("round trip overlap"),
            orthogonal
        );

        let changed = SyncTarget::new("a.xml", Some(PRE.to_owned()), None).expect("changed");
        assert!(SyncReceipt::applied(vec![], vec![target("a.xml")], vec![changed]).is_err());
    }

    #[test]
    fn conflict_round_trip_preserves_nonempty_skipped_and_conflicted() {
        let receipt = SyncReceipt::conflict(
            vec![target("skipped.xml"), target("conflict.xml")],
            vec![target("skipped.xml")],
            vec![target("conflict.xml")],
        )
        .expect("receipt");
        let json = serde_json::to_value(&receipt).expect("serialize");
        assert_eq!(json["processed"], serde_json::json!([]));
        assert_eq!(json["skipped"][0]["path"], "skipped.xml");
        assert_eq!(json["conflicted"][0]["path"], "conflict.xml");
        assert_eq!(
            serde_json::from_value::<SyncReceipt>(json).expect("round trip"),
            receipt
        );
    }

    #[test]
    fn json_is_camel_case_and_rejects_invalid_terminal_state() {
        let receipt = SyncReceipt::applied(vec![], vec![target("a.xml")], vec![]).expect("receipt");
        let value = serde_json::to_value(receipt).expect("serialize");
        assert_eq!(value["processed"][0]["preHash"], PRE);
        assert_eq!(value["processed"][0]["postHash"], POST);

        let invalid = serde_json::json!({
            "status": "failed",
            "requested": [],
            "processed": [{"path":"a.xml","preHash":null,"postHash":POST}],
            "skipped": [],
            "conflicted": []
        });
        assert!(serde_json::from_value::<SyncReceipt>(invalid).is_err());

        let duplicate = serde_json::json!({
            "status": "failed",
            "requested": [
                {"path":"a.xml","preHash":null,"postHash":POST},
                {"path":"a.xml","preHash":PRE,"postHash":POST}
            ],
            "processed": [],
            "skipped": [],
            "conflicted": []
        });
        assert!(serde_json::from_value::<SyncReceipt>(duplicate).is_err());
    }
}
