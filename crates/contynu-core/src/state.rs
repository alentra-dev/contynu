use std::fs;
use std::path::{Path, PathBuf};

use crate::ids::{CheckpointId, ProjectId};

#[derive(Debug, Clone)]
pub struct StatePaths {
    root: PathBuf,
}

impl StatePaths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn ensure_layout(&self) -> std::io::Result<()> {
        fs::create_dir_all(self.sqlite_root())?;
        fs::create_dir_all(self.blobs_root())?;
        fs::create_dir_all(self.checkpoints_root())?;
        Ok(())
    }

    pub fn sqlite_root(&self) -> PathBuf {
        self.root.join("sqlite")
    }

    pub fn sqlite_db(&self) -> PathBuf {
        self.sqlite_root().join("contynu.db")
    }

    pub fn config_path(&self) -> PathBuf {
        self.root.join("config.json")
    }

    pub fn blobs_root(&self) -> PathBuf {
        self.root.join("blobs")
    }

    pub fn runtime_root(&self) -> PathBuf {
        self.root.join("runtime")
    }

    pub fn checkpoints_root(&self) -> PathBuf {
        self.root.join("checkpoints")
    }

    pub fn checkpoint_dir(&self, project_id: &ProjectId, checkpoint_id: &CheckpointId) -> PathBuf {
        self.checkpoints_root()
            .join(project_id.as_str())
            .join(checkpoint_id.as_str())
    }

    pub fn project_runtime_dir(&self, project_id: &ProjectId) -> PathBuf {
        self.runtime_root().join(project_id.as_str())
    }

    /// Check for and remove old architecture artifacts (journal/, events, turns tables, etc.)
    pub fn cleanup_old_architecture(&self) -> std::io::Result<()> {
        // Remove journal directory if it exists
        let journal_dir = self.root.join("journal");
        if journal_dir.exists() {
            fs::remove_dir_all(&journal_dir)?;
        }

        // Remove runtime directory (old transcript reconciliation files)
        let runtime_dir = self.runtime_root();
        if runtime_dir.exists() {
            fs::remove_dir_all(&runtime_dir)?;
        }

        // Remove imported-sessions.json
        let imported = self.root.join("imported-sessions.json");
        if imported.exists() {
            fs::remove_file(&imported)?;
        }

        Ok(())
    }
}
