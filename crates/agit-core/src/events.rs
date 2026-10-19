//! Append-only event log.
//!
//! Every state change (commit, ref move, lease, workspace, merge) is appended
//! as one JSON line. Agents can tail it (`agit events --follow`) to react to
//! each other's work instead of polling `status` in a loop.

use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{io_at, Result};
use crate::lock::Lock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// Monotonic sequence number within this repository.
    pub seq: u64,
    pub time_ms: i64,
    /// e.g. `commit`, `ref.update`, `lease.claim`, `workspace.create`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    pub data: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct EventLog {
    path: PathBuf,
    lock_path: PathBuf,
    /// (last seq we know, file length when we learned it). Reused while the
    /// file length is unchanged, so the tail is scanned only when another
    /// process appended.
    cache: std::sync::Arc<std::sync::Mutex<Option<(u64, u64)>>>,
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl EventLog {
    pub fn open(agit_dir: &Path) -> Self {
        EventLog {
            path: agit_dir.join("events.jsonl"),
            lock_path: agit_dir.join("locks").join("events.lock"),
            cache: Default::default(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn last_seq(&self) -> Result<u64> {
        // Events are small; scanning the tail is cheap enough for now.
        // (A sidecar counter is an easy optimization once logs get long.)
        let file = match std::fs::File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => {
                return Err(crate::error::Error::IoAt {
                    path: self.path.clone(),
                    source: e,
                })
            }
        };
        let len = file.metadata()?.len();
        let mut reader = BufReader::new(file);
        let start = len.saturating_sub(64 * 1024);
        reader.seek(SeekFrom::Start(start))?;
        let mut last = 0u64;
        let mut line = String::new();
        let mut first = start != 0;
        while reader.read_line(&mut line)? > 0 {
            if first {
                first = false; // possibly partial line
            } else if let Ok(ev) = serde_json::from_str::<Event>(line.trim_end()) {
                last = ev.seq;
            }
            line.clear();
        }
        Ok(last)
    }

    pub fn append(
        &self,
        kind: &str,
        workspace: Option<&str>,
        actor: Option<&str>,
        data: serde_json::Value,
    ) -> Result<Event> {
        let _guard = Lock::acquire(&self.lock_path, Duration::from_secs(5))?;
        let len_now = std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        let cached = *self.cache.lock().unwrap();
        let last = match cached {
            Some((seq, len)) if len == len_now => seq,
            _ => self.last_seq()?,
        };
        let seq = last + 1;
        let ev = Event {
            seq,
            time_ms: now_ms(),
            kind: kind.to_string(),
            workspace: workspace.map(str::to_string),
            actor: actor.map(str::to_string),
            data,
        };
        let mut line = serde_json::to_vec(&ev)?;
        line.push(b'\n');
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(io_at(&self.path))?;
        f.write_all(&line).map_err(io_at(&self.path))?;
        *self.cache.lock().unwrap() = Some((seq, len_now + line.len() as u64));
        if crate::durability::sync_data() {
            crate::durability::sync_file(&f).map_err(io_at(&self.path))?;
        }
        Ok(ev)
    }

    /// Read events with `seq > after`. Returns them in order.
    pub fn read_after(&self, after: u64) -> Result<Vec<Event>> {
        let file = match std::fs::File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => {
                return Err(crate::error::Error::IoAt {
                    path: self.path.clone(),
                    source: e,
                })
            }
        };
        let reader = BufReader::new(file);
        let mut out = Vec::new();
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(ev) = serde_json::from_str::<Event>(&line) {
                if ev.seq > after {
                    out.push(ev);
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_and_read() {
        let dir = tempfile::tempdir().unwrap();
        let log = EventLog::open(dir.path());
        log.append("a", None, Some("x"), serde_json::json!({"n": 1}))
            .unwrap();
        log.append("b", Some("ws"), None, serde_json::json!({"n": 2}))
            .unwrap();
        let all = log.read_after(0).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].seq, 1);
        assert_eq!(all[1].seq, 2);
        assert_eq!(all[1].workspace.as_deref(), Some("ws"));
        assert_eq!(log.read_after(1).unwrap().len(), 1);
    }
}
