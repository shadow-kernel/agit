//! Push, fetch, pull and clone between a local repository and a server.

use std::collections::HashSet;
use std::path::Path;

use agit_core::merge::is_ancestor;
use agit_core::refs::{Expect, HEADS_PREFIX, TAGS_PREFIX};
use agit_core::repo::RemoteConfig;
use agit_core::{Error, ObjectId, Repository, Result};
use serde::{Deserialize, Serialize};

use crate::client::{Client, RepoUrl};
use crate::stream;
use crate::types::{ExpectSpec, RefUpdate, RefUpdateRequest};
use crate::walk;

pub const REMOTES_PREFIX: &str = "refs/remotes/";
const UPLOAD_CHUNK_BYTES: usize = 8 * 1024 * 1024;
const MISSING_BATCH: usize = 2000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushResult {
    pub remote: String,
    pub branch: String,
    pub old: Option<ObjectId>,
    pub new: ObjectId,
    pub objects_sent: usize,
    pub up_to_date: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchedRef {
    pub name: String,
    pub old: Option<ObjectId>,
    pub new: ObjectId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchResult {
    pub remote: String,
    pub refs: Vec<FetchedRef>,
    pub objects_received: usize,
}

/// Look up a configured remote.
pub fn remote(repo: &Repository, name: &str) -> Result<(RepoUrl, RemoteConfig)> {
    let cfg = repo.config()?;
    let rc = cfg
        .remotes
        .get(name)
        .cloned()
        .ok_or_else(|| Error::Invalid(format!("no such remote: {name}")))?;
    Ok((RepoUrl::parse(&rc.url)?, rc))
}

pub fn add_remote(repo: &Repository, name: &str, url: &str) -> Result<()> {
    RepoUrl::parse(url)?;
    if name.is_empty() || name.contains(['/', ' ']) {
        return Err(Error::Invalid(format!("invalid remote name {name:?}")));
    }
    let mut cfg = repo.config()?;
    if cfg.remotes.contains_key(name) {
        return Err(Error::AlreadyExists(name.into()));
    }
    cfg.remotes.insert(name.to_string(), RemoteConfig { url: url.to_string() });
    repo.save_config(&cfg)
}

pub fn remove_remote(repo: &Repository, name: &str) -> Result<()> {
    let mut cfg = repo.config()?;
    if cfg.remotes.remove(name).is_none() {
        return Err(Error::Invalid(format!("no such remote: {name}")));
    }
    repo.save_config(&cfg)?;
    for (r, id) in repo.refs().list(&format!("{REMOTES_PREFIX}{name}/"))? {
        repo.refs().update(&r, Expect::Is(id), None)?;
    }
    Ok(())
}

pub fn client_for(repo: &Repository, name: &str) -> Result<(Client, String)> {
    let (url, _) = remote(repo, name)?;
    Ok((Client::new(url.base), url.repo))
}

fn local_tips(repo: &Repository) -> Result<Vec<ObjectId>> {
    let mut v: Vec<ObjectId> = repo.refs().list("refs/")?.into_iter().map(|(_, id)| id).collect();
    v.sort();
    v.dedup();
    Ok(v)
}

/// Upload every object reachable from `tip` that the server does not have.
fn upload_missing(repo: &Repository, client: &Client, remote_repo: &str, tip: ObjectId, haves: &[ObjectId]) -> Result<usize> {
    let store = repo.store();
    let ids = walk::objects_to_send(store, &[tip], haves)?;
    let mut missing: Vec<ObjectId> = Vec::new();
    for chunk in ids.chunks(MISSING_BATCH) {
        missing.extend(client.missing(remote_repo, chunk)?);
    }
    let missing_set: HashSet<ObjectId> = missing.iter().copied().collect();
    // Keep original order (trees/blobs before commits).
    let ordered: Vec<ObjectId> = ids.into_iter().filter(|i| missing_set.contains(i)).collect();
    let mut buf = Vec::new();
    let mut sent = 0;
    for id in &ordered {
        let (kind, payload) = store.read_raw(id)?;
        stream::encode_frame(&mut buf, kind, &payload);
        sent += 1;
        if buf.len() >= UPLOAD_CHUNK_BYTES {
            client.upload(remote_repo, &buf)?;
            buf.clear();
        }
    }
    if !buf.is_empty() {
        client.upload(remote_repo, &buf)?;
    }
    Ok(sent)
}

pub fn push(repo: &Repository, remote_name: &str, branch: &str, force: bool, create_repo: bool) -> Result<PushResult> {
    let (client, remote_repo) = client_for(repo, remote_name)?;
    let local_ref = format!("{HEADS_PREFIX}{branch}");
    let local = repo
        .refs()
        .read(&local_ref)?
        .ok_or_else(|| Error::RefNotFound(local_ref.clone()))?;

    if create_repo && client.repo(&remote_repo).is_err() {
        client.create_repo(&remote_repo, Some(branch))?;
    }
    let remote_refs = client.refs(&remote_repo, HEADS_PREFIX)?;
    let remote_old = remote_refs.iter().find(|r| r.name == local_ref).map(|r| r.id);
    let tracking = format!("{REMOTES_PREFIX}{remote_name}/{branch}");
    if remote_old == Some(local) {
        repo.refs().update(&tracking, Expect::Any, Some(local))?;
        return Ok(PushResult { remote: remote_name.into(), branch: branch.into(), old: remote_old, new: local, objects_sent: 0, up_to_date: true });
    }
    if let Some(old) = remote_old {
        if !force {
            let ff = repo.store().contains(&old) && is_ancestor(repo.store(), &old, &local)?;
            if !ff {
                return Err(Error::Invalid(format!(
                    "non-fast-forward: remote {branch} is at {} which is not an ancestor of {}; run `agit pull` first or push with --force",
                    old.short(),
                    local.short()
                )));
            }
        }
    }
    // Objects the server definitely has: its branch tips that we also have.
    let haves: Vec<ObjectId> = remote_refs.iter().map(|r| r.id).filter(|id| repo.store().contains(id)).collect();
    let objects_sent = upload_missing(repo, &client, &remote_repo, local, &haves)?;
    client.update_refs(
        &remote_repo,
        &RefUpdateRequest {
            updates: vec![RefUpdate {
                name: local_ref.clone(),
                expect: if force { ExpectSpec::Any(true) } else { ExpectSpec::from_old(remote_old) },
                new: Some(local),
            }],
            message: None,
        },
    )?;
    repo.refs().update(&tracking, Expect::Any, Some(local))?;
    repo.events().append(
        "push",
        Some(repo.workspace()),
        std::env::var("AGIT_AGENT").ok().as_deref(),
        serde_json::json!({ "remote": remote_name, "branch": branch, "old": remote_old, "new": local, "objects": objects_sent }),
    )?;
    Ok(PushResult { remote: remote_name.into(), branch: branch.into(), old: remote_old, new: local, objects_sent, up_to_date: false })
}

/// Fetch branches (all when `branches` is None) and tags into
/// `refs/remotes/<remote>/*` and `refs/tags/*`.
pub fn fetch(repo: &Repository, remote_name: &str, branches: Option<&[String]>) -> Result<FetchResult> {
    let (client, remote_repo) = client_for(repo, remote_name)?;
    let heads = client.refs(&remote_repo, HEADS_PREFIX)?;
    let tags = client.refs(&remote_repo, TAGS_PREFIX)?;
    let mut targets: Vec<(String, ObjectId)> = Vec::new();
    for r in &heads {
        let short = &r.name[HEADS_PREFIX.len()..];
        if branches.is_none_or(|b| b.iter().any(|x| x == short)) {
            targets.push((format!("{REMOTES_PREFIX}{remote_name}/{short}"), r.id));
        }
    }
    for r in &tags {
        targets.push((r.name.clone(), r.id));
    }
    let wants: Vec<ObjectId> = {
        let mut w: Vec<ObjectId> = targets.iter().map(|(_, id)| *id).filter(|id| !repo.store().contains(id)).collect();
        w.sort();
        w.dedup();
        w
    };
    let mut received = 0;
    if !wants.is_empty() {
        let haves = local_tips(repo)?;
        let bytes = client.fetch(&remote_repo, &wants, &haves)?;
        let (stored, _) = stream::import(repo.store(), &bytes)?;
        received = stored;
        for w in &wants {
            if !repo.store().contains(w) {
                return Err(Error::ObjectNotFound(*w));
            }
        }
    }
    let mut out = Vec::new();
    for (name, id) in targets {
        let old = repo.refs().read(&name)?;
        if old != Some(id) {
            // Tags are not overwritten once present.
            if name.starts_with(TAGS_PREFIX) && old.is_some() {
                continue;
            }
            repo.refs().update(&name, Expect::Any, Some(id))?;
            out.push(FetchedRef { name, old, new: id });
        }
    }
    repo.events().append(
        "fetch",
        Some(repo.workspace()),
        std::env::var("AGIT_AGENT").ok().as_deref(),
        serde_json::json!({ "remote": remote_name, "refs": out.len(), "objects": received }),
    )?;
    Ok(FetchResult { remote: remote_name.into(), refs: out, objects_received: received })
}

/// Fetch the current branch and merge its remote-tracking ref.
pub fn pull(repo: &Repository, remote_name: &str, opts: agit_core::ops::MergeOptions) -> Result<(FetchResult, agit_core::ops::MergeResult)> {
    let branch = repo
        .current_branch()?
        .ok_or_else(|| Error::Invalid("not on a branch; switch to a branch to pull".into()))?;
    let f = fetch(repo, remote_name, Some(&[branch.clone()]))?;
    let tracking = format!("{REMOTES_PREFIX}{remote_name}/{branch}");
    if repo.refs().read(&tracking)?.is_none() {
        return Err(Error::RefNotFound(tracking));
    }
    let m = agit_core::ops::merge(repo, &tracking, opts)?;
    Ok((f, m))
}

/// Clone another local repository (a plain directory path) into `dir`:
/// copies reachable objects and branch refs, checks out its HEAD branch.
pub fn clone_local(src_dir: &Path, dir: &Path) -> Result<(Repository, FetchResult)> {
    let src = Repository::discover(src_dir)?;
    let repo = Repository::init(dir)?;
    let mut cfg = repo.config()?;
    cfg.remotes.insert("origin".into(), RemoteConfig { url: src.work_dir().display().to_string() });
    repo.save_config(&cfg)?;
    let heads = src.refs().list(HEADS_PREFIX)?;
    let wants: Vec<ObjectId> = heads.iter().map(|(_, id)| *id).collect();
    let ids = walk::objects_to_send(src.store(), &wants, &[])?;
    let bytes = stream::export(src.store(), &ids)?;
    let (stored, _) = stream::import(repo.store(), &bytes)?;
    let mut refs = Vec::new();
    for (name, id) in &heads {
        let short = &name[HEADS_PREFIX.len()..];
        let tracking = format!("{REMOTES_PREFIX}origin/{short}");
        repo.refs().update(&tracking, Expect::Any, Some(*id))?;
        refs.push(FetchedRef { name: tracking, old: None, new: *id });
    }
    let branch = src.current_branch()?.unwrap_or_else(|| "main".into());
    repo.set_head(&agit_core::Head::Branch(branch.clone()))?;
    if let Some(id) = src.refs().read(&format!("{HEADS_PREFIX}{branch}"))? {
        repo.refs().update(&format!("{HEADS_PREFIX}{branch}"), Expect::Absent, Some(id))?;
        let tree = repo.store().read_commit(&id)?.tree;
        agit_core::worktree::checkout_tree(&repo, None, &tree, true)?;
    }
    Ok((repo, FetchResult { remote: "origin".into(), refs, objects_received: stored }))
}

/// Is `url` a local directory rather than a server URL?
pub fn is_local_url(url: &str) -> bool {
    !url.contains("://") || url.starts_with("file://")
}

/// Clone `url` into `dir`.
pub fn clone(url: &str, dir: &Path) -> Result<(Repository, FetchResult)> {
    if is_local_url(url) {
        let p = url.strip_prefix("file://").unwrap_or(url);
        return clone_local(Path::new(p), dir);
    }
    let parsed = RepoUrl::parse(url)?;
    let client = Client::new(parsed.base.clone());
    let summary = client.repo(&parsed.repo)?;
    let repo = Repository::init(dir)?;
    add_remote(&repo, "origin", url)?;
    let mut cfg = repo.config()?;
    cfg.default_branch = summary.default_branch.clone();
    repo.save_config(&cfg)?;
    let f = fetch(&repo, "origin", None)?;
    let tracking = format!("{REMOTES_PREFIX}origin/{}", summary.default_branch);
    let head_ref = format!("{HEADS_PREFIX}{}", summary.default_branch);
    repo.set_head(&agit_core::Head::Branch(summary.default_branch.clone()))?;
    if let Some(id) = repo.refs().read(&tracking)? {
        repo.refs().update(&head_ref, Expect::Absent, Some(id))?;
        let tree = repo.store().read_commit(&id)?.tree;
        agit_core::worktree::checkout_tree(&repo, None, &tree, true)?;
    }
    Ok((repo, f))
}
