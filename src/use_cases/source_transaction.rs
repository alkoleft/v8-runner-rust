use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use tempfile::TempDir;
use thiserror::Error;
use walkdir::WalkDir;

use crate::change_detection::analyzer::PreparedStateUpdate;
use crate::change_detection::scanner::{portable_relative_path, ScanError, SourceInventoryPolicy};
use crate::use_cases::runtime_state::ValidatedCdfi;

#[derive(Debug)]
pub(crate) enum CdfiSeed<'a> {
    None,
    Validated(&'a ValidatedCdfi),
}

#[derive(Debug, Error)]
pub(crate) enum SourceTransactionError {
    #[error("failed to prepare Designer source transaction: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to {operation} '{path}': {source}")]
    FileIo {
        operation: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to inspect Designer source transaction: {0}")]
    Walk(#[from] walkdir::Error),
    #[error("invalid Designer source inventory: {0}")]
    Inventory(#[from] ScanError),
    #[error("staged Designer source differs from the planned source snapshot: {0}")]
    SnapshotMismatch(String),
}

pub(crate) struct DesignerSourceTransaction {
    _transaction: TempDir,
    load_root: PathBuf,
}

impl DesignerSourceTransaction {
    pub(crate) fn create(
        source_root: &Path,
        excluded_roots: &[PathBuf],
        transactions_dir: &Path,
        cdfi_seed: CdfiSeed<'_>,
    ) -> Result<Self, SourceTransactionError> {
        let transactions_existed = transactions_dir.try_exists()?;
        fs::create_dir_all(transactions_dir)?;
        if !transactions_existed {
            if let Some(parent) = transactions_dir.parent() {
                sync_directory(parent)?;
            }
        }
        let transaction = tempfile::Builder::new()
            .prefix("designer-build-")
            .tempdir_in(transactions_dir)?;
        let load_root = transaction.path().join("source");
        fs::create_dir(&load_root)?;
        sync_directory(transactions_dir)?;
        let canonical_source_root = fs::canonicalize(source_root)
            .map_err(|error| file_io("canonicalize source root", source_root, error))?;
        let canonical_excluded_roots = excluded_roots
            .iter()
            .map(|excluded| {
                excluded
                    .strip_prefix(source_root)
                    .map(|relative| canonical_source_root.join(relative))
                    .unwrap_or_else(|_| excluded.clone())
            })
            .collect::<Vec<_>>();
        let policy = SourceInventoryPolicy::new(&canonical_source_root, &canonical_excluded_roots)?;
        for entry in WalkDir::new(&canonical_source_root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| {
                !entry.file_type().is_dir() || policy.should_descend(entry.path())
            })
        {
            let entry = entry?;
            if !entry.file_type().is_file() || !policy.includes_file(entry.path()) {
                continue;
            }
            let relative = portable_relative_path(&canonical_source_root, entry.path())?;
            let target = load_root.join(&relative);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            copy_regular_no_follow(
                &canonical_source_root,
                &canonical_source_root,
                Path::new(&relative),
                entry.path(),
                &target,
            )?;
        }
        if let CdfiSeed::Validated(cdfi) = cdfi_seed {
            fs::write(load_root.join("ConfigDumpInfo.xml"), cdfi.bytes())?;
        }
        Ok(Self {
            _transaction: transaction,
            load_root,
        })
    }

    pub(crate) fn load_root(&self) -> &Path {
        &self.load_root
    }

    #[cfg(test)]
    pub(crate) fn transaction_root(&self) -> &Path {
        self._transaction.path()
    }

    pub(crate) fn verify_snapshot(
        &self,
        prepared: &PreparedStateUpdate,
    ) -> Result<(), SourceTransactionError> {
        if let Some((planned, actual)) = snapshot_mismatch_counts(&self.load_root, &[], prepared)? {
            Err(SourceTransactionError::SnapshotMismatch(format!(
                "planned {} file(s), staged {} file(s)",
                planned, actual
            )))
        } else {
            Ok(())
        }
    }

    pub(crate) fn close(self) -> Result<(), SourceTransactionError> {
        let parent = self._transaction.path().parent().map(Path::to_path_buf);
        self._transaction.close()?;
        if let Some(parent) = parent {
            sync_directory(&parent)?;
        }
        Ok(())
    }
}

pub(crate) fn verify_source_snapshot(
    source_root: &Path,
    excluded_roots: &[PathBuf],
    prepared: &PreparedStateUpdate,
) -> Result<(), SourceTransactionError> {
    if let Some((planned, actual)) =
        snapshot_mismatch_counts(source_root, excluded_roots, prepared)?
    {
        Err(SourceTransactionError::SnapshotMismatch(format!(
            "planned {} file(s), observed {} file(s) in '{}'",
            planned,
            actual,
            source_root.display()
        )))
    } else {
        Ok(())
    }
}

fn snapshot_mismatch_counts(
    source_root: &Path,
    excluded_roots: &[PathBuf],
    prepared: &PreparedStateUpdate,
) -> Result<Option<(usize, usize)>, SourceTransactionError> {
    let scan =
        crate::change_detection::scanner::scan(source_root, None, &HashSet::new(), excluded_roots)?;
    let actual = scan
        .candidates
        .iter()
        .map(|file| (file.rel_path.as_str(), file.hash.as_str()))
        .collect::<HashMap<_, _>>();
    let planned = prepared
        .snapshot
        .iter()
        .map(|file| (file.rel_path.as_str(), file.hash.as_str()))
        .collect::<HashMap<_, _>>();
    Ok((actual != planned).then_some((planned.len(), actual.len())))
}

fn copy_regular_no_follow(
    source_root: &Path,
    _canonical_source_root: &Path,
    relative: &Path,
    source: &Path,
    target: &Path,
) -> Result<(), SourceTransactionError> {
    #[cfg(unix)]
    return copy_regular_beneath_unix(source_root, relative, source, target, || {});
    #[cfg(not(unix))]
    copy_regular_no_follow_portable(_canonical_source_root, source, target, || {})
}

#[cfg(test)]
fn copy_regular_no_follow_with_hook<F>(
    canonical_source_root: &Path,
    source: &Path,
    target: &Path,
    before_open: F,
) -> Result<(), SourceTransactionError>
where
    F: FnOnce(),
{
    #[cfg(unix)]
    {
        let relative = source.strip_prefix(canonical_source_root).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "source '{}' is outside managed root '{}'",
                    source.display(),
                    canonical_source_root.display()
                ),
            )
        })?;
        return copy_regular_beneath_unix(
            canonical_source_root,
            relative,
            source,
            target,
            before_open,
        );
    }
    #[cfg(not(unix))]
    copy_regular_no_follow_portable(canonical_source_root, source, target, before_open)
}

#[cfg(not(unix))]
fn copy_regular_no_follow_portable<F>(
    canonical_source_root: &Path,
    source: &Path,
    target: &Path,
    before_open: F,
) -> Result<(), SourceTransactionError>
where
    F: FnOnce(),
{
    validate_source_containment(canonical_source_root, source)?;
    let before = fs::symlink_metadata(source)
        .map_err(|error| file_io("inspect source file", source, error))?;
    if !before.file_type().is_file()
        || before.file_type().is_symlink()
        || metadata_is_reparse_point(&before)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("source '{}' is not a regular file", source.display()),
        )
        .into());
    }
    #[cfg(windows)]
    let before_handle_identity = windows_path_identity(source)?;
    before_open();
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let mut input = options
        .open(source)
        .map_err(|error| file_io("open source file without following symlinks", source, error))?;
    let opened = input
        .metadata()
        .map_err(|error| file_io("inspect opened source file", source, error))?;
    #[cfg(windows)]
    let windows_identity_changed = windows_file_identity(&input, source)? != before_handle_identity;
    #[cfg(not(windows))]
    let windows_identity_changed = false;
    if !opened.file_type().is_file()
        || opened.file_type().is_symlink()
        || metadata_is_reparse_point(&opened)
        || !same_file_identity(&before, &opened)
        || windows_identity_changed
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("source '{}' changed while staging", source.display()),
        )
        .into());
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|error| file_io("create staged file", target, error))?;
    io::copy(&mut input, &mut output)
        .map_err(|error| file_io("copy source into stage", target, error))?;
    output
        .sync_all()
        .map_err(|error| file_io("sync staged file", target, error))?;
    let after = input
        .metadata()
        .map_err(|error| file_io("re-inspect opened source file", source, error))?;
    validate_source_containment(canonical_source_root, source)?;
    if !same_file_identity(&opened, &after) || metadata_is_reparse_point(&after) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("source '{}' changed during staging", source.display()),
        )
        .into());
    }
    Ok(())
}

#[cfg(unix)]
fn copy_regular_beneath_unix<F>(
    source_root: &Path,
    relative: &Path,
    source: &Path,
    target: &Path,
    before_open: F,
) -> Result<(), SourceTransactionError>
where
    F: FnOnce(),
{
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;

    let mut root_options = OpenOptions::new();
    root_options
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let mut directory = root_options.open(source_root).map_err(|error| {
        file_io(
            "open source root without following symlinks",
            source_root,
            error,
        )
    })?;
    before_open();

    let components = relative.components().collect::<Vec<_>>();
    let Some((file_component, parent_components)) = components.split_last() else {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty source path").into());
    };
    for component in parent_components {
        let std::path::Component::Normal(name) = component else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "source '{}' has a non-normal path component",
                    source.display()
                ),
            )
            .into());
        };
        let name = CString::new(name.as_bytes()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "source path contains a NUL byte",
            )
        })?;
        // SAFETY: `directory` is a live directory descriptor and `name` is a
        // NUL-terminated single component. O_NOFOLLOW rejects a swapped symlink.
        let descriptor = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if descriptor < 0 {
            return Err(file_io(
                "open source directory component without following symlinks",
                source,
                io::Error::last_os_error(),
            ));
        }
        // SAFETY: `openat` returned a new owned descriptor.
        directory = unsafe { fs::File::from_raw_fd(descriptor) };
    }

    let std::path::Component::Normal(file_name) = file_component else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "source '{}' has a non-normal file component",
                source.display()
            ),
        )
        .into());
    };
    let file_name = CString::new(file_name.as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "source path contains a NUL byte",
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
        return Err(file_io(
            "open source file beneath root without following symlinks",
            source,
            io::Error::last_os_error(),
        ));
    }
    // SAFETY: `openat` returned a new owned descriptor.
    let mut input = unsafe { fs::File::from_raw_fd(descriptor) };
    let opened = input
        .metadata()
        .map_err(|error| file_io("inspect opened source file", source, error))?;
    if !opened.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("source '{}' is not a regular file", source.display()),
        )
        .into());
    }

    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|error| file_io("create staged file", target, error))?;
    io::copy(&mut input, &mut output)
        .map_err(|error| file_io("copy source into stage", target, error))?;
    output
        .sync_all()
        .map_err(|error| file_io("sync staged file", target, error))?;
    let after = input
        .metadata()
        .map_err(|error| file_io("re-inspect opened source file", source, error))?;
    if !same_file_identity(&opened, &after) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("source '{}' changed during staging", source.display()),
        )
        .into());
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_source_containment(
    canonical_source_root: &Path,
    source: &Path,
) -> Result<(), SourceTransactionError> {
    let canonical_source = fs::canonicalize(source)
        .map_err(|error| file_io("canonicalize source file", source, error))?;
    if canonical_source.starts_with(canonical_source_root) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "source '{}' resolves outside managed root '{}'",
                source.display(),
                canonical_source_root.display()
            ),
        )
        .into())
    }
}

fn file_io(operation: &'static str, path: &Path, source: io::Error) -> SourceTransactionError {
    SourceTransactionError::FileIo {
        operation,
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(unix)]
fn same_file_identity(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev() && left.ino() == right.ino() && left.len() == right.len()
}

#[cfg(windows)]
fn same_file_identity(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    // Stable identity is checked with GetFileInformationByHandle; metadata still
    // detects a size change during the copy.
    left.len() == right.len()
}

#[cfg(not(any(unix, windows)))]
fn same_file_identity(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.len() == right.len() && left.modified().ok() == right.modified().ok()
}

#[cfg(windows)]
fn metadata_is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WindowsFileIdentity {
    volume_serial: u32,
    file_index: u64,
}

#[cfg(windows)]
fn windows_path_identity(path: &Path) -> Result<WindowsFileIdentity, SourceTransactionError> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|error| file_io("open source identity handle", path, error))?;
    let metadata = file
        .metadata()
        .map_err(|error| file_io("inspect source identity handle", path, error))?;
    if metadata_is_reparse_point(&metadata) || !metadata.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("source '{}' is a Windows reparse point", path.display()),
        )
        .into());
    }
    windows_file_identity(&file, path)
}

#[cfg(windows)]
fn windows_file_identity(
    file: &fs::File,
    path: &Path,
) -> Result<WindowsFileIdentity, SourceTransactionError> {
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };

    let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    // SAFETY: the handle is borrowed from a live File, and Windows initializes
    // BY_HANDLE_FILE_INFORMATION when the call succeeds.
    let succeeded = unsafe {
        GetFileInformationByHandle(file.as_raw_handle().cast(), information.as_mut_ptr())
    };
    if succeeded == 0 {
        return Err(file_io(
            "query Windows source file identity",
            path,
            io::Error::last_os_error(),
        ));
    }
    // SAFETY: guarded by the successful Win32 call above.
    let information = unsafe { information.assume_init() };
    Ok(WindowsFileIdentity {
        volume_serial: information.dwVolumeSerialNumber,
        file_index: (u64::from(information.nFileIndexHigh) << 32)
            | u64::from(information.nFileIndexLow),
    })
}

#[cfg(not(any(unix, windows)))]
fn metadata_is_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), SourceTransactionError> {
    fs::File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), SourceTransactionError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{copy_regular_no_follow_with_hook, CdfiSeed, DesignerSourceTransaction};
    use crate::change_detection::analyzer::{PreparedFileState, PreparedStateUpdate};
    use crate::change_detection::hash_storage::ObservedStorageState;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn copies_only_regular_managed_files_and_cleans_stage_on_drop() {
        let dir = tempdir().expect("tempdir");
        let source = dir.path().join("source");
        let work = source.join("nested-work");
        let transactions = dir.path().join("state/transactions");
        fs::create_dir_all(source.join("Catalogs")).expect("source dirs");
        fs::create_dir_all(&work).expect("nested work");
        fs::write(source.join("Configuration.xml"), b"config").expect("config");
        fs::write(source.join("Catalogs/Item.xml"), b"item").expect("item");
        fs::write(source.join("CONFIGDUMPINFO.XML"), b"user-owned").expect("cdfi");
        fs::write(work.join("secret.txt"), b"state").expect("work file");

        let stage_path = {
            let transaction =
                DesignerSourceTransaction::create(&source, &[work], &transactions, CdfiSeed::None)
                    .expect("transaction");
            assert_eq!(
                fs::read(transaction.load_root().join("Configuration.xml")).expect("staged"),
                b"config"
            );
            assert_eq!(
                fs::read(transaction.load_root().join("Catalogs/Item.xml")).expect("staged"),
                b"item"
            );
            assert!(!transaction.load_root().join("CONFIGDUMPINFO.XML").exists());
            assert!(!transaction.load_root().join("nested-work").exists());
            transaction.transaction_root().to_path_buf()
        };

        assert!(!stage_path.exists());
    }

    #[test]
    fn seeds_only_prevalidated_private_cdfi() {
        let dir = tempdir().expect("tempdir");
        let source = dir.path().join("source");
        let private = dir.path().join("private.xml");
        fs::create_dir_all(&source).expect("source");
        fs::write(source.join("Configuration.xml"), b"config").expect("config");
        fs::write(
            &private,
            r#"<ConfigDumpInfo version="2.17"><Metadata id="seed" configVersion="7"/></ConfigDumpInfo>"#,
        )
        .expect("private CDFI");
        let validated = match crate::use_cases::runtime_state::inspect_private_cdfi(&private)
            .expect("inspect")
        {
            crate::use_cases::runtime_state::PrivateCdfiState::Valid(cdfi) => cdfi,
            state => panic!("expected valid CDFI, got {state:?}"),
        };

        let transaction = DesignerSourceTransaction::create(
            &source,
            &[],
            &dir.path().join("transactions"),
            CdfiSeed::Validated(&validated),
        )
        .expect("transaction");
        assert_eq!(
            fs::read(transaction.load_root().join("ConfigDumpInfo.xml")).expect("seed"),
            fs::read(private).expect("private")
        );
    }

    #[test]
    fn rejects_stage_that_differs_from_planned_snapshot() {
        let dir = tempdir().expect("tempdir");
        let source = dir.path().join("source");
        fs::create_dir_all(&source).expect("source");
        fs::write(source.join("Configuration.xml"), b"actual").expect("source file");
        let transaction = DesignerSourceTransaction::create(
            &source,
            &[],
            &dir.path().join("transactions"),
            CdfiSeed::None,
        )
        .expect("transaction");
        let planned = PreparedStateUpdate {
            snapshot: vec![PreparedFileState {
                rel_path: "Configuration.xml".to_owned(),
                mtime_ns: 0,
                hash: "0".repeat(64),
            }],
            scan_started_at: 0,
            observed_storage: ObservedStorageState::MissingPath,
        };

        assert!(transaction.verify_snapshot(&planned).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn does_not_follow_file_or_directory_symlinks() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().expect("tempdir");
        let source = dir.path().join("source");
        let outside = dir.path().join("outside");
        fs::create_dir_all(&source).expect("source");
        fs::create_dir_all(&outside).expect("outside");
        fs::write(outside.join("file.xml"), b"outside").expect("outside file");
        symlink(outside.join("file.xml"), source.join("file-link.xml")).expect("file symlink");
        symlink(&outside, source.join("dir-link")).expect("dir symlink");

        let transaction = DesignerSourceTransaction::create(
            &source,
            &[],
            &dir.path().join("transactions"),
            CdfiSeed::None,
        )
        .expect("transaction");
        assert!(!transaction.load_root().join("file-link.xml").exists());
        assert!(!transaction.load_root().join("dir-link").exists());
    }

    #[cfg(unix)]
    #[test]
    fn accepts_symlinked_source_root_without_following_nested_symlinks() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().expect("tempdir");
        let physical_source = dir.path().join("physical-source");
        let source_link = dir.path().join("source-link");
        let outside = dir.path().join("outside.xml");
        let excluded = physical_source.join("excluded");
        fs::create_dir_all(&physical_source).expect("physical source");
        fs::create_dir_all(&excluded).expect("excluded source");
        fs::write(physical_source.join("Configuration.xml"), b"config").expect("source file");
        fs::write(excluded.join("secret.xml"), b"secret").expect("excluded file");
        fs::write(&outside, b"outside").expect("outside file");
        symlink(&outside, physical_source.join("outside-link.xml")).expect("nested symlink");
        symlink(&physical_source, &source_link).expect("source root symlink");

        let transaction = DesignerSourceTransaction::create(
            &source_link,
            &[source_link.join("excluded")],
            &dir.path().join("transactions"),
            CdfiSeed::None,
        )
        .expect("symlinked source root");

        assert_eq!(
            fs::read(transaction.load_root().join("Configuration.xml")).expect("staged source"),
            b"config"
        );
        assert!(!transaction.load_root().join("excluded").exists());
        assert!(!transaction.load_root().join("outside-link.xml").exists());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_regular_file_replaced_by_symlink_before_open() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().expect("tempdir");
        let source = dir.path().join("source.xml");
        let outside = dir.path().join("outside.xml");
        let target = dir.path().join("target.xml");
        fs::write(&source, b"inside").expect("source");
        fs::write(&outside, b"outside-secret").expect("outside");

        let canonical_root = fs::canonicalize(dir.path()).expect("canonical root");
        let error = copy_regular_no_follow_with_hook(&canonical_root, &source, &target, || {
            fs::remove_file(&source).expect("remove source");
            symlink(&outside, &source).expect("replace by symlink");
        })
        .expect_err("symlink swap must fail");

        assert!(error.to_string().contains("source"));
        assert!(!target.exists());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_intermediate_directory_replaced_by_symlink() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("root");
        let nested = root.join("nested");
        let moved = root.join("nested-old");
        let attack_link = root.join("attack-link");
        let source = nested.join("source.xml");
        let target = dir.path().join("target.xml");
        fs::create_dir_all(&nested).expect("nested");
        fs::write(&source, b"inside").expect("inside");
        let canonical_root = fs::canonicalize(&root).expect("canonical root");

        let result = copy_regular_no_follow_with_hook(&canonical_root, &source, &target, || {
            fs::rename(&nested, &moved).expect("move nested");
            symlink(&moved, &nested).expect("replace intermediate directory");
        });
        fs::rename(&nested, &attack_link).expect("move attack symlink aside");
        if moved.try_exists().expect("inspect moved directory") {
            fs::rename(&moved, &nested).expect("restore nested directory");
        } else {
            fs::rename(&attack_link, &nested).expect("restore moved directory");
        }

        result.expect_err("intermediate symlink swap to the same file must fail");

        assert!(!target.exists());
    }

    #[cfg(windows)]
    #[test]
    fn rejects_regular_file_replaced_by_windows_symlink_before_open() {
        use std::os::windows::fs::symlink_file;

        let dir = tempdir().expect("tempdir");
        let source = dir.path().join("source.xml");
        let outside = dir.path().join("outside.xml");
        let target = dir.path().join("target.xml");
        fs::write(&source, b"inside").expect("source");
        fs::write(&outside, b"outside-secret").expect("outside");
        let probe = dir.path().join("probe-link.xml");
        if symlink_file(&outside, &probe).is_err() {
            // Windows requires Developer Mode or SeCreateSymbolicLinkPrivilege.
            return;
        }
        fs::remove_file(&probe).expect("remove probe");
        let canonical_root = fs::canonicalize(dir.path()).expect("canonical root");

        copy_regular_no_follow_with_hook(&canonical_root, &source, &target, || {
            fs::remove_file(&source).expect("remove source");
            symlink_file(&outside, &source).expect("replace by symlink");
        })
        .expect_err("Windows reparse swap must fail");

        assert!(!target.exists());
    }
}
