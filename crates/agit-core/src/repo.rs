//! Repository discovery, layout, HEAD, configuration and workspaces.
//!
//! Layout of the common directory (`.agit/`):
//!
//! ```text
//! .agit/
//!   HEAD                 (primary workspace)
//!   index                (primary workspace)
//!   config.json
//!   objects/             content-addressed store (shared by all workspaces)
//!   refs/heads|tags|checkpoints/
//!   locks/               flock files
//!   leases/
//!   events.jsonl
//!   workspaces/<name>/   HEAD, index, meta.json  (secondary workspaces)
//! ```
//!
//! A secondary workspace's working directory contains a `.agit` *file*
//! pointing back at the common directory. Every workspace has its own HEAD
//! and index but shares objects, refs, leases and events, which is exactly
//! what a fleet of agents working on one repository needs.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{io_at, Error, Result};
use crate::events::EventLog;
use crate::hash::ObjectId;
use crate::index::Index;
use crate::lease::LeaseStore;
use crate::object::Signature;
use crate::refs::{write_atomic, Expect, RefStore, HEADS_PREFIX, TAGS_PREFIX};
use crate::store::ObjectStore;

pub const AGIT_DIR: &str = ".agit";
pub const PRIMARY_WORKSPACE: &str = "main";
pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteConfig {
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub format_version: u32,
    #[serde(default = "default_branch")]
    pub default_branch: String,
    #[serde(default)]
    pub user: UserConfig,
    #[serde(default)]
    pub remotes: std::collections::BTreeMap<String, RemoteConfig>,
}

fn default_branch() -> String {
    "main".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Config {
            format_version: FORMAT_VERSION,
            default_branch: default_branch(),
            user: UserConfig::default(),
            remotes: Default::default(),
        }
    }
}

/// Pointer file written into secondary workspaces.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspacePointer {
    common_dir: PathBuf,
    workspace: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceMeta {
    pub name: String,
    pub path: PathBuf,
    pub created_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    Branch(String),
    Detached(ObjectId),
}

#[derive(Debug, Clone)]
pub struct Repository {
    common_dir: PathBuf,
    ws_dir: PathBuf,
    work_dir: PathBuf,
    workspace: String,
    store: ObjectStore,
    refs: RefStore,
    events: EventLog,
    leases: LeaseStore,
}

impl Repository {
    // -- lifecycle -----------------------------------------------------------

    pub fn init(work_dir: &Path) -> Result<Repository> {
        let work_dir = absolutize(work_dir)?;
        let common = work_dir.join(AGIT_DIR);
        if common.exists() {
            return Err(Error::AlreadyExists(common));
        }
        fs::create_dir_all(&work_dir).map_err(io_at(&work_dir))?;
        fs::create_dir_all(&common).map_err(io_at(&common))?;
        for sub in [
            "objects",
            "refs/heads",
            "refs/tags",
            "refs/checkpoints",
            "locks",
            "leases",
            "workspaces",
        ] {
            let p = common.join(sub);
            fs::create_dir_all(&p).map_err(io_at(&p))?;
        }
        let cfg = Config::default();
        write_atomic(
            &common.join("config.json"),
            &serde_json::to_vec_pretty(&cfg)?,
        )?;
        write_atomic(
            &common.join("HEAD"),
            format!("ref: {HEADS_PREFIX}{}\n", cfg.default_branch).as_bytes(),
        )?;
        let repo = Repository::open_primary(common, work_dir)?;
        repo.events().append(
            "repo.init",
            Some(PRIMARY_WORKSPACE),
            None,
            serde_json::json!({ "path": repo.work_dir() }),
        )?;
        Ok(repo)
    }

    fn open_primary(common_dir: PathBuf, work_dir: PathBuf) -> Result<Repository> {
        Ok(Repository {
            store: ObjectStore::open(common_dir.join("objects")),
            refs: RefStore::open(&common_dir),
            events: EventLog::open(&common_dir),
            leases: LeaseStore::open(&common_dir),
            ws_dir: common_dir.clone(),
            common_dir,
            work_dir,
            workspace: PRIMARY_WORKSPACE.to_string(),
        })
    }

    fn open_secondary(common_dir: PathBuf, work_dir: PathBuf, name: String) -> Result<Repository> {
        let ws_dir = common_dir.join("workspaces").join(&name);
        if !ws_dir.is_dir() {
            return Err(Error::WorkspaceNotFound(name));
        }
        Ok(Repository {
            store: ObjectStore::open(common_dir.join("objects")),
            refs: RefStore::open(&common_dir),
            events: EventLog::open(&common_dir),
            leases: LeaseStore::open(&common_dir),
            ws_dir,
            common_dir,
            work_dir,
            workspace: name,
        })
    }

    /// Walk up from `start` to find a repository.
    pub fn discover(start: &Path) -> Result<Repository> {
        let start = absolutize(start)?;
        let mut cur: Option<&Path> = Some(&start);
        while let Some(dir) = cur {
            let candidate = dir.join(AGIT_DIR);
            if candidate.is_dir() {
                return Repository::open_primary(candidate, dir.to_path_buf());
            }
            if candidate.is_file() {
                let bytes = fs::read(&candidate).map_err(io_at(&candidate))?;
                let ptr: WorkspacePointer = serde_json::from_slice(&bytes)?;
                return Repository::open_secondary(
                    ptr.common_dir,
                    dir.to_path_buf(),
                    ptr.workspace,
                );
            }
            cur = dir.parent();
        }
        Err(Error::NotARepository(start))
    }

    /// Open a repository whose working directory is exactly `work_dir`.
    pub fn open(work_dir: &Path) -> Result<Repository> {
        let repo = Repository::discover(work_dir)?;
        if repo.work_dir != absolutize(work_dir)? {
            return Err(Error::NotARepository(work_dir.to_path_buf()));
        }
        Ok(repo)
    }

    // -- accessors -----------------------------------------------------------

    pub fn common_dir(&self) -> &Path {
        &self.common_dir
    }
    pub fn work_dir(&self) -> &Path {
        &self.work_dir
    }
    pub fn workspace(&self) -> &str {
        &self.workspace
    }
    pub fn workspace_dir(&self) -> &Path {
        &self.ws_dir
    }
    pub fn store(&self) -> &ObjectStore {
        &self.store
    }
    pub fn refs(&self) -> &RefStore {
        &self.refs
    }
    pub fn events(&self) -> &EventLog {
        &self.events
    }
    pub fn leases(&self) -> &LeaseStore {
        &self.leases
    }
    pub fn index_path(&self) -> PathBuf {
        self.ws_dir.join("index")
    }
    pub fn head_path(&self) -> PathBuf {
        self.ws_dir.join("HEAD")
    }
    pub fn merge_head_path(&self) -> PathBuf {
        self.ws_dir.join("MERGE_HEAD")
    }

    pub fn load_index(&self) -> Result<Index> {
        Index::load(&self.index_path())
    }

    pub fn save_index(&self, index: &Index) -> Result<()> {
        index.save(&self.index_path())
    }

    // -- config --------------------------------------------------------------

    pub fn config(&self) -> Result<Config> {
        let p = self.common_dir.join("config.json");
        match fs::read(&p) {
            Ok(b) => Ok(serde_json::from_slice(&b)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(Error::IoAt { path: p, source: e }),
        }
    }

    pub fn save_config(&self, cfg: &Config) -> Result<()> {
        write_atomic(
            &self.common_dir.join("config.json"),
            &serde_json::to_vec_pretty(cfg)?,
        )
    }

    /// Resolve the author identity: env > config > OS user.
    pub fn author(&self) -> Result<Signature> {
        let cfg = self.config()?;
        let name = std::env::var("AGIT_AUTHOR_NAME")
            .ok()
            .or(cfg.user.name)
            .or_else(|| std::env::var("USER").ok())
            .unwrap_or_else(|| "unknown".into());
        let email = std::env::var("AGIT_AUTHOR_EMAIL")
            .ok()
            .or(cfg.user.email)
            .unwrap_or_else(|| {
                let host = hostname::get()
                    .ok()
                    .and_then(|h| h.into_string().ok())
                    .unwrap_or_else(|| "localhost".into());
                format!("{name}@{host}")
            });
        Ok(Signature { name, email })
    }

    // -- HEAD ----------------------------------------------------------------

    pub fn head(&self) -> Result<Head> {
        let p = self.head_path();
        let s = fs::read_to_string(&p).map_err(io_at(&p))?;
        let s = s.trim();
        if let Some(r) = s.strip_prefix("ref: ") {
            let r = r.trim();
            let name = r.strip_prefix(HEADS_PREFIX).unwrap_or(r);
            Ok(Head::Branch(name.to_string()))
        } else {
            Ok(Head::Detached(s.parse()?))
        }
    }

    pub fn set_head(&self, head: &Head) -> Result<()> {
        let content = match head {
            Head::Branch(b) => format!("ref: {HEADS_PREFIX}{b}\n"),
            Head::Detached(id) => format!("{id}\n"),
        };
        write_atomic(&self.head_path(), content.as_bytes())
    }

    /// Commit id HEAD points at (None on an unborn branch).
    pub fn head_commit(&self) -> Result<Option<ObjectId>> {
        match self.head()? {
            Head::Branch(b) => self.refs.read(&format!("{HEADS_PREFIX}{b}")),
            Head::Detached(id) => Ok(Some(id)),
        }
    }

    pub fn head_tree(&self) -> Result<Option<ObjectId>> {
        Ok(match self.head_commit()? {
            Some(c) => Some(self.store.read_commit(&c)?.tree),
            None => None,
        })
    }

    pub fn current_branch(&self) -> Result<Option<String>> {
        Ok(match self.head()? {
            Head::Branch(b) => Some(b),
            Head::Detached(_) => None,
        })
    }

    /// Move HEAD forward to `new` with CAS on the underlying branch.
    pub fn advance_head(&self, expected: Option<ObjectId>, new: ObjectId) -> Result<()> {
        match self.head()? {
            Head::Branch(b) => {
                let expect = match expected {
                    Some(id) => Expect::Is(id),
                    None => Expect::Absent,
                };
                self.refs
                    .update(&format!("{HEADS_PREFIX}{b}"), expect, Some(new))
            }
            Head::Detached(_) => self.set_head(&Head::Detached(new)),
        }
    }

    pub fn merge_head(&self) -> Result<Option<ObjectId>> {
        match fs::read_to_string(self.merge_head_path()) {
            Ok(s) => Ok(Some(s.trim().parse()?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn set_merge_head(&self, id: Option<ObjectId>) -> Result<()> {
        let p = self.merge_head_path();
        match id {
            Some(id) => write_atomic(&p, format!("{id}\n").as_bytes()),
            None => match fs::remove_file(&p) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(Error::IoAt { path: p, source: e }),
            },
        }
    }

    // -- revisions -----------------------------------------------------------

    /// Resolve a revision expression to a commit id.
    ///
    /// Supported: `HEAD`, `@`, branch, tag, `refs/...`, full or abbreviated
    /// ids, and `~N` / `^` / `^N` suffixes (any combination).
    pub fn resolve(&self, rev: &str) -> Result<ObjectId> {
        let rev = rev.trim();
        if rev.is_empty() {
            return Err(Error::UnknownRevision(rev.to_string()));
        }
        // Split off trailing navigation operators.
        let base_end = rev.find(['~', '^']).unwrap_or(rev.len());
        let (base, ops) = rev.split_at(base_end);
        let mut id = self.resolve_base(base)?;
        let mut chars = ops.chars().peekable();
        while let Some(op) = chars.next() {
            let mut num = String::new();
            while let Some(c) = chars.peek() {
                if c.is_ascii_digit() {
                    num.push(*c);
                    chars.next();
                } else {
                    break;
                }
            }
            match op {
                '~' => {
                    let n: usize = if num.is_empty() {
                        1
                    } else {
                        num.parse().unwrap_or(1)
                    };
                    for _ in 0..n {
                        id = self.first_parent(&id, rev)?;
                    }
                }
                '^' => {
                    let n: usize = if num.is_empty() {
                        1
                    } else {
                        num.parse().unwrap_or(1)
                    };
                    let c = self.store.read_commit(&id)?;
                    id = *c
                        .parents
                        .get(n - 1)
                        .ok_or_else(|| Error::UnknownRevision(rev.to_string()))?;
                }
                _ => unreachable!(),
            }
        }
        Ok(id)
    }

    fn first_parent(&self, id: &ObjectId, rev: &str) -> Result<ObjectId> {
        let c = self.store.read_commit(id)?;
        c.parents
            .first()
            .copied()
            .ok_or_else(|| Error::UnknownRevision(rev.to_string()))
    }

    fn resolve_base(&self, base: &str) -> Result<ObjectId> {
        if base == "HEAD" || base == "@" || base.is_empty() {
            return self
                .head_commit()?
                .ok_or_else(|| Error::UnknownRevision("HEAD (unborn branch)".into()));
        }
        if base.starts_with("refs/") {
            return self
                .refs
                .read(base)?
                .ok_or_else(|| Error::RefNotFound(base.to_string()));
        }
        if let Some(id) = self.refs.read(&format!("{HEADS_PREFIX}{base}"))? {
            return Ok(id);
        }
        if let Some(id) = self.refs.read(&format!("{TAGS_PREFIX}{base}"))? {
            return Ok(id);
        }
        if let Some(id) = self
            .refs
            .read(&format!("{}{base}", crate::refs::CHECKPOINTS_PREFIX))?
        {
            return Ok(id);
        }
        self.store.resolve_prefix(base)
    }

    /// Resolve a revision that must be a commit, returning (id, commit).
    pub fn resolve_commit(&self, rev: &str) -> Result<(ObjectId, crate::object::Commit)> {
        let id = self.resolve(rev)?;
        let c = self.store.read_commit(&id)?;
        Ok((id, c))
    }

    // -- workspaces ----------------------------------------------------------

    pub fn list_workspaces(&self) -> Result<Vec<WorkspaceMeta>> {
        let mut out = vec![WorkspaceMeta {
            name: PRIMARY_WORKSPACE.into(),
            path: self.primary_work_dir()?,
            created_ms: 0,
            agent: None,
        }];
        let dir = self.common_dir.join("workspaces");
        if dir.is_dir() {
            for e in crate::index::read_dir_sorted(&dir)? {
                let meta = e.path().join("meta.json");
                if let Ok(b) = fs::read(&meta) {
                    if let Ok(m) = serde_json::from_slice::<WorkspaceMeta>(&b) {
                        out.push(m);
                    }
                }
            }
        }
        Ok(out)
    }

    fn primary_work_dir(&self) -> Result<PathBuf> {
        Ok(self
            .common_dir
            .parent()
            .ok_or_else(|| Error::Invalid("common dir has no parent".into()))?
            .to_path_buf())
    }

    /// Create a secondary workspace at `path` (default: sibling directory
    /// `<repo>.<name>`), checked out at `start` (default: current HEAD).
    pub fn create_workspace(
        &self,
        name: &str,
        path: Option<&Path>,
        start: Option<ObjectId>,
        agent: Option<&str>,
    ) -> Result<(Repository, WorkspaceMeta)> {
        if name.is_empty() || name == PRIMARY_WORKSPACE || name.contains(['/', '\\', '.', ' ']) {
            return Err(Error::Invalid(format!("invalid workspace name {name:?}")));
        }
        let ws_dir = self.common_dir.join("workspaces").join(name);
        if ws_dir.exists() {
            return Err(Error::WorkspaceExists(name.to_string()));
        }
        let primary = self.primary_work_dir()?;
        let path = match path {
            Some(p) => absolutize(p)?,
            None => {
                let base = primary
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "repo".into());
                primary
                    .parent()
                    .unwrap_or(&primary)
                    .join(format!("{base}.{name}"))
            }
        };
        if path.exists()
            && fs::read_dir(&path)
                .map(|mut d| d.next().is_some())
                .unwrap_or(true)
        {
            return Err(Error::Invalid(format!(
                "workspace path {} exists and is not empty",
                path.display()
            )));
        }
        fs::create_dir_all(&path).map_err(io_at(&path))?;
        fs::create_dir_all(&ws_dir).map_err(io_at(&ws_dir))?;

        // Each workspace starts on its own branch so agents never fight over
        // one branch head by accident: refs/heads/ws/<name>.
        let branch = format!("ws/{name}");
        let start = match start {
            Some(id) => Some(id),
            None => self.head_commit()?,
        };
        if let Some(id) = start {
            self.refs
                .update(&format!("{HEADS_PREFIX}{branch}"), Expect::Absent, Some(id))?;
        }
        write_atomic(
            &ws_dir.join("HEAD"),
            format!("ref: {HEADS_PREFIX}{branch}\n").as_bytes(),
        )?;
        let meta = WorkspaceMeta {
            name: name.to_string(),
            path: path.clone(),
            created_ms: crate::events::now_ms(),
            agent: agent.map(str::to_string),
        };
        write_atomic(
            &ws_dir.join("meta.json"),
            &serde_json::to_vec_pretty(&meta)?,
        )?;
        let ptr = WorkspacePointer {
            common_dir: self.common_dir.clone(),
            workspace: name.to_string(),
        };
        write_atomic(&path.join(AGIT_DIR), &serde_json::to_vec_pretty(&ptr)?)?;

        let repo = Repository::open_secondary(self.common_dir.clone(), path, name.to_string())?;
        if let Some(id) = start {
            let tree = self.store.read_commit(&id)?.tree;
            crate::worktree::checkout_tree(&repo, None, &tree, true)?;
        }
        self.events.append(
            "workspace.create",
            Some(name),
            agent,
            serde_json::json!({ "path": meta.path, "branch": branch, "start": start }),
        )?;
        Ok((repo, meta))
    }

    /// Remove a secondary workspace's metadata (and optionally its directory).
    pub fn remove_workspace(&self, name: &str, delete_dir: bool) -> Result<WorkspaceMeta> {
        if name == PRIMARY_WORKSPACE {
            return Err(Error::Invalid("cannot remove the primary workspace".into()));
        }
        let ws_dir = self.common_dir.join("workspaces").join(name);
        let meta_path = ws_dir.join("meta.json");
        let meta: WorkspaceMeta = serde_json::from_slice(
            &fs::read(&meta_path).map_err(|_| Error::WorkspaceNotFound(name.to_string()))?,
        )?;
        fs::remove_dir_all(&ws_dir).map_err(io_at(&ws_dir))?;
        if delete_dir && meta.path.join(AGIT_DIR).is_file() {
            fs::remove_dir_all(&meta.path).map_err(io_at(&meta.path))?;
        }
        self.events.append(
            "workspace.remove",
            Some(name),
            None,
            serde_json::json!({ "path": meta.path, "deleted_dir": delete_dir }),
        )?;
        Ok(meta)
    }

    // -- paths ---------------------------------------------------------------

    /// Convert a user-supplied path (relative to cwd or absolute) into a
    /// repository-relative, `/`-separated path.
    pub fn rel_path(&self, cwd: &Path, user_path: &Path) -> Result<String> {
        let abs = if user_path.is_absolute() {
            user_path.to_path_buf()
        } else {
            cwd.join(user_path)
        };
        let abs = normalize_lexically(&abs);
        let rel = abs
            .strip_prefix(&self.work_dir)
            .map_err(|_| Error::PathOutsideWorktree(user_path.to_path_buf()))?;
        let s = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join("/");
        Ok(s)
    }

    pub fn abs_path(&self, rel: &str) -> PathBuf {
        if rel.is_empty() {
            self.work_dir.clone()
        } else {
            self.work_dir.join(rel)
        }
    }
}

pub fn absolutize(p: &Path) -> Result<PathBuf> {
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()?.join(p)
    };
    // Resolve symlinks when possible so discovery and rel_path agree.
    Ok(match abs.canonicalize() {
        Ok(c) => c,
        Err(_) => normalize_lexically(&abs),
    })
}

pub fn normalize_lexically(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}
