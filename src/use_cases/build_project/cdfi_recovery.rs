use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use tempfile::Builder;
use uuid::Uuid;

use crate::domain::build::{CdfiRecoveryAction, CdfiRecoverySummary};
use crate::support::error::AppError;
use crate::support::fs::{replace_file_atomically, ReplaceFileOutcome};

const CDFI_FILE_NAME: &str = "ConfigDumpInfo.xml";

#[cfg(test)]
thread_local! {
    static TEST_CLEANUP_FAILURE: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(super) struct TestCleanupFailureGuard;

#[cfg(test)]
impl Drop for TestCleanupFailureGuard {
    fn drop(&mut self) {
        TEST_CLEANUP_FAILURE.with(|failure| {
            failure.borrow_mut().take();
        });
    }
}

#[cfg(test)]
pub(super) fn simulate_cleanup_failure(message: &str) -> TestCleanupFailureGuard {
    TEST_CLEANUP_FAILURE.with(|failure| {
        *failure.borrow_mut() = Some(message.to_owned());
    });
    TestCleanupFailureGuard
}

#[derive(Debug)]
pub(super) struct CdfiRecoveryGuard {
    tracked_path: PathBuf,
    snapshot_path: PathBuf,
    snapshot_dir: Option<PathBuf>,
    original_exists: bool,
    original_permissions: Option<fs::Permissions>,
}

impl CdfiRecoveryGuard {
    pub(super) fn capture(source_root: &Path, work_path: &Path) -> Result<Self, AppError> {
        let tracked_path = source_root.join(CDFI_FILE_NAME);
        fs::create_dir_all(work_path).map_err(|error| {
            AppError::Runtime(format!(
                "failed to create CDFI recovery work directory '{}': {error}",
                work_path.display()
            ))
        })?;
        let snapshot_dir = Builder::new()
            .prefix("cdfi-recovery-")
            .tempdir_in(work_path)
            .map_err(|error| {
                AppError::Runtime(format!(
                    "failed to create CDFI recovery snapshot under '{}': {error}",
                    work_path.display()
                ))
            })?;
        let snapshot_path = snapshot_dir.path().join(CDFI_FILE_NAME);

        let original_permissions = match fs::metadata(&tracked_path) {
            Ok(metadata) => {
                let bytes = fs::read(&tracked_path).map_err(|error| {
                    AppError::Runtime(format!(
                        "failed to capture CDFI '{}': {error}",
                        tracked_path.display()
                    ))
                })?;
                fs::write(&snapshot_path, bytes).map_err(|error| {
                    AppError::Runtime(format!(
                        "failed to write CDFI recovery snapshot '{}': {error}",
                        snapshot_path.display()
                    ))
                })?;
                Some(metadata.permissions())
            }
            Err(error) if error.kind() == ErrorKind::NotFound => None,
            Err(error) => {
                return Err(AppError::Runtime(format!(
                    "failed to capture CDFI '{}': {error}",
                    tracked_path.display()
                )));
            }
        };
        let original_exists = original_permissions.is_some();
        let snapshot_dir = snapshot_dir.keep();

        Ok(Self {
            tracked_path,
            snapshot_path,
            snapshot_dir: Some(snapshot_dir),
            original_exists,
            original_permissions,
        })
    }

    pub(super) fn restore(&mut self) -> Result<CdfiRecoverySummary, AppError> {
        self.restore_with(replace_file_atomically)
    }

    fn restore_with<F>(&mut self, replace: F) -> Result<CdfiRecoverySummary, AppError>
    where
        F: FnOnce(&Path, &Path, &str, &str) -> std::io::Result<ReplaceFileOutcome>,
    {
        let changed_entry_count = self.changed_entry_count();
        if self.original_state_is_unchanged() {
            return Ok(CdfiRecoverySummary {
                tracked_path: self.tracked_path.clone(),
                original_existed: self.original_exists,
                changed_entry_count,
                action: CdfiRecoveryAction::NotNeeded,
                snapshot_path: None,
                cleanup_warning: None,
                failure: None,
            });
        }
        let cleanup_warning = if self.original_exists {
            self.restore_snapshot_with(replace)?.cleanup_warning
        } else {
            self.remove_created_file()?;
            None
        };
        let action = if self.original_exists {
            CdfiRecoveryAction::Restored
        } else {
            CdfiRecoveryAction::RemovedCreatedFile
        };

        Ok(CdfiRecoverySummary {
            tracked_path: self.tracked_path.clone(),
            original_existed: self.original_exists,
            changed_entry_count,
            action,
            snapshot_path: None,
            cleanup_warning,
            failure: None,
        })
    }

    pub(super) fn failed_summary(
        &self,
        error: &AppError,
        changed_entry_count: Option<usize>,
    ) -> CdfiRecoverySummary {
        CdfiRecoverySummary {
            tracked_path: self.tracked_path.clone(),
            original_existed: self.original_exists,
            changed_entry_count,
            action: CdfiRecoveryAction::Failed,
            snapshot_path: self
                .original_exists
                .then(|| self.snapshot_path.clone())
                .filter(|path| path.exists()),
            cleanup_warning: None,
            failure: Some(error.to_string()),
        }
    }

    pub(super) fn finalize_successful_restore(
        &mut self,
        mut summary: CdfiRecoverySummary,
    ) -> CdfiRecoverySummary {
        if let Err(error) = self.cleanup() {
            summary.snapshot_path = self
                .snapshot_path
                .exists()
                .then(|| self.snapshot_path.clone());
            append_warning(
                &mut summary.cleanup_warning,
                format!("failed to remove CDFI recovery snapshot after restoration: {error}"),
            );
        }
        summary
    }

    pub(super) fn finalize_successful_build(&mut self) -> CdfiRecoverySummary {
        let mut summary = CdfiRecoverySummary {
            tracked_path: self.tracked_path.clone(),
            original_existed: self.original_exists,
            changed_entry_count: self.changed_entry_count(),
            action: CdfiRecoveryAction::NotNeeded,
            snapshot_path: None,
            cleanup_warning: None,
            failure: None,
        };
        if let Err(error) = self.cleanup() {
            summary.snapshot_path = self
                .snapshot_path
                .exists()
                .then(|| self.snapshot_path.clone());
            summary.cleanup_warning = Some(format!(
                "failed to remove CDFI recovery snapshot after successful Designer build: {error}"
            ));
        }
        summary
    }

    pub(super) fn cleanup(&mut self) -> Result<(), AppError> {
        #[cfg(test)]
        if let Some(message) = TEST_CLEANUP_FAILURE.with(|failure| failure.borrow_mut().take()) {
            return Err(AppError::Runtime(message));
        }
        let Some(snapshot_dir) = self.snapshot_dir.as_ref() else {
            return Ok(());
        };
        fs::remove_dir_all(snapshot_dir).map_err(|error| {
            AppError::Runtime(format!(
                "failed to remove CDFI recovery directory '{}': {error}",
                snapshot_dir.display()
            ))
        })?;
        self.snapshot_dir = None;
        Ok(())
    }

    pub(super) fn changed_entry_count(&self) -> Option<usize> {
        let original = if self.original_exists {
            Some(fs::read(&self.snapshot_path).ok()?)
        } else {
            None
        };
        let current = match fs::read(&self.tracked_path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == ErrorKind::NotFound => None,
            Err(_) => return None,
        };
        changed_cdfi_entry_count(original.as_deref(), current.as_deref())
    }

    fn original_state_is_unchanged(&self) -> bool {
        if self.original_exists {
            match (fs::read(&self.snapshot_path), fs::read(&self.tracked_path)) {
                (Ok(original), Ok(current)) => original == current,
                (Ok(_), Err(_)) | (Err(_), Ok(_)) | (Err(_), Err(_)) => false,
            }
        } else {
            matches!(
                fs::metadata(&self.tracked_path),
                Err(error) if error.kind() == ErrorKind::NotFound
            )
        }
    }

    fn restore_snapshot_with<F>(&self, replace: F) -> Result<ReplaceFileOutcome, AppError>
    where
        F: FnOnce(&Path, &Path, &str, &str) -> std::io::Result<ReplaceFileOutcome>,
    {
        let bytes = fs::read(&self.snapshot_path).map_err(|error| {
            AppError::Runtime(format!(
                "failed to read CDFI recovery snapshot '{}': {error}",
                self.snapshot_path.display()
            ))
        })?;
        let parent = self.tracked_path.parent().ok_or_else(|| {
            AppError::Runtime(format!(
                "CDFI path has no parent: '{}'",
                self.tracked_path.display()
            ))
        })?;
        fs::create_dir_all(parent).map_err(|error| {
            AppError::Runtime(format!(
                "failed to create CDFI directory '{}': {error}",
                parent.display()
            ))
        })?;
        let mut staging_file = Builder::new()
            .prefix(".ConfigDumpInfo.xml.restore-")
            .tempfile_in(parent)
            .map_err(|error| {
                AppError::Runtime(format!(
                    "failed to create CDFI restore staging file in '{}': {error}",
                    parent.display()
                ))
            })?;
        staging_file.write_all(&bytes).map_err(|error| {
            AppError::Runtime(format!(
                "failed to write CDFI restore staging file for '{}': {error}",
                self.tracked_path.display()
            ))
        })?;
        if let Some(permissions) = self.original_permissions.as_ref() {
            staging_file
                .as_file()
                .set_permissions(permissions.clone())
                .map_err(|error| {
                    AppError::Runtime(format!(
                        "failed to preserve CDFI permissions for '{}': {error}",
                        self.tracked_path.display()
                    ))
                })?;
        }
        staging_file.as_file().sync_all().map_err(|error| {
            AppError::Runtime(format!(
                "failed to write CDFI restore staging file for '{}': {error}",
                self.tracked_path.display()
            ))
        })?;
        replace(
            staging_file.path(),
            &self.tracked_path,
            &Uuid::new_v4().to_string(),
            "cdfi-recovery",
        )
        .map_err(|error| {
            AppError::Runtime(format!(
                "failed to restore CDFI '{}': {error}",
                self.tracked_path.display()
            ))
        })
    }

    fn remove_created_file(&self) -> Result<(), AppError> {
        match fs::remove_file(&self.tracked_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(AppError::Runtime(format!(
                "failed to remove CDFI created during build '{}': {error}",
                self.tracked_path.display()
            ))),
        }
    }
}

fn append_warning(target: &mut Option<String>, warning: String) {
    match target {
        Some(existing) => {
            existing.push_str("; ");
            existing.push_str(&warning);
        }
        None => *target = Some(warning),
    }
}

type CdfiEntrySignature = Vec<(String, String)>;

fn changed_cdfi_entry_count(original: Option<&[u8]>, current: Option<&[u8]>) -> Option<usize> {
    if original == current {
        return Some(0);
    }

    let original_entries = match original {
        Some(bytes) => parse_cdfi_entries(bytes)?,
        None => BTreeMap::new(),
    };
    let current_entries = match current {
        Some(bytes) => parse_cdfi_entries(bytes)?,
        None => BTreeMap::new(),
    };
    if original_entries.is_empty() && current_entries.is_empty() {
        return Some(1);
    }

    let names = original_entries
        .keys()
        .chain(current_entries.keys())
        .collect::<BTreeSet<_>>();
    Some(
        names
            .into_iter()
            .filter(|name| original_entries.get(*name) != current_entries.get(*name))
            .count(),
    )
}

fn parse_cdfi_entries(bytes: &[u8]) -> Option<BTreeMap<String, CdfiEntrySignature>> {
    let mut reader = Reader::from_reader(bytes);
    let mut entries = BTreeMap::new();

    loop {
        match reader.read_event().ok()? {
            Event::Start(element) | Event::Empty(element)
                if element.local_name().as_ref() == b"Metadata" =>
            {
                let (name, signature) = parse_metadata_signature(&reader, &element)?;
                if entries.insert(name, signature).is_some() {
                    return None;
                }
            }
            Event::Eof => return Some(entries),
            Event::Start(_)
            | Event::End(_)
            | Event::Empty(_)
            | Event::Text(_)
            | Event::CData(_)
            | Event::Comment(_)
            | Event::Decl(_)
            | Event::PI(_)
            | Event::DocType(_) => {}
        }
    }
}

fn parse_metadata_signature(
    reader: &Reader<&[u8]>,
    element: &BytesStart<'_>,
) -> Option<(String, CdfiEntrySignature)> {
    let mut signature = element
        .attributes()
        .map(|attribute| {
            let attribute = attribute.ok()?;
            let key = std::str::from_utf8(attribute.key.as_ref()).ok()?.to_owned();
            let value = attribute
                .decode_and_unescape_value(reader.decoder())
                .ok()?
                .into_owned();
            Some((key, value))
        })
        .collect::<Option<Vec<_>>>()?;
    signature.sort();
    let name = signature
        .iter()
        .find_map(|(key, value)| (key == "name").then(|| value.clone()))
        .filter(|value| !value.is_empty())?;
    Some((name, signature))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{changed_cdfi_entry_count, CdfiRecoveryGuard};
    use crate::domain::build::CdfiRecoveryAction;
    use crate::support::error::AppError;
    use crate::support::fs::ReplaceFileOutcome;

    const CDFI_FILE_NAME: &str = "ConfigDumpInfo.xml";

    #[test]
    fn changed_entry_count_tracks_metadata_add_remove_and_attribute_change() {
        let baseline = br#"<ConfigDumpInfo><ConfigVersions>
            <Metadata name="Catalog.Items" id="item-id" configVersion="v1"/>
            <Metadata name="Document.Order" id="order-id" configVersion="v1"/>
        </ConfigVersions></ConfigDumpInfo>"#;
        let current = br#"<ConfigDumpInfo><ConfigVersions>
            <Metadata name="Catalog.Items" id="item-id" configVersion="v2"/>
            <Metadata name="Report.Sales" id="report-id" configVersion="v1"/>
        </ConfigVersions></ConfigDumpInfo>"#;

        assert_eq!(
            changed_cdfi_entry_count(Some(baseline), Some(current)),
            Some(3)
        );
    }

    #[test]
    fn changed_entry_count_rejects_ambiguous_duplicate_metadata_names() {
        let duplicate = br#"<ConfigDumpInfo><ConfigVersions>
            <Metadata name="Catalog.Items" id="first"/>
            <Metadata name="Catalog.Items" id="second"/>
        </ConfigVersions></ConfigDumpInfo>"#;

        assert_eq!(
            changed_cdfi_entry_count(Some(duplicate), Some(duplicate)),
            Some(0),
            "byte-identical snapshots are known unchanged without parsing"
        );
        assert_eq!(
            changed_cdfi_entry_count(Some(b"<ConfigDumpInfo/>"), Some(duplicate)),
            None
        );
    }

    #[test]
    fn restore_recreates_original_cdfi_bytes_without_rewriting_xml() {
        let temp = tempdir().expect("tempdir");
        let source_root = temp.path().join("source");
        let work_path = temp.path().join("work");
        let tracked_path = source_root.join(CDFI_FILE_NAME);
        let original = b"\xEF\xBB\xBF<?xml version=\"1.0\"?>\r\n<ConfigDumpInfo>\r\n  <Version>1</Version>\r\n</ConfigDumpInfo>\r\n";

        fs::create_dir_all(&source_root).expect("source root");
        fs::write(&tracked_path, original).expect("original CDFI");
        let mut guard = CdfiRecoveryGuard::capture(&source_root, &work_path).expect("capture");
        fs::write(
            &tracked_path,
            b"<ConfigDumpInfo><Version>changed</Version></ConfigDumpInfo>",
        )
        .expect("mutate CDFI");

        guard.restore().expect("restore");

        assert_eq!(fs::read(&tracked_path).expect("restored CDFI"), original);
    }

    #[test]
    fn restore_reports_not_needed_when_present_cdfi_is_unchanged() {
        let temp = tempdir().expect("tempdir");
        let source_root = temp.path().join("source");
        let work_path = temp.path().join("work");
        let tracked_path = source_root.join(CDFI_FILE_NAME);

        fs::create_dir_all(&source_root).expect("source root");
        fs::write(&tracked_path, b"<ConfigDumpInfo/>").expect("original CDFI");
        let mut guard = CdfiRecoveryGuard::capture(&source_root, &work_path).expect("capture");

        let summary = guard.restore().expect("restore");

        assert_eq!(summary.action, CdfiRecoveryAction::NotNeeded);
        assert_eq!(summary.changed_entry_count, Some(0));
    }

    #[test]
    fn restore_uses_raw_bytes_even_when_metadata_entry_count_is_zero() {
        let temp = tempdir().expect("tempdir");
        let source_root = temp.path().join("source");
        let work_path = temp.path().join("work");
        let tracked_path = source_root.join(CDFI_FILE_NAME);
        let original = br#"<ConfigDumpInfo version="1"><ConfigVersions>
            <Metadata name="Catalog.Items" id="item-id" configVersion="v1"/>
        </ConfigVersions></ConfigDumpInfo>"#;
        let changed = br#"<ConfigDumpInfo version="2"><ConfigVersions>
            <Metadata name="Catalog.Items" id="item-id" configVersion="v1"/>
        </ConfigVersions></ConfigDumpInfo>"#;

        fs::create_dir_all(&source_root).expect("source root");
        fs::write(&tracked_path, original).expect("original CDFI");
        let mut guard = CdfiRecoveryGuard::capture(&source_root, &work_path).expect("capture");
        fs::write(&tracked_path, changed).expect("changed CDFI");

        let summary = guard.restore().expect("restore");

        assert_eq!(summary.action, CdfiRecoveryAction::Restored);
        assert_eq!(summary.changed_entry_count, Some(0));
        assert_eq!(fs::read(tracked_path).expect("restored CDFI"), original);
    }

    #[test]
    fn restore_reports_not_needed_when_absent_cdfi_remains_absent() {
        let temp = tempdir().expect("tempdir");
        let source_root = temp.path().join("source");
        let work_path = temp.path().join("work");

        fs::create_dir_all(&source_root).expect("source root");
        let mut guard = CdfiRecoveryGuard::capture(&source_root, &work_path).expect("capture");

        let summary = guard.restore().expect("restore");

        assert_eq!(summary.action, CdfiRecoveryAction::NotNeeded);
        assert_eq!(summary.changed_entry_count, Some(0));
        assert!(!summary.original_existed);
        assert!(summary.snapshot_path.is_none());
    }

    #[test]
    fn restore_removes_cdfi_created_after_absent_capture() {
        let temp = tempdir().expect("tempdir");
        let source_root = temp.path().join("source");
        let work_path = temp.path().join("work");
        let tracked_path = source_root.join(CDFI_FILE_NAME);

        fs::create_dir_all(&source_root).expect("source root");
        let mut guard = CdfiRecoveryGuard::capture(&source_root, &work_path).expect("capture");
        fs::write(&tracked_path, b"<ConfigDumpInfo/>").expect("create CDFI");

        let summary = guard.restore().expect("restore");

        assert!(!tracked_path.exists());
        assert_eq!(summary.tracked_path, tracked_path);
        assert!(!summary.original_existed);
        assert_eq!(summary.changed_entry_count, Some(1));
        assert_eq!(summary.action, CdfiRecoveryAction::RemovedCreatedFile);
        assert!(summary.snapshot_path.is_none());
    }

    #[test]
    fn failed_absent_baseline_recovery_points_to_tracked_file_not_missing_snapshot() {
        let temp = tempdir().expect("tempdir");
        let source_root = temp.path().join("source");
        let work_path = temp.path().join("work");
        let tracked_path = source_root.join(CDFI_FILE_NAME);

        fs::create_dir_all(&source_root).expect("source root");
        let guard = CdfiRecoveryGuard::capture(&source_root, &work_path).expect("capture");
        fs::write(&tracked_path, b"<ConfigDumpInfo/>").expect("created CDFI");

        let summary = guard.failed_summary(
            &AppError::Runtime("failed to remove created CDFI".to_owned()),
            Some(1),
        );

        assert_eq!(summary.action, CdfiRecoveryAction::Failed);
        assert_eq!(summary.tracked_path, tracked_path);
        assert!(!summary.original_existed);
        assert_eq!(summary.changed_entry_count, Some(1));
        assert!(summary.snapshot_path.is_none());
    }

    #[test]
    fn atomic_restore_cleanup_warning_is_preserved_in_summary() {
        let temp = tempdir().expect("tempdir");
        let source_root = temp.path().join("source");
        let work_path = temp.path().join("work");
        let tracked_path = source_root.join(CDFI_FILE_NAME);
        let original = b"<ConfigDumpInfo>original</ConfigDumpInfo>";

        fs::create_dir_all(&source_root).expect("source root");
        fs::write(&tracked_path, original).expect("original CDFI");
        let mut guard = CdfiRecoveryGuard::capture(&source_root, &work_path).expect("capture");
        fs::write(&tracked_path, b"<ConfigDumpInfo>changed</ConfigDumpInfo>")
            .expect("changed CDFI");

        let summary = guard
            .restore_with(|staging_file, target_file, _run_id, _target_identity| {
                fs::write(target_file, fs::read(staging_file)?)?;
                fs::remove_file(staging_file)?;
                Ok(ReplaceFileOutcome {
                    cleanup_warning: Some(
                        "failed to remove backup file '.ConfigDumpInfo.xml.backup-test'".to_owned(),
                    ),
                })
            })
            .expect("restore");

        assert_eq!(fs::read(&tracked_path).expect("restored CDFI"), original);
        assert_eq!(summary.action, CdfiRecoveryAction::Restored);
        assert_eq!(summary.changed_entry_count, Some(1));
        assert!(summary
            .cleanup_warning
            .as_deref()
            .is_some_and(|warning| warning.contains(".ConfigDumpInfo.xml.backup-test")));
    }

    #[test]
    fn cleanup_removes_private_snapshot() {
        let temp = tempdir().expect("tempdir");
        let source_root = temp.path().join("source");
        let work_path = temp.path().join("work");

        fs::create_dir_all(&source_root).expect("source root");
        let mut guard = CdfiRecoveryGuard::capture(&source_root, &work_path).expect("capture");
        let snapshot_path = guard.snapshot_path.clone();

        guard.cleanup().expect("cleanup");

        assert!(!snapshot_path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn failed_restore_keeps_pristine_snapshot_after_guard_is_dropped() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempdir().expect("tempdir");
        let source_root = temp.path().join("source");
        let work_path = temp.path().join("work");
        let tracked_path = source_root.join(CDFI_FILE_NAME);
        let original = b"\xEF\xBB\xBF<ConfigDumpInfo/>\r\n";

        fs::create_dir_all(&source_root).expect("source root");
        fs::write(&tracked_path, original).expect("original CDFI");
        let mut guard = CdfiRecoveryGuard::capture(&source_root, &work_path).expect("capture");
        let snapshot_path = guard.snapshot_path.clone();
        fs::write(&tracked_path, b"<ConfigDumpInfo>changed</ConfigDumpInfo>")
            .expect("changed CDFI");
        fs::set_permissions(&source_root, fs::Permissions::from_mode(0o500))
            .expect("block restore staging");

        guard.restore().expect_err("restore must fail");
        drop(guard);
        fs::set_permissions(&source_root, fs::Permissions::from_mode(0o700))
            .expect("restore source permissions");

        assert_eq!(
            fs::read(snapshot_path).expect("retained snapshot"),
            original
        );
    }

    #[cfg(unix)]
    #[test]
    fn restore_preserves_original_cdfi_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempdir().expect("tempdir");
        let source_root = temp.path().join("source");
        let work_path = temp.path().join("work");
        let tracked_path = source_root.join(CDFI_FILE_NAME);

        fs::create_dir_all(&source_root).expect("source root");
        fs::write(&tracked_path, b"<ConfigDumpInfo/>").expect("original CDFI");
        fs::set_permissions(&tracked_path, fs::Permissions::from_mode(0o640))
            .expect("set permissions");
        let mut guard = CdfiRecoveryGuard::capture(&source_root, &work_path).expect("capture");
        fs::write(
            &tracked_path,
            b"<ConfigDumpInfo><Changed/></ConfigDumpInfo>",
        )
        .expect("mutate CDFI");

        guard.restore().expect("restore");

        assert_eq!(
            fs::metadata(&tracked_path)
                .expect("restored metadata")
                .permissions()
                .mode()
                & 0o777,
            0o640
        );
    }
}
