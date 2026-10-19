//! Server state: a registry of bare repositories on disk.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use agit_core::events::{now_ms, Event, EventLog};
use agit_core::lease::LeaseStore;
use agit_core::refs::{write_atomic, RefStore, HEADS_PREFIX, TAGS_PREFIX};
use agit_core::store::ObjectStore;
use agit_core::{Error, ObjectId, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoMeta {
    pub name: String,
    pub default_branch: String,
    pub created_ms: i64,
}

pub struct RepoHandle {
    pub meta: RepoMeta,
    pub dir: PathBuf,
    pub store: ObjectStore,
    pub refs: RefStore,
    pub events: EventLog,
    pub leases: LeaseStore,
    pub tx: broadcast::Sender<Event>,
    /// Serializes pull-request and knowledge file mutations.
    pub write_lock: Mutex<()>,
    /// Lazily built full-text index and staleness diff cache for notes.
    pub knowledge: crate::knowledge::KnowledgeCache,
}

impl RepoHandle {
    fn open(dir: PathBuf) -> Result<RepoHandle> {
        let meta_path = dir.join("meta.json");
        let meta: RepoMeta = serde_json::from_slice(&fs::read(&meta_path)?)?;
        for sub in [
            "objects",
            "refs/heads",
            "refs/tags",
            "locks",
            "leases",
            "pulls",
            "knowledge/notes",
        ] {
            fs::create_dir_all(dir.join(sub))?;
        }
        let (tx, _) = broadcast::channel(1024);
        let store = ObjectStore::open(dir.join("objects"));
        let hook_store = store.clone();
        Ok(RepoHandle {
            store,
            refs: RefStore::open(&dir).with_before_update(Arc::new(move || hook_store.flush())),
            events: EventLog::open(&dir),
            leases: LeaseStore::open(&dir),
            meta,
            dir,
            tx,
            write_lock: Mutex::new(()),
            knowledge: Default::default(),
        })
    }

    pub fn emit(&self, kind: &str, actor: Option<&str>, data: serde_json::Value) -> Result<Event> {
        let ev = self.events.append(kind, None, actor, data)?;
        let _ = self.tx.send(ev.clone());
        if !kind.starts_with("webhook.") {
            crate::webhooks::WebhookStore::new(self).dispatch(&ev);
        }
        Ok(ev)
    }

    pub fn head(&self) -> Result<Option<ObjectId>> {
        self.refs
            .read(&format!("{HEADS_PREFIX}{}", self.meta.default_branch))
    }

    /// Resolve a revision: full ref, branch, tag, or (abbreviated) id.
    pub fn resolve(&self, rev: &str) -> Result<ObjectId> {
        let base_end = rev.find(['~', '^']).unwrap_or(rev.len());
        if base_end < rev.len() {
            let (base, ops) = rev.split_at(base_end);
            let id = self.resolve(base)?;
            return agit_core::repo::apply_navigation(&self.store, id, ops, rev);
        }
        if rev == "HEAD" {
            return self
                .head()?
                .ok_or_else(|| Error::UnknownRevision("HEAD".into()));
        }
        if rev.starts_with("refs/") {
            return self
                .refs
                .read(rev)?
                .ok_or_else(|| Error::RefNotFound(rev.into()));
        }
        if let Some(id) = self.refs.read(&format!("{HEADS_PREFIX}{rev}"))? {
            return Ok(id);
        }
        if let Some(id) = self.refs.read(&format!("{TAGS_PREFIX}{rev}"))? {
            return Ok(id);
        }
        self.store.resolve_prefix(rev)
    }

    /// Normalize a branch name or full ref to a full ref name.
    pub fn full_ref(&self, name: &str) -> String {
        if name.starts_with("refs/") {
            name.to_string()
        } else {
            format!("{HEADS_PREFIX}{name}")
        }
    }

    pub fn summary(&self) -> Result<agit_protocol::types::RepoSummary> {
        Ok(agit_protocol::types::RepoSummary {
            name: self.meta.name.clone(),
            default_branch: self.meta.default_branch.clone(),
            head: self.head()?,
            created_ms: self.meta.created_ms,
            branches: self.refs.list(HEADS_PREFIX)?.len(),
            tags: self.refs.list(TAGS_PREFIX)?.len(),
            objects: 0,
        })
    }
}

pub struct AppState {
    pub data_dir: PathBuf,
    pub token: Option<String>,
    pub auth: crate::auth::AuthStore,
    repos: RwLock<HashMap<String, Arc<RepoHandle>>>,
}

fn valid_repo_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

impl AppState {
    pub fn open(data_dir: PathBuf, token: Option<String>) -> Result<AppState> {
        fs::create_dir_all(data_dir.join("repos"))?;
        let mut repos = HashMap::new();
        for e in fs::read_dir(data_dir.join("repos"))?.flatten() {
            if e.path().join("meta.json").is_file() {
                match RepoHandle::open(e.path()) {
                    Ok(h) => {
                        repos.insert(h.meta.name.clone(), Arc::new(h));
                    }
                    Err(err) => {
                        tracing::warn!(path = %e.path().display(), %err, "skipping unreadable repository")
                    }
                }
            }
        }
        tracing::info!(count = repos.len(), "repositories loaded");
        Ok(AppState {
            auth: crate::auth::AuthStore::open(&data_dir),
            data_dir,
            token,
            repos: RwLock::new(repos),
        })
    }

    /// SSH public keys with full admin rights (from `--admin-key`). A server
    /// with admin keys is closed even without a root token.
    pub fn with_admin_keys(mut self, keys: Vec<agit_core::sshkey::SshPublicKey>) -> Self {
        self.auth.set_admin_keys(keys);
        self
    }

    /// Does every request need credentials?
    pub fn closed(&self) -> bool {
        self.token.is_some() || self.auth.has_admin_keys()
    }

    pub fn repos_dir(&self) -> PathBuf {
        self.data_dir.join("repos")
    }

    pub fn list(&self) -> Vec<Arc<RepoHandle>> {
        let mut v: Vec<_> = self.repos.read().unwrap().values().cloned().collect();
        v.sort_by(|a, b| a.meta.name.cmp(&b.meta.name));
        v
    }

    pub fn get(&self, name: &str) -> Result<Arc<RepoHandle>> {
        self.repos
            .read()
            .unwrap()
            .get(name)
            .cloned()
            .ok_or_else(|| Error::NotARepository(Path::new(name).to_path_buf()))
    }

    pub fn create(&self, name: &str, default_branch: Option<&str>) -> Result<Arc<RepoHandle>> {
        if !valid_repo_name(name) {
            return Err(Error::Invalid(format!("invalid repository name {name:?}")));
        }
        let mut repos = self.repos.write().unwrap();
        if repos.contains_key(name) {
            return Err(Error::AlreadyExists(name.into()));
        }
        let dir = self.repos_dir().join(name);
        fs::create_dir_all(&dir)?;
        let meta = RepoMeta {
            name: name.to_string(),
            default_branch: default_branch.unwrap_or("main").to_string(),
            created_ms: now_ms(),
        };
        write_atomic(&dir.join("meta.json"), &serde_json::to_vec_pretty(&meta)?)?;
        let handle = Arc::new(RepoHandle::open(dir)?);
        handle.emit("repo.create", None, serde_json::json!({ "name": name }))?;
        repos.insert(name.to_string(), handle.clone());
        Ok(handle)
    }

    pub fn delete(&self, name: &str) -> Result<()> {
        let mut repos = self.repos.write().unwrap();
        let h = repos
            .remove(name)
            .ok_or_else(|| Error::NotARepository(Path::new(name).to_path_buf()))?;
        fs::remove_dir_all(&h.dir)?;
        Ok(())
    }
}
