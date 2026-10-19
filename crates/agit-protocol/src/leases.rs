//! Leases shared through the remote server.
//!
//! A repository with an HTTP remote (`origin`, or `AGIT_LEASE_REMOTE`) shares
//! its leases with everyone who uses that server:
//!
//! * `claim`, `release`, `renew` act on the server first and keep a mirror in
//!   the local store with the same id (`Lease::remote` names the remote), so
//!   workspaces on this machine and offline checks still see them;
//! * `attach` plugs a [`ServerLeases`] check into the repository, so every
//!   commit, merge and `agit lease check` also refuses paths under another
//!   agent's exclusive lease on the server, wherever that agent works;
//! * `list` merges the server's leases with the local ones;
//! * `sync` registers leases that were claimed while the server was
//!   unreachable and drops mirrors of leases released elsewhere.
//!
//! `AGIT_LEASE_SYNC` selects the behaviour when the server cannot be reached:
//! `auto` (default) keeps working with local leases and warns once,
//! `required` fails instead, `off` never talks to the server for leases.
//! After a failed attempt the server is not asked again for 60 seconds, so an
//! offline machine does not pay a connect timeout on every commit.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agit_core::events::now_ms;
use agit_core::lease::{Lease, LeaseRequest, RemoteLeases};
use agit_core::{Error, Repository, Result};
use serde::Serialize;

use crate::client::Client;
use crate::types::LeaseCreate;

const BACKOFF_MS: i64 = 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncMode {
    Auto,
    Required,
    Off,
}

pub fn sync_mode() -> SyncMode {
    match std::env::var("AGIT_LEASE_SYNC")
        .unwrap_or_default()
        .to_lowercase()
        .as_str()
    {
        "off" | "0" | "false" | "no" | "local" => SyncMode::Off,
        "required" | "require" | "strict" => SyncMode::Required,
        _ => SyncMode::Auto,
    }
}

/// The server leases are shared through.
#[derive(Debug, Clone)]
pub struct LeaseServer {
    pub client: Client,
    /// Repository name on the server.
    pub repo: String,
    /// Remote name in this repository (`origin`).
    pub remote: String,
    marker: PathBuf,
}

impl LeaseServer {
    fn down_until(&self) -> i64 {
        std::fs::read_to_string(&self.marker)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0)
    }

    /// Recently unreachable: skip it for a while.
    fn backing_off(&self) -> bool {
        self.down_until() > now_ms()
    }

    fn mark_down(&self) {
        if let Some(p) = self.marker.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        let _ = std::fs::write(&self.marker, (now_ms() + BACKOFF_MS).to_string());
    }

    fn mark_up(&self) {
        if self.marker.exists() {
            let _ = std::fs::remove_file(&self.marker);
        }
    }
}

/// The lease server for `repo`: the remote named by `AGIT_LEASE_REMOTE`
/// (default `origin`) when it is an HTTP remote and sync is not off.
pub fn lease_server(repo: &Repository) -> Option<LeaseServer> {
    if sync_mode() == SyncMode::Off {
        return None;
    }
    let name = std::env::var("AGIT_LEASE_REMOTE")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "origin".into());
    let (url, _) = crate::sync::remote(repo, &name).ok()?;
    Some(LeaseServer {
        client: Client::new(url.base.clone())
            .with_timeouts(Duration::from_secs(2), Duration::from_secs(8)),
        repo: url.repo,
        remote: name,
        marker: repo.common_dir().join("leases").join(".server-unreachable"),
    })
}

static WARNED: AtomicBool = AtomicBool::new(false);

fn warn_once(srv: &LeaseServer, why: &str) {
    if !WARNED.swap(true, Ordering::SeqCst) {
        eprintln!(
            "agit: lease server {} ({}) not reachable: {why}; using leases of this machine only \
             (run `agit lease sync` once it is back, or set AGIT_LEASE_SYNC=required to fail instead)",
            srv.remote,
            srv.client.base()
        );
    }
}

/// Errors that mean "the server cannot help right now" rather than "no".
fn is_unavailable(e: &Error) -> bool {
    match e {
        Error::Invalid(m) => m.starts_with("transport error") || m.contains("server error 5"),
        Error::Unauthorized(_) => true,
        _ => false,
    }
}

/// Run `f` against the server. `Ok(None)` when the server is unavailable and
/// sync is not required (the caller falls back to local leases).
fn with_server<T>(srv: &LeaseServer, f: impl FnOnce(&Client) -> Result<T>) -> Result<Option<T>> {
    if srv.backing_off() && sync_mode() != SyncMode::Required {
        warn_once(srv, "it failed less than a minute ago");
        return Ok(None);
    }
    match f(&srv.client) {
        Ok(v) => {
            srv.mark_up();
            Ok(Some(v))
        }
        // Not pushed yet (`agit push --create` creates it): nothing is shared yet.
        Err(Error::Invalid(m)) if m.contains("[not_a_repository]") => Ok(None),
        Err(e) if is_unavailable(&e) => {
            if matches!(&e, Error::Invalid(m) if m.starts_with("transport error")) {
                srv.mark_down();
            }
            if sync_mode() == SyncMode::Required {
                return Err(Error::Invalid(format!(
                    "lease server {} unavailable (AGIT_LEASE_SYNC=required): {e}",
                    srv.client.base()
                )));
            }
            warn_once(srv, &e.to_string());
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

/// [`RemoteLeases`] backed by the server: plugged into a repository by
/// [`attach`].
pub struct ServerLeases {
    srv: LeaseServer,
}

impl RemoteLeases for ServerLeases {
    fn check(&self, holder: &str, paths: &[String]) -> Result<()> {
        with_server(&self.srv, |c| c.check_lease(&self.srv.repo, holder, paths)).map(|_| ())
    }
}

/// Make every lease check of `repo` (commit, merge, `lease check`) also ask
/// the shared server. No-op without an HTTP remote or with sync off.
pub fn attach(repo: &mut Repository) {
    if let Some(srv) = lease_server(repo) {
        repo.set_remote_leases(Arc::new(ServerLeases { srv }));
    }
}

/// Result of a claim.
#[derive(Debug, Clone, Serialize)]
pub struct Claimed {
    pub lease: Lease,
    /// Registered on the server (visible to the whole team).
    pub shared: bool,
}

/// Claim paths: on the server when there is one (team-wide), mirrored
/// locally; locally only without a server or while it is unreachable.
pub fn claim(repo: &Repository, req: LeaseRequest) -> Result<Claimed> {
    if req.paths.is_empty() {
        return Err(Error::Invalid("a lease needs at least one path".into()));
    }
    // Same machine first: other workspaces' leases that are not on the server.
    if req.mode == agit_core::lease::LeaseMode::Exclusive {
        if let Some((other, path)) = repo.leases().find_conflict(&req.holder, &req.paths)? {
            return Err(Error::LeaseConflict {
                path,
                holder: other.holder,
                lease_id: other.id,
            });
        }
    }
    if let Some(srv) = lease_server(repo) {
        let body = LeaseCreate {
            holder: req.holder.clone(),
            paths: req.paths.clone(),
            mode: req.mode,
            ttl_secs: Some(req.ttl.map(|t| t.as_secs().max(1)).unwrap_or(3600)),
            note: req.note.clone(),
        };
        if let Some(mut granted) = with_server(&srv, |c| c.claim_lease(&srv.repo, &body))? {
            granted.workspace = req.workspace.clone();
            granted.remote = Some(srv.remote.clone());
            let lease = repo.leases().adopt(granted)?;
            return Ok(Claimed {
                lease,
                shared: true,
            });
        }
    }
    let lease = repo.leases().claim(req)?;
    Ok(Claimed {
        lease,
        shared: false,
    })
}

fn release_on_server(srv: &LeaseServer, id: &str) -> Result<bool> {
    match with_server(srv, |c| c.release_lease(&srv.repo, id)) {
        Ok(Some(_)) => Ok(true),
        Ok(None) => Ok(false),
        Err(Error::LeaseNotFound(_)) | Err(Error::NotFound(_)) => Ok(false),
        Err(e) => Err(e),
    }
}

/// Release one lease by id, on the server and locally. Works for a lease
/// that exists in only one of the two places.
pub fn release(repo: &Repository, id: &str) -> Result<Lease> {
    let local = repo.leases().get(id).ok();
    let mut known = local.clone();
    if let Some(srv) = lease_server(repo) {
        if known.is_none() {
            if let Ok(Some(list)) = with_server(&srv, |c| c.leases(&srv.repo)) {
                known = list.into_iter().find(|l| l.id == id).map(|mut l| {
                    l.remote = Some(srv.remote.clone());
                    l
                });
            }
        }
        release_on_server(&srv, id)?;
    }
    if local.is_some() {
        repo.leases().release(id)?;
    }
    known.ok_or_else(|| Error::LeaseNotFound(id.into()))
}

/// Release every lease of `holder`, on the server and locally.
pub fn release_all(repo: &Repository, holder: &str) -> Result<Vec<Lease>> {
    let mut released: Vec<Lease> = Vec::new();
    if let Some(srv) = lease_server(repo) {
        if let Some(list) = with_server(&srv, |c| c.leases(&srv.repo))? {
            for mut l in list.into_iter().filter(|l| l.holder == holder) {
                if release_on_server(&srv, &l.id)? {
                    l.remote = Some(srv.remote.clone());
                    released.push(l);
                }
            }
        }
    }
    for l in repo.leases().release_all(holder)? {
        if !released.iter().any(|r| r.id == l.id) {
            released.push(l);
        }
    }
    Ok(released)
}

/// Extend a lease on the server and locally.
pub fn renew(repo: &Repository, id: &str, ttl: Duration) -> Result<Lease> {
    let local = repo.leases().get(id).ok();
    if let Some(srv) = lease_server(repo) {
        let on_server = local.as_ref().is_none_or(|l| l.remote.is_some());
        if on_server {
            match with_server(&srv, |c| c.renew_lease(&srv.repo, id, ttl.as_secs().max(1))) {
                Ok(Some(mut l)) => {
                    l.remote = Some(srv.remote.clone());
                    if let Some(loc) = &local {
                        l.workspace = loc.workspace.clone();
                        repo.leases().adopt(l.clone())?;
                    }
                    return Ok(l);
                }
                Ok(None) => {}
                Err(Error::LeaseNotFound(_)) | Err(Error::NotFound(_)) if local.is_some() => {}
                Err(e) => return Err(e),
            }
        }
    }
    repo.leases().renew(id, ttl)
}

/// Active leases: the server's (whole team) merged with this repository's
/// (local-only ones included), newest last.
pub fn list(repo: &Repository) -> Result<Vec<Lease>> {
    let mut out: Vec<Lease> = repo.leases().list()?;
    if let Some(srv) = lease_server(repo) {
        if let Some(server) = with_server(&srv, |c| c.leases(&srv.repo))? {
            let now = now_ms();
            // Mirrors of leases that ended on the server (released or expired
            // elsewhere) are stale.
            out.retain(|l| l.remote.is_none() || server.iter().any(|s| s.id == l.id));
            for mut s in server.into_iter().filter(|s| !s.is_expired_at(now)) {
                match out.iter_mut().find(|l| l.id == s.id) {
                    Some(l) => {
                        l.expires_ms = s.expires_ms;
                        l.remote = Some(srv.remote.clone());
                    }
                    None => {
                        s.remote = Some(srv.remote.clone());
                        out.push(s);
                    }
                }
            }
        }
    }
    out.sort_by_key(|l| l.created_ms);
    Ok(out)
}

/// What `sync` did.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SyncReport {
    /// Local-only leases now registered on the server.
    pub registered: Vec<Lease>,
    /// Local-only leases the server refused because someone else holds the
    /// paths: `(lease, conflicting holder)`. They stay local.
    pub refused: Vec<(Lease, String)>,
    /// Mirrors of leases that no longer exist on the server, dropped.
    pub dropped: Vec<String>,
    /// Active leases on the server after the sync.
    pub server_leases: usize,
}

/// Register local-only leases on the server and drop stale mirrors.
pub fn sync(repo: &Repository) -> Result<SyncReport> {
    let srv = lease_server(repo).ok_or_else(|| {
        Error::Invalid(
            "no lease server: add an HTTP remote (`agit remote add origin <url>`) or unset \
             AGIT_LEASE_SYNC=off"
                .into(),
        )
    })?;
    srv.mark_up();
    let server = srv.client.leases(&srv.repo)?;
    let mut report = SyncReport::default();
    let now = now_ms();
    for l in repo.leases().list()? {
        if l.remote.is_some() {
            if !server.iter().any(|s| s.id == l.id) {
                let _ = repo.leases().release(&l.id);
                report.dropped.push(l.id);
            }
            continue;
        }
        let ttl = l.expires_ms.map(|e| ((e - now) / 1000).max(1) as u64);
        let body = LeaseCreate {
            holder: l.holder.clone(),
            paths: l.paths.clone(),
            mode: l.mode,
            ttl_secs: Some(ttl.unwrap_or(3600)),
            note: l.note.clone(),
        };
        match srv.client.claim_lease(&srv.repo, &body) {
            Ok(mut granted) => {
                granted.workspace = l.workspace.clone();
                granted.remote = Some(srv.remote.clone());
                let _ = repo.leases().release(&l.id);
                report.registered.push(repo.leases().adopt(granted)?);
            }
            Err(Error::LeaseConflict { holder, .. }) => report.refused.push((l, holder)),
            Err(e) => return Err(e),
        }
    }
    report.server_leases = srv
        .client
        .leases(&srv.repo)?
        .into_iter()
        .filter(|l| !l.is_expired_at(now))
        .count();
    Ok(report)
}
