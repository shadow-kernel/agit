//! References: named pointers to commits.
//!
//! Every update is a compare-and-swap. Agents pass the value they last saw;
//! if another agent moved the ref in the meantime the update fails with
//! `RefConflict` instead of silently clobbering work. This is the primitive
//! that makes optimistic multi-agent coordination possible.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::{io_at, Error, Result};
use crate::hash::ObjectId;
use crate::lock::Lock;

pub const HEADS_PREFIX: &str = "refs/heads/";
pub const TAGS_PREFIX: &str = "refs/tags/";
pub const CHECKPOINTS_PREFIX: &str = "refs/checkpoints/";

/// What to require of the current value during a CAS update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expect {
    /// Do not check the current value (last writer wins).
    Any,
    /// The ref must not exist yet.
    Absent,
    /// The ref must currently point exactly here.
    Is(ObjectId),
}

#[derive(Debug, Clone)]
pub struct RefStore {
    dir: PathBuf,
    lock_path: PathBuf,
}

impl RefStore {
    pub fn open(agit_dir: &Path) -> Self {
        RefStore {
            dir: agit_dir.to_path_buf(),
            lock_path: agit_dir.join("locks").join("refs.lock"),
        }
    }

    pub fn validate_name(name: &str) -> Result<()> {
        let bad = name.is_empty()
            || !name.starts_with("refs/")
            || name.ends_with('/')
            || name.ends_with(".lock")
            || name.contains("//")
            || name.contains("..")
            || name.split('/').any(|c| c.is_empty() || c.starts_with('.'))
            || name.chars().any(|c| {
                c.is_control() || matches!(c, ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\')
            });
        if bad {
            Err(Error::InvalidRefName(name.to_string()))
        } else {
            Ok(())
        }
    }

    pub fn validate_branch_name(name: &str) -> Result<()> {
        Self::validate_name(&format!("{HEADS_PREFIX}{name}"))
    }

    fn path_for(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    pub fn read(&self, name: &str) -> Result<Option<ObjectId>> {
        let path = self.path_for(name);
        match fs::read_to_string(&path) {
            Ok(s) => {
                let s = s.trim();
                if s.is_empty() {
                    return Ok(None);
                }
                Ok(Some(s.parse().map_err(|_| Error::CorruptObject {
                    id: ObjectId::ZERO,
                    reason: format!("ref {name} contains invalid id {s:?}"),
                })?))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(Error::IoAt { path, source: e }),
        }
    }

    pub fn exists(&self, name: &str) -> bool {
        self.path_for(name).is_file()
    }

    fn lock(&self) -> Result<Lock> {
        Lock::acquire(&self.lock_path, Duration::from_secs(10))
    }

    /// Atomically set `name` to `new` (or delete it when `new` is `None`).
    pub fn update(&self, name: &str, expect: Expect, new: Option<ObjectId>) -> Result<()> {
        self.update_with(name, expect, new, "update")
    }

    /// Like [`update`](Self::update) with a reflog message.
    pub fn update_with(
        &self,
        name: &str,
        expect: Expect,
        new: Option<ObjectId>,
        message: &str,
    ) -> Result<()> {
        Self::validate_name(name)?;
        let _guard = self.lock()?;
        let current = self.read(name)?;
        let ok = match expect {
            Expect::Any => true,
            Expect::Absent => current.is_none(),
            Expect::Is(id) => current == Some(id),
        };
        if !ok {
            return Err(Error::RefConflict {
                name: name.to_string(),
                expected: match expect {
                    Expect::Is(id) => Some(id),
                    _ => None,
                },
                actual: current,
            });
        }
        let path = self.path_for(name);
        match new {
            Some(id) => write_atomic(&path, format!("{id}\n").as_bytes())?,
            None => match fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(Error::IoAt { path, source: e }),
            },
        }
        if current != new {
            crate::reflog::append(&self.dir, name, current, new, message)?;
        }
        Ok(())
    }

    /// Reflog of a ref, oldest first.
    pub fn reflog(&self, name: &str) -> Result<Vec<crate::reflog::ReflogEntry>> {
        crate::reflog::read(&self.dir, name)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Apply several CAS updates atomically: either all succeed or none do.
    pub fn update_many(&self, updates: &[(String, Expect, Option<ObjectId>)]) -> Result<()> {
        self.update_many_with(updates, "update")
    }

    pub fn update_many_with(
        &self,
        updates: &[(String, Expect, Option<ObjectId>)],
        message: &str,
    ) -> Result<()> {
        for (name, _, _) in updates {
            Self::validate_name(name)?;
        }
        let _guard = self.lock()?;
        let mut olds = Vec::with_capacity(updates.len());
        // Phase 1: validate all expectations under the lock.
        for (name, expect, _) in updates {
            let current = self.read(name)?;
            olds.push(current);
            let ok = match expect {
                Expect::Any => true,
                Expect::Absent => current.is_none(),
                Expect::Is(id) => current == Some(*id),
            };
            if !ok {
                return Err(Error::RefConflict {
                    name: name.clone(),
                    expected: match expect {
                        Expect::Is(id) => Some(*id),
                        _ => None,
                    },
                    actual: current,
                });
            }
        }
        // Phase 2: apply.
        for ((name, _, new), old) in updates.iter().zip(olds) {
            let path = self.path_for(name);
            match new {
                Some(id) => write_atomic(&path, format!("{id}\n").as_bytes())?,
                None => {
                    if let Err(e) = fs::remove_file(&path) {
                        if e.kind() != std::io::ErrorKind::NotFound {
                            return Err(Error::IoAt { path, source: e });
                        }
                    }
                }
            }
            if old != *new {
                crate::reflog::append(&self.dir, name, old, *new, message)?;
            }
        }
        Ok(())
    }

    /// List all refs under `prefix` (e.g. `refs/heads/`), sorted by name.
    pub fn list(&self, prefix: &str) -> Result<Vec<(String, ObjectId)>> {
        let base = self.path_for(prefix.trim_end_matches('/'));
        let mut out = Vec::new();
        if !base.exists() {
            return Ok(out);
        }
        let mut stack = vec![base.clone()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).map_err(io_at(&dir))?.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    stack.push(p);
                } else if let Ok(rel) = p.strip_prefix(&self.dir) {
                    let name = rel.to_string_lossy().replace('\\', "/");
                    if let Some(id) = self.read(&name)? {
                        out.push((name, id));
                    }
                }
            }
        }
        out.sort();
        Ok(out)
    }
}

/// Write a file atomically (temp + fsync + rename). Used for refs and other
/// small control files that must never be observed half-written.
pub fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    write_atomic_with(path, data, crate::durability::sync_refs())
}

/// Atomic write for bulk/cache data (index, metadata): synced only when the
/// durability level is `All`.
pub fn write_atomic_bulk(path: &Path, data: &[u8]) -> Result<()> {
    write_atomic_with(path, data, crate::durability::sync_data())
}

fn write_atomic_with(path: &Path, data: &[u8], sync: bool) -> Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| Error::Invalid("path has no parent".into()))?;
    fs::create_dir_all(dir).map_err(io_at(dir))?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".tmp-")
        .tempfile_in(dir)
        .map_err(io_at(dir))?;
    use std::io::Write;
    tmp.write_all(data).map_err(io_at(path))?;
    if sync {
        tmp.as_file().sync_data().map_err(io_at(path))?;
    }
    tmp.persist(path).map_err(|e| Error::IoAt {
        path: path.to_path_buf(),
        source: e.error,
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> ObjectId {
        ObjectId::from_bytes([n; 32])
    }

    #[test]
    fn cas_semantics() {
        let dir = tempfile::tempdir().unwrap();
        let refs = RefStore::open(dir.path());
        let name = "refs/heads/main";
        assert_eq!(refs.read(name).unwrap(), None);
        refs.update(name, Expect::Absent, Some(id(1))).unwrap();
        assert!(matches!(
            refs.update(name, Expect::Absent, Some(id(2))),
            Err(Error::RefConflict { .. })
        ));
        assert!(matches!(
            refs.update(name, Expect::Is(id(9)), Some(id(2))),
            Err(Error::RefConflict { .. })
        ));
        refs.update(name, Expect::Is(id(1)), Some(id(2))).unwrap();
        assert_eq!(refs.read(name).unwrap(), Some(id(2)));
        refs.update(name, Expect::Is(id(2)), None).unwrap();
        assert_eq!(refs.read(name).unwrap(), None);
    }

    #[test]
    fn list_and_validate() {
        let dir = tempfile::tempdir().unwrap();
        let refs = RefStore::open(dir.path());
        refs.update("refs/heads/a", Expect::Any, Some(id(1)))
            .unwrap();
        refs.update("refs/heads/feature/x", Expect::Any, Some(id(2)))
            .unwrap();
        let l = refs.list(HEADS_PREFIX).unwrap();
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].0, "refs/heads/a");
        assert!(RefStore::validate_branch_name("ok/name-1").is_ok());
        assert!(RefStore::validate_branch_name("bad name").is_err());
        assert!(RefStore::validate_branch_name("../etc").is_err());
        assert!(RefStore::validate_branch_name(".hidden").is_err());
    }
}
