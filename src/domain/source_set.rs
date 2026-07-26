use std::path::{Path, PathBuf};

use crate::domain::runtime_state::{BaselineRole, IbBaseline, RuntimeSourceState, StateGeneration};

/// Runtime context for one logical source-set.
#[derive(Debug, Clone)]
pub struct SourceSetContext {
    /// Logical name (matches `SourceSetConfig.name`).
    name: String,
    /// Absolute root directory of the sources.
    path: PathBuf,
    /// Already resolved, versioned runtime state for this source view.
    runtime_state: RuntimeSourceState,
    excluded_roots: Vec<PathBuf>,
}

impl SourceSetContext {
    pub fn new(name: impl Into<String>, path: PathBuf, runtime_state: RuntimeSourceState) -> Self {
        assert!(
            path.is_absolute(),
            "SourceSetContext.path must be absolute, got: {}",
            path.display()
        );

        Self {
            name: name.into(),
            path,
            runtime_state,
            excluded_roots: Vec::new(),
        }
    }

    pub fn with_excluded_roots(mut self, excluded_roots: Vec<PathBuf>) -> Self {
        self.excluded_roots = excluded_roots;
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn excluded_roots(&self) -> &[PathBuf] {
        &self.excluded_roots
    }

    #[cfg(test)]
    pub fn runtime_state(&self) -> &RuntimeSourceState {
        &self.runtime_state
    }

    /// Absolute path to this context's versioned redb hash storage.
    pub fn storage_path(&self) -> PathBuf {
        self.runtime_state.hash_storage_path()
    }

    /// Private platform-owned `ConfigDumpInfo.xml` for this source view.
    pub(crate) fn private_cdfi_path(&self) -> PathBuf {
        self.runtime_state.private_cdfi_path()
    }

    /// Complete private baseline for a semantic role and state generation.
    pub(crate) fn baseline(&self, role: BaselineRole, generation: StateGeneration) -> IbBaseline {
        self.runtime_state.baseline(role, generation)
    }

    /// Owned transaction directory for this source view.
    pub(crate) fn transactions_dir(&self) -> PathBuf {
        self.runtime_state.transactions_dir()
    }

    /// Per-source lock serializing recovery, staging and runtime-state publication.
    pub(crate) fn state_lock_path(&self) -> PathBuf {
        self.transactions_dir()
            .parent()
            .map(|state_dir| state_dir.join("runtime-state.lock"))
            .unwrap_or_else(|| self.transactions_dir().join("runtime-state.lock"))
    }
}

#[cfg(test)]
mod tests {
    use super::SourceSetContext;
    use crate::config::model::{BuilderBackend, InfobaseConfig, SourceFormat, SourceSetPurpose};
    use crate::domain::runtime_state::{
        InfobaseIdentity, LogicalSourceRole, RuntimeSourceDescriptor, RuntimeSourceIdentityInputs,
        RuntimeStateLayout,
    };
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    fn context(path: PathBuf) -> SourceSetContext {
        let dir = tempdir().expect("tempdir");
        let identity = InfobaseIdentity::normalize(&InfobaseConfig::file(format!(
            "File={}",
            dir.path().join("ib").display()
        )))
        .expect("identity");
        let layout = RuntimeStateLayout::new(dir.path().join("work"), identity).expect("layout");
        let descriptor = RuntimeSourceDescriptor::new(RuntimeSourceIdentityInputs {
            configured_source_identity: Path::new("src-main"),
            source_root: &path,
            purpose: SourceSetPurpose::Configuration,
            format: SourceFormat::Designer,
            backend: BuilderBackend::Designer,
            logical_role: LogicalSourceRole::DesignerSource,
        })
        .expect("descriptor");
        let state = layout.source_state("main", &descriptor);
        SourceSetContext::new("main", path, state)
    }

    #[test]
    fn uses_resolved_versioned_storage_path() {
        let source = std::env::current_dir()
            .expect("current dir")
            .join("target/source-set-context");
        let context = context(source.clone());

        assert_eq!(context.name(), "main");
        assert_eq!(context.path(), source);
        assert_eq!(
            context.storage_path(),
            context
                .runtime_state()
                .state_dir()
                .join("hash-storage.redb")
        );
        assert!(!context
            .storage_path()
            .to_string_lossy()
            .contains("hash-storages"));
    }

    #[test]
    #[should_panic(expected = "must be absolute")]
    fn rejects_relative_path() {
        let absolute = std::env::current_dir()
            .expect("current dir")
            .join("target/source-set-context");
        let valid = context(absolute);
        let _ = SourceSetContext::new(
            "main",
            PathBuf::from("relative/path"),
            valid.runtime_state().clone(),
        );
    }
}
