use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};
use similar::TextDiff;
use walkdir::WalkDir;

use crate::error::Result;
use crate::event::sha256_hex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSnapshot {
    pub relative_path: String,
    pub absolute_path: PathBuf,
    pub sha256: String,
    pub size_bytes: u64,
    pub is_text: bool,
    pub text: Option<String>,
    pub role: FileRole,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeKind {
    Added,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileRole {
    Source,
    Generated,
    Artifact,
}

impl FileRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Generated => "generated",
            Self::Artifact => "artifact",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    pub kind: FileChangeKind,
    pub path: String,
    pub role: FileRole,
    pub before_sha256: Option<String>,
    pub after_sha256: Option<String>,
    pub diff: Option<String>,
    pub snapshot: Option<FileSnapshot>,
}

#[derive(Debug, Clone)]
pub struct FileTracker {
    root: PathBuf,
    ignores: GlobSet,
    small_text_limit: usize,
}

impl FileTracker {
    pub fn new(root: impl Into<PathBuf>, ignore_patterns: &[String]) -> Result<Self> {
        let root = root.into();
        let mut builder = GlobSetBuilder::new();
        for pattern in default_ignore_patterns()
            .into_iter()
            .chain(ignore_patterns.iter().cloned())
        {
            builder.add(
                Glob::new(&pattern)
                    .map_err(|error| crate::error::ContynuError::Validation(error.to_string()))?,
            );
        }
        let ignores = builder
            .build()
            .map_err(|error| crate::error::ContynuError::Validation(error.to_string()))?;
        Ok(Self {
            root,
            ignores,
            small_text_limit: 128 * 1024,
        })
    }

    pub fn snapshot(&self) -> Result<BTreeMap<String, FileSnapshot>> {
        let mut snapshots = BTreeMap::new();
        for entry in WalkDir::new(&self.root)
            .into_iter()
            .filter_map(std::result::Result::ok)
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let relative = path
                .strip_prefix(&self.root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");
            if self.ignores.is_match(relative.as_str()) {
                continue;
            }
            let bytes = fs::read(path)?;
            let is_text = std::str::from_utf8(&bytes).is_ok();
            let text = if is_text && bytes.len() <= self.small_text_limit {
                Some(String::from_utf8_lossy(&bytes).into_owned())
            } else {
                None
            };
            snapshots.insert(
                relative.clone(),
                FileSnapshot {
                    relative_path: relative.clone(),
                    absolute_path: path.to_path_buf(),
                    sha256: format!("sha256:{}", sha256_hex(&bytes)),
                    size_bytes: bytes.len() as u64,
                    is_text,
                    text,
                    role: classify_file_role(&path, &relative, is_text),
                },
            );
        }
        Ok(snapshots)
    }

    pub fn diff(
        &self,
        before: &BTreeMap<String, FileSnapshot>,
        after: &BTreeMap<String, FileSnapshot>,
    ) -> Vec<FileChange> {
        let paths = before
            .keys()
            .chain(after.keys())
            .cloned()
            .collect::<BTreeSet<_>>();

        let mut changes = Vec::new();
        for path in paths {
            match (before.get(&path), after.get(&path)) {
                (None, Some(snapshot)) => changes.push(FileChange {
                    kind: FileChangeKind::Added,
                    path,
                    role: snapshot.role,
                    before_sha256: None,
                    after_sha256: Some(snapshot.sha256.clone()),
                    diff: None,
                    snapshot: Some(snapshot.clone()),
                }),
                (Some(snapshot), None) => changes.push(FileChange {
                    kind: FileChangeKind::Deleted,
                    path,
                    role: snapshot.role,
                    before_sha256: Some(snapshot.sha256.clone()),
                    after_sha256: None,
                    diff: None,
                    snapshot: None,
                }),
                (Some(before), Some(after)) if before.sha256 != after.sha256 => {
                    let diff = match (&before.text, &after.text) {
                        (Some(before_text), Some(after_text)) => {
                            let diff = TextDiff::from_lines(before_text, after_text)
                                .unified_diff()
                                .context_radius(3)
                                .header(&before.relative_path, &after.relative_path)
                                .to_string();
                            Some(diff)
                        }
                        _ => None,
                    };
                    changes.push(FileChange {
                        kind: FileChangeKind::Modified,
                        path,
                        role: after.role,
                        before_sha256: Some(before.sha256.clone()),
                        after_sha256: Some(after.sha256.clone()),
                        diff,
                        snapshot: Some(after.clone()),
                    });
                }
                _ => {}
            }
        }
        changes
    }
}

fn default_ignore_patterns() -> Vec<String> {
    vec![
        ".git/**".into(),
        ".contynu/**".into(),
        "target/**".into(),
        "node_modules/**".into(),
    ]
}

fn classify_file_role(path: &std::path::Path, relative_path: &str, is_text: bool) -> FileRole {
    let lower = relative_path.to_ascii_lowercase();
    if lower.starts_with("dist/")
        || lower.starts_with("build/")
        || lower.starts_with("coverage/")
        || lower.starts_with("out/")
        || lower.starts_with(".next/")
    {
        return FileRole::Generated;
    }

    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());
    match extension.as_deref() {
        Some(
            "rs" | "toml" | "md" | "txt" | "json" | "yaml" | "yml" | "ts" | "tsx" | "js" | "jsx"
            | "py" | "go" | "java" | "c" | "cc" | "cpp" | "h" | "hpp" | "sh" | "sql" | "html"
            | "css" | "scss" | "proto",
        ) => FileRole::Source,
        Some(
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "pdf" | "zip" | "tar" | "gz" | "wasm" | "ico"
            | "mp4" | "mp3" | "mov" | "bin",
        ) => FileRole::Artifact,
        _ if is_text => FileRole::Generated,
        _ => FileRole::Artifact,
    }
}
