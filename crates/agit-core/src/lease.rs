//! Leases: advisory, time-limited claims on paths.
//!
//! When several agents work in one repository the expensive failure is not
//! the merge conflict itself but discovering it *after* both agents spent
//! their budget. A lease lets an agent announce "I am editing `src/auth/**`
//! for the next 30 minutes" up front. Other agents see the claim immediately
//! and either pick different work or wait.
//!
//! Leases are advisory: `agit commit` refuses to commit into another agent's
//! exclusive lease unless `--force` is given, but nothing stops a raw file
//! write. Leases expire automatically so a crashed agent cannot block work
//! forever.
//!
//! This store holds the leases of one repository on one machine (shared by
//! its workspaces). When the repository has a remote, the protocol layer
//! registers every claim on the server first and keeps a mirror here
//! (`Lease::remote`), and plugs a [`RemoteLeases`] check into the
//! [`crate::Repository`], so commits and merges also respect the leases of
//! agents on other machines.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use globset::{Glob, GlobMatcher};
use serde::{Deserialize, Serialize};

use crate::error::{io_at, Error, Result};
use crate::events::now_ms;
use crate::lock::Lock;
use crate::refs::write_atomic;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LeaseMode {
    /// Only the holder should modify the paths.
    #[default]
    Exclusive,
    /// Informational: the holder is reading/depending on these paths.
    Shared,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    pub id: String,
    pub holder: String,
    /// Path patterns. A pattern without glob characters matches itself and
    /// everything beneath it (`src/auth` covers `src/auth/login.rs`).
    pub paths: Vec<String>,
    pub mode: LeaseMode,
    pub created_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// Name of the remote this lease is registered on (it is visible to and
    /// enforced for everyone using that server); `None` for a lease that
    /// exists only in this repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
}

/// Leases kept somewhere else, typically the server shared with agents on
/// other machines. The protocol layer plugs an implementation into a
/// [`crate::Repository`]; every lease check of the core (commit, merge,
/// `agit lease check`) then asks it after the local store.
pub trait RemoteLeases: Send + Sync {
    /// `Ok(())` when `holder` may modify `paths`, `Err(LeaseConflict)` when
    /// another holder's exclusive lease covers one of them.
    fn check(&self, holder: &str, paths: &[String]) -> Result<()>;
}

/// Cloneable, debuggable handle to a [`RemoteLeases`] implementation.
#[derive(Clone)]
pub struct RemoteLeaseHook(pub Arc<dyn RemoteLeases>);

impl std::fmt::Debug for RemoteLeaseHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RemoteLeaseHook")
    }
}

impl Lease {
    pub fn is_expired_at(&self, now: i64) -> bool {
        self.expires_ms.is_some_and(|e| e <= now)
    }

    pub fn is_expired(&self) -> bool {
        self.is_expired_at(now_ms())
    }

    pub fn covers(&self, path: &str) -> bool {
        self.paths.iter().any(|p| pattern_covers(p, path))
    }
}

/// Does `pattern` cover the concrete `path`?
pub fn pattern_covers(pattern: &str, path: &str) -> bool {
    let pattern = pattern.trim_end_matches('/');
    let path = path.trim_end_matches('/');
    if pattern.is_empty() || pattern == "." {
        return true;
    }
    if has_glob(pattern) {
        if let Ok(m) = matcher(pattern) {
            if m.is_match(path) {
                return true;
            }
            // A glob like `src/*` should also cover `src/a/b` for leasing
            // purposes: check every ancestor.
            let mut cur = path;
            while let Some(i) = cur.rfind('/') {
                cur = &cur[..i];
                if m.is_match(cur) {
                    return true;
                }
            }
        }
        return false;
    }
    path == pattern || path.starts_with(&format!("{pattern}/"))
}

/// Conservative overlap test between two patterns. Returns true when the two
/// *might* cover a common path. Literal prefixes are compared; globs fall
/// back to prefix-of-literal-part comparison.
pub fn patterns_overlap(a: &str, b: &str) -> bool {
    let la = literal_prefix(a);
    let lb = literal_prefix(b);
    let la = la.trim_end_matches('/');
    let lb = lb.trim_end_matches('/');
    if la.is_empty() || lb.is_empty() {
        return true;
    }
    la == lb || la.starts_with(&format!("{lb}/")) || lb.starts_with(&format!("{la}/"))
        // `src/foo*` vs `src/foobar`: literal prefix of the glob is a string
        // prefix of the other (not on a '/' boundary) when the glob char
        // immediately followed the literal part.
        || (has_glob(a) && lb.starts_with(la))
        || (has_glob(b) && la.starts_with(lb))
}

fn has_glob(p: &str) -> bool {
    p.contains(['*', '?', '[', '{'])
}

fn literal_prefix(p: &str) -> &str {
    match p.find(['*', '?', '[', '{']) {
        Some(i) => &p[..i],
        None => p,
    }
}

fn matcher(pattern: &str) -> Result<GlobMatcher> {
    Glob::new(pattern)
        .map(|g| g.compile_matcher())
        .map_err(|e| Error::Invalid(format!("bad path pattern {pattern:?}: {e}")))
}

#[derive(Debug, Clone)]
pub struct LeaseRequest {
    pub holder: String,
    pub paths: Vec<String>,
    pub mode: LeaseMode,
    pub ttl: Option<Duration>,
    pub note: Option<String>,
    pub workspace: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LeaseStore {
    dir: PathBuf,
    lock_path: PathBuf,
}

impl LeaseStore {
    pub fn open(agit_dir: &Path) -> Self {
        LeaseStore {
            dir: agit_dir.join("leases"),
            lock_path: agit_dir.join("locks").join("leases.lock"),
        }
    }

    fn lock(&self) -> Result<Lock> {
        Lock::acquire(&self.lock_path, Duration::from_secs(5))
    }

    fn path_for(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    fn read_all_unlocked(&self) -> Result<Vec<Lease>> {
        let mut out = Vec::new();
        let rd = match fs::read_dir(&self.dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => {
                return Err(Error::IoAt {
                    path: self.dir.clone(),
                    source: e,
                })
            }
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.extension().is_some_and(|e| e == "json") {
                // A concurrent release may delete the file between listing
                // and reading; that lease is simply gone.
                let bytes = match fs::read(&p) {
                    Ok(b) => b,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(e) => return Err(Error::IoAt { path: p, source: e }),
                };
                if let Ok(l) = serde_json::from_slice::<Lease>(&bytes) {
                    out.push(l);
                }
            }
        }
        out.sort_by_key(|l| l.created_ms);
        Ok(out)
    }

    /// Active (non-expired) leases.
    pub fn list(&self) -> Result<Vec<Lease>> {
        let now = now_ms();
        Ok(self
            .read_all_unlocked()?
            .into_iter()
            .filter(|l| !l.is_expired_at(now))
            .collect())
    }

    pub fn get(&self, id: &str) -> Result<Lease> {
        let p = self.path_for(id);
        let bytes = fs::read(&p).map_err(|_| Error::LeaseNotFound(id.to_string()))?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// Find the first active exclusive lease by someone other than `holder`
    /// that conflicts with any of `paths` (concrete paths or patterns).
    pub fn find_conflict(&self, holder: &str, paths: &[String]) -> Result<Option<(Lease, String)>> {
        let active = self.list()?;
        for lease in active {
            if lease.holder == holder || lease.mode != LeaseMode::Exclusive {
                continue;
            }
            for p in paths {
                let overlaps = lease.paths.iter().any(|lp| {
                    if has_glob(p) || has_glob(lp) {
                        patterns_overlap(lp, p)
                    } else {
                        pattern_covers(lp, p) || pattern_covers(p, lp)
                    }
                });
                if overlaps {
                    return Ok(Some((lease, p.clone())));
                }
            }
        }
        Ok(None)
    }

    pub fn claim(&self, req: LeaseRequest) -> Result<Lease> {
        if req.paths.is_empty() {
            return Err(Error::Invalid("a lease needs at least one path".into()));
        }
        for p in &req.paths {
            if has_glob(p) {
                matcher(p)?;
            }
        }
        let _guard = self.lock()?;
        self.purge_expired_unlocked()?;
        // Idempotent: an active lease of the same holder and mode that already
        // covers every requested path is renewed instead of duplicated.
        if let Some(existing) = self
            .read_all_unlocked()?
            .into_iter()
            .filter(|l| l.holder == req.holder && l.mode == req.mode && !l.is_expired())
            .find(|l| {
                req.paths
                    .iter()
                    .all(|p| l.paths.contains(p) || (!has_glob(p) && l.covers(p)))
            })
        {
            let mut renewed = existing;
            if let Some(ttl) = req.ttl {
                renewed.expires_ms = Some(now_ms() + ttl.as_millis() as i64);
            }
            if req.note.is_some() {
                renewed.note = req.note;
            }
            write_atomic(
                &self.path_for(&renewed.id),
                &serde_json::to_vec_pretty(&renewed)?,
            )?;
            return Ok(renewed);
        }
        if req.mode == LeaseMode::Exclusive {
            if let Some((other, path)) = self.find_conflict(&req.holder, &req.paths)? {
                return Err(Error::LeaseConflict {
                    path,
                    holder: other.holder,
                    lease_id: other.id,
                });
            }
        }
        let now = now_ms();
        let lease = Lease {
            id: uuid::Uuid::new_v4().simple().to_string()[..12].to_string(),
            holder: req.holder,
            paths: req.paths,
            mode: req.mode,
            created_ms: now,
            expires_ms: req.ttl.map(|t| now + t.as_millis() as i64),
            note: req.note,
            workspace: req.workspace,
            remote: None,
        };
        fs::create_dir_all(&self.dir).map_err(io_at(&self.dir))?;
        write_atomic(
            &self.path_for(&lease.id),
            &serde_json::to_vec_pretty(&lease)?,
        )?;
        Ok(lease)
    }

    /// Store `lease` as given (same id), replacing a lease with that id.
    /// Used to mirror a lease the server granted.
    pub fn adopt(&self, lease: Lease) -> Result<Lease> {
        let _guard = self.lock()?;
        fs::create_dir_all(&self.dir).map_err(io_at(&self.dir))?;
        write_atomic(
            &self.path_for(&lease.id),
            &serde_json::to_vec_pretty(&lease)?,
        )?;
        Ok(lease)
    }

    pub fn renew(&self, id: &str, ttl: Duration) -> Result<Lease> {
        let _guard = self.lock()?;
        let mut lease = self.get(id)?;
        lease.expires_ms = Some(now_ms() + ttl.as_millis() as i64);
        write_atomic(&self.path_for(id), &serde_json::to_vec_pretty(&lease)?)?;
        Ok(lease)
    }

    pub fn release(&self, id: &str) -> Result<Lease> {
        let _guard = self.lock()?;
        let lease = self.get(id)?;
        fs::remove_file(self.path_for(id)).map_err(io_at(self.path_for(id)))?;
        Ok(lease)
    }

    /// Release every lease of `holder`. Returns how many were released.
    pub fn release_all(&self, holder: &str) -> Result<Vec<Lease>> {
        let _guard = self.lock()?;
        let mut released = Vec::new();
        for l in self.read_all_unlocked()? {
            if l.holder == holder {
                fs::remove_file(self.path_for(&l.id)).map_err(io_at(self.path_for(&l.id)))?;
                released.push(l);
            }
        }
        Ok(released)
    }

    fn purge_expired_unlocked(&self) -> Result<usize> {
        let now = now_ms();
        let mut n = 0;
        for l in self.read_all_unlocked()? {
            if l.is_expired_at(now) {
                let _ = fs::remove_file(self.path_for(&l.id));
                n += 1;
            }
        }
        Ok(n)
    }

    pub fn purge_expired(&self) -> Result<usize> {
        let _guard = self.lock()?;
        self.purge_expired_unlocked()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(holder: &str, paths: &[&str]) -> LeaseRequest {
        LeaseRequest {
            holder: holder.into(),
            paths: paths.iter().map(|s| s.to_string()).collect(),
            mode: LeaseMode::Exclusive,
            ttl: Some(Duration::from_secs(60)),
            note: None,
            workspace: None,
        }
    }

    #[test]
    fn covers_and_overlap() {
        assert!(pattern_covers("src/auth", "src/auth/login.rs"));
        assert!(pattern_covers("src/auth", "src/auth"));
        assert!(!pattern_covers("src/auth", "src/authz/x.rs"));
        assert!(pattern_covers("src/**/*.rs", "src/a/b/c.rs"));
        assert!(pattern_covers("*.md", "README.md"));
        assert!(patterns_overlap("src", "src/auth/**"));
        assert!(!patterns_overlap("src/a", "src/b"));
        assert!(patterns_overlap("src/foo*", "src/foobar/x"));
    }

    #[test]
    fn exclusive_conflict_and_expiry() {
        let dir = tempfile::tempdir().unwrap();
        let store = LeaseStore::open(dir.path());
        let a = store.claim(req("agent-a", &["src/auth"])).unwrap();
        let err = store
            .claim(req("agent-b", &["src/auth/login.rs"]))
            .unwrap_err();
        assert!(matches!(err, Error::LeaseConflict { .. }));
        // Same holder re-claiming a covered path gets the existing lease back
        // (idempotent), not a duplicate.
        let again = store.claim(req("agent-a", &["src/auth/x"])).unwrap();
        assert_eq!(again.id, a.id);
        // A path outside the existing lease creates a new one.
        store.claim(req("agent-a", &["docs2"])).unwrap();
        // Disjoint path is fine for another holder.
        store.claim(req("agent-b", &["src/db"])).unwrap();
        assert_eq!(store.list().unwrap().len(), 3);
        store.release(&a.id).unwrap();
        assert_eq!(store.list().unwrap().len(), 2);

        let mut r = req("agent-c", &["docs"]);
        r.ttl = Some(Duration::from_millis(1));
        store.claim(r).unwrap();
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(store.list().unwrap().len(), 2);
        store.claim(req("agent-d", &["docs/x.md"])).unwrap();
    }
}
