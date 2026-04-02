use crate::error::{ContynuError, Result};
use crate::event::EventEnvelope;
use parking_lot::Mutex;
use serde_json::{Map, Value};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone)]
pub struct Journal {
    path: PathBuf,
    next_seq: Arc<Mutex<u64>>,
}

impl Journal {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        if !path.exists() {
            File::create(&path)?;
        }
        let next_seq = Self::discover_next_seq(&path)?;
        Ok(Self {
            path,
            next_seq: Arc::new(Mutex::new(next_seq)),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append(&self, event: &mut EventEnvelope) -> Result<u64> {
        let mut guard = self.next_seq.lock();
        event.seq = *guard;
        event.checksum = None;
        event.checksum = Some(Self::checksum(event)?);
        let line = serde_json::to_string(event)?;

        let mut file = OpenOptions::new().append(true).open(&self.path)?;
        writeln!(file, "{line}")?;
        file.sync_data()?;

        *guard += 1;
        Ok(event.seq)
    }

    pub fn replay(&self) -> Result<Vec<EventEnvelope>> {
        let file = File::open(&self.path)?;
        let reader = BufReader::new(file);
        let mut out = Vec::new();
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            out.push(serde_json::from_str::<EventEnvelope>(&line)?);
        }
        Ok(out)
    }

    pub fn repair_truncated_tail(&self) -> Result<()> {
        let file = File::open(&self.path)?;
        let reader = BufReader::new(file);
        let mut valid_len: u64 = 0;
        for line in reader.lines() {
            let line = line?;
            let bytes = line.as_bytes().len() as u64 + 1;
            if line.trim().is_empty() || serde_json::from_str::<EventEnvelope>(&line).is_ok() {
                valid_len += bytes;
            } else {
                break;
            }
        }
        let mut file = OpenOptions::new().write(true).open(&self.path)?;
        file.set_len(valid_len)?;
        file.seek(SeekFrom::Start(valid_len))?;
        Ok(())
    }

    fn discover_next_seq(path: &Path) -> Result<u64> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let mut next = 1;
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(event) = serde_json::from_str::<EventEnvelope>(&line) {
                next = event.seq + 1;
            }
        }
        Ok(next)
    }

    fn checksum(event: &EventEnvelope) -> Result<String> {
        let mut value = serde_json::to_value(event)?;
        let object = value
            .as_object_mut()
            .ok_or_else(|| ContynuError::InvalidState("event did not serialize to object".into()))?;
        object.remove("checksum");
        let canonical = canonicalize_object(object);
        Ok(crate::blobs::BlobStore::sha256(canonical.as_bytes()))
    }
}

fn canonicalize_object(map: &Map<String, Value>) -> String {
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    let mut out = String::from("{");
    for (index, key) in keys.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&serde_json::to_string(key).unwrap());
        out.push(':');
        out.push_str(&canonicalize_value(&map[*key]));
    }
    out.push('}');
    out
}

fn canonicalize_value(value: &Value) -> String {
    match value {
        Value::Object(map) => canonicalize_object(map),
        Value::Array(values) => {
            let inner = values.iter().map(canonicalize_value).collect::<Vec<_>>().join(",");
            format!("[{inner}]")
        }
        _ => serde_json::to_string(value).unwrap(),
    }
}
