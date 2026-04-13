use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{ContynuError, Result};

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobDescriptor {
    pub sha256: String,
    pub size_bytes: u64,
    pub relative_path: String,
}

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

    pub fn put_bytes(&self, bytes: &[u8]) -> Result<BlobDescriptor> {
        fs::create_dir_all(self.root.join("sha256"))?;

        let sha256 = format!("sha256:{}", sha256_hex(bytes));
        let relative_path = self.relative_path_for_sha(&sha256);
        let full_path = self.root.join(&relative_path);

        if !full_path.exists() {
            if let Some(parent) = full_path.parent() {
                fs::create_dir_all(parent)?;
            }
            let tmp_path = full_path.with_extension("tmp");
            let mut file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&tmp_path)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            fs::rename(&tmp_path, &full_path)?;
        }

        Ok(BlobDescriptor {
            sha256,
            size_bytes: bytes.len() as u64,
            relative_path,
        })
    }

    pub fn put_text(&self, text: &str) -> Result<BlobDescriptor> {
        self.put_bytes(text.as_bytes())
    }

    pub fn get_bytes(&self, sha256: &str) -> Result<Vec<u8>> {
        Ok(fs::read(
            self.root.join(self.relative_path_for_sha(sha256)),
        )?)
    }

    pub fn verify(&self, sha256: &str) -> Result<()> {
        let bytes = self.get_bytes(sha256)?;
        let actual = format!("sha256:{}", sha256_hex(&bytes));
        if actual != sha256 {
            return Err(ContynuError::Validation(format!(
                "blob digest mismatch: expected {sha256}, got {actual}"
            )));
        }
        Ok(())
    }

    pub fn relative_path_for_sha(&self, sha256: &str) -> String {
        let digest = sha256.strip_prefix("sha256:").unwrap_or(sha256);
        let a = &digest[0..2.min(digest.len())];
        let b = &digest[2..4.min(digest.len())];
        format!("sha256/{a}/{b}/{digest}")
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::BlobStore;

    #[test]
    fn blob_store_deduplicates_and_verifies() {
        let dir = tempdir().unwrap();
        let store = BlobStore::new(dir.path());

        let first = store.put_text("hello world").unwrap();
        let second = store.put_text("hello world").unwrap();

        assert_eq!(first, second);
        store.verify(&first.sha256).unwrap();
    }
}
