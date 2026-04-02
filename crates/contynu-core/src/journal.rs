use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::error::{ContynuError, Result};
use crate::event::EventEnvelope;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalReplay {
    pub event: EventEnvelope,
    pub line_number: usize,
    pub byte_offset: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct JournalAppend {
    pub seq: u64,
    pub byte_offset: u64,
    pub line_number: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct JournalRepair {
    pub repaired: bool,
    pub truncated_at: u64,
}

#[derive(Debug, Clone)]
pub struct Journal {
    path: PathBuf,
    next_seq: Arc<Mutex<u64>>,
    next_line_number: Arc<Mutex<usize>>,
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

        let repair = Self::repair_truncated_tail_at(&path)?;
        let replays = Self::replay_from_path(&path)?;
        let next_seq = replays.last().map(|item| item.event.seq + 1).unwrap_or(1);
        let next_line_number = replays.last().map(|item| item.line_number + 1).unwrap_or(1);
        if repair.repaired {
            let _ = repair;
        }

        Ok(Self {
            path,
            next_seq: Arc::new(Mutex::new(next_seq)),
            next_line_number: Arc::new(Mutex::new(next_line_number)),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append(
        &self,
        draft: crate::event::EventDraft,
    ) -> Result<(EventEnvelope, JournalAppend)> {
        let mut seq_guard = self
            .next_seq
            .lock()
            .map_err(|_| ContynuError::Validation("journal sequence mutex poisoned".into()))?;
        let mut line_guard = self
            .next_line_number
            .lock()
            .map_err(|_| ContynuError::Validation("journal line mutex poisoned".into()))?;

        let event = draft.finalize(*seq_guard)?;
        let line = event.to_json_line()?;

        let mut file = OpenOptions::new().append(true).open(&self.path)?;
        let byte_offset = file.seek(SeekFrom::End(0))?;
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_data()?;

        let append = JournalAppend {
            seq: *seq_guard,
            byte_offset,
            line_number: *line_guard,
        };
        *seq_guard += 1;
        *line_guard += 1;

        Ok((event, append))
    }

    pub fn replay(&self) -> Result<Vec<JournalReplay>> {
        Self::replay_from_path(&self.path)
    }

    pub fn repair_truncated_tail(&self) -> Result<JournalRepair> {
        Self::repair_truncated_tail_at(&self.path)
    }

    pub fn verify(&self) -> Result<()> {
        let _ = self.replay()?;
        Ok(())
    }

    fn replay_from_path(path: &Path) -> Result<Vec<JournalReplay>> {
        let mut file = File::open(path)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;

        let mut out = Vec::new();
        let mut start = 0usize;
        let mut line_number = 1usize;

        while start < bytes.len() {
            let Some(relative_end) = bytes[start..].iter().position(|byte| *byte == b'\n') else {
                return Err(ContynuError::CorruptJournal {
                    line: line_number,
                    reason: "truncated tail without newline terminator".into(),
                });
            };
            let end = start + relative_end;
            let line_bytes = &bytes[start..end];
            let line =
                std::str::from_utf8(line_bytes).map_err(|error| ContynuError::CorruptJournal {
                    line: line_number,
                    reason: error.to_string(),
                })?;

            if !line.trim().is_empty() {
                let event: EventEnvelope =
                    serde_json::from_str(line).map_err(|error| ContynuError::CorruptJournal {
                        line: line_number,
                        reason: error.to_string(),
                    })?;
                event.validate()?;
                out.push(JournalReplay {
                    event,
                    line_number,
                    byte_offset: start as u64,
                });
            }

            line_number += 1;
            start = end + 1;
        }

        Ok(out)
    }

    fn repair_truncated_tail_at(path: &Path) -> Result<JournalRepair> {
        let mut file = OpenOptions::new().read(true).write(true).open(path)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;

        let mut start = 0usize;
        let mut valid_end = 0usize;
        let mut line_number = 1usize;

        while start < bytes.len() {
            let Some(relative_end) = bytes[start..].iter().position(|byte| *byte == b'\n') else {
                file.set_len(valid_end as u64)?;
                return Ok(JournalRepair {
                    repaired: true,
                    truncated_at: valid_end as u64,
                });
            };
            let end = start + relative_end;
            let line_bytes = &bytes[start..end];
            let line =
                std::str::from_utf8(line_bytes).map_err(|error| ContynuError::CorruptJournal {
                    line: line_number,
                    reason: error.to_string(),
                })?;

            if !line.trim().is_empty() {
                let event: EventEnvelope = match serde_json::from_str(line) {
                    Ok(event) => event,
                    Err(_) => {
                        file.set_len(valid_end as u64)?;
                        return Ok(JournalRepair {
                            repaired: true,
                            truncated_at: valid_end as u64,
                        });
                    }
                };
                if event.validate().is_err() {
                    file.set_len(valid_end as u64)?;
                    return Ok(JournalRepair {
                        repaired: true,
                        truncated_at: valid_end as u64,
                    });
                }
            }

            valid_end = end + 1;
            line_number += 1;
            start = end + 1;
        }

        Ok(JournalRepair {
            repaired: false,
            truncated_at: valid_end as u64,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use std::io::Write;

    use serde_json::json;
    use tempfile::tempdir;

    use super::Journal;
    use crate::event::{Actor, EventDraft, EventType};
    use crate::ids::SessionId;

    #[test]
    fn replay_and_tail_repair_work() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let journal = Journal::open(&path).unwrap();
        let session = SessionId::new();

        journal
            .append(EventDraft::new(
                session.clone(),
                None,
                Actor::Runtime,
                EventType::SessionStarted,
                json!({"cwd": "/tmp"}),
            ))
            .unwrap();

        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        write!(file, "{{\"broken\":true").unwrap();
        file.sync_all().unwrap();

        let journal = Journal::open(&path).unwrap();
        let replay = journal.replay().unwrap();
        assert_eq!(replay.len(), 1);
        assert_eq!(replay[0].event.seq, 1);
    }
}
