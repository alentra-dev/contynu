use std::fs;
use std::path::{Path, PathBuf};

use crate::ids::{CheckpointId, ProjectId, SessionId};

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
        fs::create_dir_all(self.journal_root())?;
        fs::create_dir_all(self.sqlite_root())?;
        fs::create_dir_all(self.blobs_root())?;
        fs::create_dir_all(self.checkpoints_root())?;
        Ok(())
    }

    pub fn journal_root(&self) -> PathBuf {
        self.root.join("journal")
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

    pub fn journal_path_for_session(&self, session_id: &SessionId) -> PathBuf {
        self.journal_root().join(format!("{session_id}.jsonl"))
    }

    pub fn journal_path_for_project(&self, project_id: &ProjectId) -> PathBuf {
        self.journal_path_for_session(project_id)
    }

    pub fn checkpoint_dir(&self, session_id: &SessionId, checkpoint_id: &CheckpointId) -> PathBuf {
        self.checkpoints_root()
            .join(session_id.as_str())
            .join(checkpoint_id.as_str())
    }

    pub fn project_checkpoint_dir(
        &self,
        project_id: &ProjectId,
        checkpoint_id: &CheckpointId,
    ) -> PathBuf {
        self.checkpoint_dir(project_id, checkpoint_id)
    }

    pub fn project_runtime_dir(&self, project_id: &ProjectId) -> PathBuf {
        self.runtime_root().join(project_id.as_str())
    }
}
