//! Submodule commands: add, update, status.

use std::path::Path;

use agit_core::refs::Expect;
use agit_core::submodule::{self, Submodule, SubmoduleStatus};
use agit_core::{Error, Head, ObjectId, Repository, Result};
use serde::{Deserialize, Serialize};

use crate::sync;

/// Resolve a submodule URL: relative local paths are relative to the parent
/// repository's working directory, not the process cwd.
fn resolve_url(repo: &Repository, url: &str) -> String {
    if sync::is_local_url(url) {
        let p = Path::new(url.strip_prefix("file://").unwrap_or(url));
        if p.is_relative() {
            return repo.work_dir().join(p).display().to_string();
        }
    }
    url.to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmoduleUpdate {
    pub path: String,
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<ObjectId>,
}

/// Clone `url` into `path`, register it in `.agitmodules` and stage the
/// submodule entry (pinned to the clone's HEAD) plus `.agitmodules`.
pub fn add(repo: &Repository, url: &str, path: &str, branch: Option<&str>) -> Result<Submodule> {
    let path = path.trim_matches('/').to_string();
    if path.is_empty() || path.contains("..") {
        return Err(Error::Invalid(format!("invalid submodule path {path:?}")));
    }
    let mut modules = submodule::read_modules(repo)?;
    if modules.iter().any(|m| m.path == path) {
        return Err(Error::AlreadyExists(path.into()));
    }
    let abs = repo.abs_path(&path);
    if abs.exists()
        && std::fs::read_dir(&abs)
            .map(|mut d| d.next().is_some())
            .unwrap_or(true)
    {
        return Err(Error::Invalid(format!("{path} exists and is not empty")));
    }
    let (nested, _) = sync::clone(&resolve_url(repo, url), &abs)?;
    if let Some(b) = branch {
        if nested.current_branch()?.as_deref() != Some(b) {
            agit_core::ops::switch(&nested, b, false, true)?;
        }
    }
    let m = Submodule {
        path: path.clone(),
        url: url.to_string(),
        branch: branch.map(str::to_string),
    };
    modules.push(m.clone());
    submodule::write_modules(repo, &modules)?;
    agit_core::worktree::add(
        repo,
        &[submodule::MODULES_FILE.to_string(), path.clone()],
        true,
    )?;
    repo.events().append(
        "submodule.add",
        Some(repo.workspace()),
        None,
        serde_json::json!({ "path": path, "url": url, "commit": nested.head_commit()? }),
    )?;
    Ok(m)
}

/// Make every registered submodule (or the given paths) match the pinned
/// commit: clone when missing, fetch when the commit is unknown, then check
/// out the pinned commit (detached).
pub fn update(repo: &Repository, paths: Option<&[String]>) -> Result<Vec<SubmoduleUpdate>> {
    let index = repo.load_index()?;
    let mut out = Vec::new();
    for m in submodule::read_modules(repo)? {
        if paths.is_some_and(|ps| !ps.iter().any(|p| p.trim_matches('/') == m.path)) {
            continue;
        }
        let pinned = index
            .get(&m.path)
            .filter(|e| e.mode.is_submodule())
            .map(|e| e.id);
        let abs = repo.abs_path(&m.path);
        let nested = match submodule::open_nested(repo, &m.path)? {
            Some(r) => r,
            None => {
                let (r, _) = sync::clone(&resolve_url(repo, &m.url), &abs)?;
                out.push(SubmoduleUpdate {
                    path: m.path.clone(),
                    action: "cloned".into(),
                    commit: r.head_commit()?,
                });
                r
            }
        };
        let Some(pinned) = pinned else {
            out.push(SubmoduleUpdate {
                path: m.path.clone(),
                action: "unpinned".into(),
                commit: nested.head_commit()?,
            });
            continue;
        };
        if nested.head_commit()? == Some(pinned) {
            out.push(SubmoduleUpdate {
                path: m.path.clone(),
                action: "up_to_date".into(),
                commit: Some(pinned),
            });
            continue;
        }
        if !nested.store().contains(&pinned) {
            if sync::is_local_url(&m.url) {
                return Err(Error::ObjectNotFound(pinned));
            }
            sync::fetch(&nested, "origin", None)?;
            if !nested.store().contains(&pinned) {
                return Err(Error::ObjectNotFound(pinned));
            }
        }
        checkout_detached(&nested, pinned)?;
        out.push(SubmoduleUpdate {
            path: m.path.clone(),
            action: "checked_out".into(),
            commit: Some(pinned),
        });
    }
    Ok(out)
}

fn checkout_detached(nested: &Repository, id: ObjectId) -> Result<()> {
    let from = nested.head_tree()?;
    let to = nested.store().read_commit(&id)?.tree;
    agit_core::worktree::checkout_tree(nested, from.as_ref(), &to, false)?;
    nested.set_head(&Head::Detached(id))?;
    let _ = nested.refs().update(
        &format!("refs/checkpoints/{}", nested.workspace()),
        Expect::Any,
        None,
    );
    Ok(())
}

pub fn status(repo: &Repository) -> Result<Vec<SubmoduleStatus>> {
    submodule::status(repo)
}

/// Remove a submodule registration and its index entry (keeps the directory
/// unless `delete_dir`).
pub fn remove(repo: &Repository, path: &str, delete_dir: bool) -> Result<()> {
    let path = path.trim_matches('/');
    let mut modules = submodule::read_modules(repo)?;
    let before = modules.len();
    modules.retain(|m| m.path != path);
    if modules.len() == before {
        return Err(Error::Invalid(format!("no submodule at {path}")));
    }
    submodule::write_modules(repo, &modules)?;
    let mut index = repo.load_index()?;
    index.remove(path);
    repo.save_index(&index)?;
    agit_core::worktree::add(repo, &[submodule::MODULES_FILE.to_string()], true)?;
    if delete_dir {
        let abs = repo.abs_path(path);
        if abs.join(agit_core::repo::AGIT_DIR).exists() {
            std::fs::remove_dir_all(&abs)?;
        }
    }
    Ok(())
}

pub fn exists_at(dir: &Path) -> bool {
    dir.join(agit_core::repo::AGIT_DIR).exists()
}
