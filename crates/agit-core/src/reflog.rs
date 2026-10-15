//! Reference logs: an append-only undo trail for every ref and HEAD move.
//!
//! One line per update: `<old> <new> <actor> <time_ms>\t<message>`.
//! Logs live under `logs/<refname>` next to the refs (and `logs/HEAD` in the
//! workspace directory). `agit reflog` reads them, `agit reset` uses them to
//! undo, and `agit gc` keeps every object they reference until they expire.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{io_at, Result};
use crate::events::now_ms;
use crate::hash::ObjectId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReflogEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old: Option<ObjectId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new: Option<ObjectId>,
    pub actor: String,
    pub time_ms: i64,
    pub message: String,
}

pub fn actor() -> String {
    std::env::var("AGIT_AGENT")
        .ok()
        .or_else(|| std::env::var("AGIT_AUTHOR_NAME").ok())
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "unknown".into())
}

fn fmt_id(id: Option<ObjectId>) -> String {
    id.map(|i| i.to_hex()).unwrap_or_else(|| "0".repeat(64))
}

fn parse_id(s: &str) -> Option<ObjectId> {
    let id: ObjectId = s.parse().ok()?;
    if id.is_zero() {
        None
    } else {
        Some(id)
    }
}

pub fn log_path(base_dir: &Path, name: &str) -> PathBuf {
    base_dir.join("logs").join(name)
}

pub fn append(
    base_dir: &Path,
    name: &str,
    old: Option<ObjectId>,
    new: Option<ObjectId>,
    message: &str,
) -> Result<()> {
    let path = log_path(base_dir, name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io_at(parent))?;
    }
    let line = format!(
        "{} {} {} {}\t{}\n",
        fmt_id(old),
        fmt_id(new),
        actor().replace([' ', '\t', '\n'], "_"),
        now_ms(),
        message.replace(['\n', '\r'], " ")
    );
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(io_at(&path))?;
    f.write_all(line.as_bytes()).map_err(io_at(&path))?;
    Ok(())
}

/// Entries oldest first (empty when the log does not exist).
pub fn read(base_dir: &Path, name: &str) -> Result<Vec<ReflogEntry>> {
    let path = log_path(base_dir, name);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(crate::error::Error::IoAt { path, source: e }),
    };
    Ok(text.lines().filter_map(parse_line).collect())
}

fn parse_line(line: &str) -> Option<ReflogEntry> {
    let (head, message) = line.split_once('\t').unwrap_or((line, ""));
    let mut parts = head.split(' ');
    let old = parse_id(parts.next()?);
    let new = parse_id(parts.next()?);
    let actor = parts.next()?.to_string();
    let time_ms = parts.next()?.parse().ok()?;
    Some(ReflogEntry {
        old,
        new,
        actor,
        time_ms,
        message: message.to_string(),
    })
}

/// Drop entries older than `cutoff_ms`. Returns how many were removed.
pub fn expire(base_dir: &Path, name: &str, cutoff_ms: i64) -> Result<usize> {
    let entries = read(base_dir, name)?;
    let keep: Vec<&ReflogEntry> = entries.iter().filter(|e| e.time_ms >= cutoff_ms).collect();
    let removed = entries.len() - keep.len();
    if removed > 0 {
        let mut out = String::new();
        for e in keep {
            out.push_str(&format!(
                "{} {} {} {}\t{}\n",
                fmt_id(e.old),
                fmt_id(e.new),
                e.actor,
                e.time_ms,
                e.message
            ));
        }
        crate::refs::write_atomic(&log_path(base_dir, name), out.as_bytes())?;
    }
    Ok(removed)
}

/// All log names under `base_dir/logs` (relative, `/`-separated).
pub fn list_logs(base_dir: &Path) -> Result<Vec<String>> {
    let root = base_dir.join("logs");
    let mut out = Vec::new();
    if !root.exists() {
        return Ok(out);
    }
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).map_err(io_at(&dir))?.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(rel) = p.strip_prefix(&root) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out.sort();
    Ok(out)
}
