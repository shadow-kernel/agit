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

type BeforeUpdate = std::sync::Arc<dyn Fn() -> Result<()> + Send + Sync>;

/// Group-commit journal shared by every `RefStore` handle of a directory
/// within one process.
///
/// Durability of ref updates comes from `refs.journal`, not from fsyncing
/// each ref file: an update writes its ref file without fsync, appends one
/// line to the journal and then joins the next journal flush. Concurrent
/// updaters share that single fsync, so throughput scales with the number
/// of writers instead of being serialized on the disk. On open, the journal
/// tail is replayed over the ref files (`recover`), so a crash after the
/// fsync never loses an acknowledged update.
struct Journal {
    path: PathBuf,
    state: std::sync::Mutex<JournalState>,
    cv: std::sync::Condvar,
}

#[derive(Default)]
struct JournalState {
    pending: Vec<u8>,
    /// Generation number: bumped after every fsync.
    committed: u64,
    /// Generation that the pending buffer will become.
    next: u64,
    flushing: bool,
}

/// Checkpoint (fsync every ref file and truncate) once the journal grows past this.
const JOURNAL_ROTATE_BYTES: u64 = 8 * 1024 * 1024;

static JOURNALS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<PathBuf, std::sync::Arc<Journal>>>,
> = std::sync::OnceLock::new();

impl Journal {
    fn for_dir(dir: &Path) -> std::sync::Arc<Journal> {
        let map = JOURNALS.get_or_init(|| std::sync::Mutex::new(Default::default()));
        let mut map = map.lock().unwrap();
        map.entry(dir.to_path_buf())
            .or_insert_with(|| {
                std::sync::Arc::new(Journal {
                    path: dir.join("refs.journal"),
                    state: std::sync::Mutex::new(JournalState {
                        next: 1,
                        ..Default::default()
                    }),
                    cv: std::sync::Condvar::new(),
                })
            })
            .clone()
    }

    /// Append `line` and return once it is durable (one fsync per batch).
    fn commit(&self, line: &str) -> Result<()> {
        if !crate::durability::sync_refs() {
            // No durability requested: still keep the journal for recovery of
            // torn ref files, but never wait on the disk.
            let mut f = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
                .map_err(io_at(&self.path))?;
            use std::io::Write;
            f.write_all(line.as_bytes()).map_err(io_at(&self.path))?;
            return Ok(());
        }
        let mut st = self.state.lock().unwrap();
        st.pending.extend_from_slice(line.as_bytes());
        let my_gen = st.next;
        loop {
            if st.committed >= my_gen {
                return Ok(());
            }
            if st.flushing {
                // A leader is writing an earlier batch; wait for it.
                st = self.cv.wait(st).unwrap();
                continue;
            }
            // Become the leader for everything queued so far. Writers that
            // arrive during the fsync queue for the next batch, so under
            // load batches form by themselves and a lone writer never waits.
            st.flushing = true;
            let batch = std::mem::take(&mut st.pending);
            let gen = st.next;
            st.next += 1;
            drop(st);
            let result = (|| -> Result<()> {
                let mut f = fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)
                    .map_err(io_at(&self.path))?;
                use std::io::Write;
                f.write_all(&batch).map_err(io_at(&self.path))?;
                crate::durability::sync_file(&f).map_err(io_at(&self.path))?;
                Ok(())
            })();
            st = self.state.lock().unwrap();
            st.committed = gen;
            st.flushing = false;
            self.cv.notify_all();
            result?;
        }
    }

    /// Replay the journal over the ref files: the last entry per ref wins.
    fn recover(&self, dir: &Path) -> Result<usize> {
        let text = match fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => {
                return Err(Error::IoAt {
                    path: self.path.clone(),
                    source: e,
                })
            }
        };
        let mut last: std::collections::HashMap<&str, Option<&str>> =
            std::collections::HashMap::new();
        for line in text.lines() {
            let mut parts = line.split(' ');
            if let (Some(name), Some(new)) = (parts.next(), parts.next()) {
                last.insert(name, if new == "-" { None } else { Some(new) });
            }
        }
        let mut repaired = 0;
        for (name, new) in last {
            if RefStore::validate_name(name).is_err() {
                continue;
            }
            let path = dir.join(name);
            let current = fs::read_to_string(&path).ok().map(|s| s.trim().to_string());
            match new {
                Some(id) if current.as_deref() != Some(id) => {
                    write_atomic_with(&path, format!("{id}\n").as_bytes(), false)?;
                    repaired += 1;
                }
                None if current.is_some() => {
                    let _ = fs::remove_file(&path);
                    repaired += 1;
                }
                _ => {}
            }
        }
        Ok(repaired)
    }

    /// Make every ref file durable on its own and truncate the journal.
    fn checkpoint(&self, dir: &Path) -> Result<()> {
        let len = fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        if len < JOURNAL_ROTATE_BYTES {
            return Ok(());
        }
        let _st = self.state.lock().unwrap();
        for sub in [
            "refs/heads",
            "refs/tags",
            "refs/checkpoints",
            "refs/remotes",
        ] {
            let base = dir.join(sub);
            let mut stack = vec![base];
            while let Some(d) = stack.pop() {
                let Ok(rd) = fs::read_dir(&d) else { continue };
                for e in rd.flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        stack.push(p);
                    } else if let Ok(f) = fs::File::open(&p) {
                        let _ = crate::durability::sync_file(&f);
                    }
                }
            }
        }
        write_atomic_with(&self.path, b"", true)?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct RefStore {
    dir: PathBuf,
    lock_path: PathBuf,
    /// Called before any ref is written; the repository uses it to publish
    /// pending objects so a ref never points at an unpublished object.
    before_update: Option<BeforeUpdate>,
    journal: std::sync::Arc<Journal>,
}

impl std::fmt::Debug for RefStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RefStore").field("dir", &self.dir).finish()
    }
}

impl RefStore {
    pub fn open(agit_dir: &Path) -> Self {
        let journal = Journal::for_dir(agit_dir);
        // Replay whatever the last process acknowledged but did not get to
        // write into the ref files (crash between journal fsync and rename).
        let _ = journal.recover(agit_dir);
        RefStore {
            dir: agit_dir.to_path_buf(),
            lock_path: agit_dir.join("locks").join("refs.lock"),
            before_update: None,
            journal,
        }
    }

    fn journal_line(name: &str, new: Option<ObjectId>) -> String {
        format!(
            "{name} {} {}\n",
            new.map(|i| i.to_hex()).unwrap_or_else(|| "-".into()),
            crate::events::now_ms()
        )
    }

    /// Install the publish hook (see [`crate::store::ObjectStore::flush`]).
    pub fn with_before_update(mut self, f: BeforeUpdate) -> Self {
        self.before_update = Some(f);
        self
    }

    fn publish(&self) -> Result<()> {
        match &self.before_update {
            Some(f) => f(),
            None => Ok(()),
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
        if new.is_some() {
            self.publish()?;
        }
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
            Some(id) => write_atomic_with(&path, format!("{id}\n").as_bytes(), false)?,
            None => match fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(Error::IoAt { path, source: e }),
            },
        }
        if current != new {
            crate::reflog::append(&self.dir, name, current, new, message)?;
        }
        drop(_guard);
        // Durability: one group fsync of the journal instead of one per ref.
        self.journal.commit(&Self::journal_line(name, new))?;
        let _ = self.journal.checkpoint(&self.dir);
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
        if updates.iter().any(|(_, _, n)| n.is_some()) {
            self.publish()?;
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
        let mut lines = String::new();
        for ((name, _, new), old) in updates.iter().zip(olds) {
            lines.push_str(&Self::journal_line(name, *new));
            let path = self.path_for(name);
            match new {
                Some(id) => write_atomic_with(&path, format!("{id}\n").as_bytes(), false)?,
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
        drop(_guard);
        self.journal.commit(&lines)?;
        let _ = self.journal.checkpoint(&self.dir);
        Ok(())
    }

    /// List all refs under `prefix` (e.g. `refs/heads/`), sorted by name.
    pub fn list(&self, prefix: &str) -> Result<Vec<(String, ObjectId)>> {
        let base = self.path_for(prefix.trim_end_matches('/'));
        let mut out = Vec::new();
        if !base.exists() {
            return Ok(out);
        }
        // The prefix names one ref exactly.
        if base.is_file() {
            let name = prefix.trim_end_matches('/').to_string();
            if let Some(id) = self.read(&name)? {
                out.push((name, id));
            }
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
        crate::durability::sync_file(tmp.as_file()).map_err(io_at(path))?;
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

#[cfg(test)]
mod journal_tests {
    use super::*;

    fn id(n: u8) -> ObjectId {
        ObjectId::from_bytes([n; 32])
    }

    #[test]
    fn journal_recovers_torn_ref_files() {
        let dir = tempfile::tempdir().unwrap();
        let refs = RefStore::open(dir.path());
        refs.update("refs/heads/main", Expect::Any, Some(id(1)))
            .unwrap();
        refs.update("refs/heads/main", Expect::Any, Some(id(2)))
            .unwrap();
        refs.update("refs/heads/gone", Expect::Any, Some(id(3)))
            .unwrap();
        refs.update("refs/heads/gone", Expect::Any, None).unwrap();
        // Simulate a crash after the journal fsync but before the ref file
        // reached disk: clobber the ref file and resurrect the deleted one.
        fs::write(dir.path().join("refs/heads/main"), format!("{}\n", id(1))).unwrap();
        fs::write(dir.path().join("refs/heads/gone"), format!("{}\n", id(3))).unwrap();
        let again = RefStore::open(dir.path());
        assert_eq!(again.read("refs/heads/main").unwrap(), Some(id(2)));
        assert_eq!(again.read("refs/heads/gone").unwrap(), None);
    }

    #[test]
    fn concurrent_updates_share_commits() {
        let dir = tempfile::tempdir().unwrap();
        let refs = RefStore::open(dir.path());
        let handles: Vec<_> = (0..8u8)
            .map(|i| {
                let r = refs.clone();
                std::thread::spawn(move || {
                    for j in 0..20u8 {
                        r.update(&format!("refs/heads/b{i}"), Expect::Any, Some(id(j)))
                            .unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        for i in 0..8u8 {
            assert_eq!(
                refs.read(&format!("refs/heads/b{i}")).unwrap(),
                Some(id(19))
            );
        }
        let journal = fs::read_to_string(dir.path().join("refs.journal")).unwrap();
        assert_eq!(journal.lines().count(), 160);
    }
}
