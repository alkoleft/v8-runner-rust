use std::collections::HashMap;
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::change_detection::file_state::FileState;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("I/O error for storage '{path}': {source}")]
    Io { path: PathBuf, source: std::io::Error },

    #[error("JSON error for storage '{path}': {source}")]
    Json { path: PathBuf, source: serde_json::Error },
}

/// Persisted map of path → FileState, stored as JSON.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct HashStorage {
    /// Keyed by the string representation of the file path.
    entries: HashMap<String, FileState>,
}

impl HashStorage {
    /// Load from a JSON file, or return an empty storage if the file does not exist.
    pub fn load(path: &Path) -> Result<Self, StorageError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let data = std::fs::read(path).map_err(|e| StorageError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        serde_json::from_slice(&data).map_err(|e| StorageError::Json {
            path: path.to_path_buf(),
            source: e,
        })
    }

    /// Persist to a JSON file, creating parent directories as needed.
    pub fn save(&self, path: &Path) -> Result<(), StorageError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| StorageError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }
        let data = serde_json::to_vec_pretty(self).map_err(|e| StorageError::Json {
            path: path.to_path_buf(),
            source: e,
        })?;
        std::fs::write(path, data).map_err(|e| StorageError::Io {
            path: path.to_path_buf(),
            source: e,
        })
    }

    /// Look up the stored state for a file path.
    pub fn get(&self, path: &Path) -> Option<&FileState> {
        self.entries.get(&path.display().to_string())
    }

    /// Insert or update the state for a file path.
    pub fn insert(&mut self, state: FileState) {
        self.entries.insert(state.path.display().to_string(), state);
    }

    /// Remove a file entry (e.g. after deletion).
    pub fn remove(&mut self, path: &Path) {
        self.entries.remove(&path.display().to_string());
    }

    /// Iterate over all stored entries.
    pub fn iter(&self) -> impl Iterator<Item = &FileState> {
        self.entries.values()
    }
}
