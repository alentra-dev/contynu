use crate::error::Result;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct BlobStore {
    root: PathBuf,
}

impl BlobStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn put_bytes(&self, bytes: &[u8]) -> Result<String> {
        let sha = Self::sha256(bytes);
        let path = self.path_for_sha(&sha);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        if !path.exists() {
            fs::write(&path, bytes)?;
        }
        Ok(sha)
    }

    pub fn get_bytes(&self, sha: &str) -> Result<Vec<u8>> {
        Ok(fs::read(self.path_for_sha(sha))?)
    }

    pub fn path_for_sha(&self, sha: &str) -> PathBuf {
        let clean = sha.strip_prefix("sha256:").unwrap_or(sha);
        let a = &clean[0..2.min(clean.len())];
        let b = &clean[2..4.min(clean.len())];
        self.root.join("sha256").join(a).join(b).join(clean)
    }

    pub fn sha256(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        format!("sha256:{:x}", hasher.finalize())
    }
}
