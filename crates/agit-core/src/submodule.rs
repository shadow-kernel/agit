//! Submodules: linked repositories pinned to a commit.
//!
//! `.agitmodules` (TOML) maps a path to a URL and branch:
//!
//! ```toml
//! [submodule."libs/core"]
//! path = "libs/core"
//! url = "http://host:7878/api/v1/repos/core"
//! branch = "main"
//! ```
//!
//! The tree entry at `path` has mode `Submodule` and its id is the pinned
//! commit of the linked repository. The working tree holds a full nested
//! agit repository at that path; the outer walker never descends into it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{io_at, Error, Result};
use crate::hash::ObjectId;
use crate::repo::Repository;

pub const MODULES_FILE: &str = ".agitmodules";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Submodule {
    pub path: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ModulesFile {
    #[serde(default)]
    submodule: BTreeMap<String, Submodule>,
}

/// Read `.agitmodules` from the working tree (empty when absent).
pub fn read_modules(repo: &Repository) -> Result<Vec<Submodule>> {
    let p = repo.work_dir().join(MODULES_FILE);
    let text = match std::fs::read_to_string(&p) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(Error::IoAt { path: p, source: e }),
    };
    let f: ModulesFile =
        toml::from_str(&text).map_err(|e| Error::Invalid(format!("bad {MODULES_FILE}: {e}")))?;
    let mut v: Vec<Submodule> = f.submodule.into_values().collect();
    v.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(v)
}

pub fn write_modules(repo: &Repository, modules: &[Submodule]) -> Result<()> {
    let f = ModulesFile {
        submodule: modules
            .iter()
            .map(|m| (m.path.clone(), m.clone()))
            .collect(),
    };
    let text = toml::to_string_pretty(&f).map_err(|e| Error::Invalid(e.to_string()))?;
    let p = repo.work_dir().join(MODULES_FILE);
    std::fs::write(&p, text).map_err(io_at(&p))
}

pub fn find(repo: &Repository, path: &str) -> Result<Option<Submodule>> {
    Ok(read_modules(repo)?.into_iter().find(|m| m.path == path))
}

/// Open the nested repository at a submodule path, if it is checked out.
pub fn open_nested(repo: &Repository, path: &str) -> Result<Option<Repository>> {
    let abs = repo.abs_path(path);
    if !abs.join(crate::repo::AGIT_DIR).exists() {
        return Ok(None);
    }
    Ok(Some(Repository::open(&abs)?))
}

/// HEAD commit of a checked-out submodule.
pub fn nested_head(repo: &Repository, path: &str) -> Result<Option<ObjectId>> {
    match open_nested(repo, path)? {
        Some(r) => r.head_commit(),
        None => Ok(None),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmoduleStatus {
    pub path: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<ObjectId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual: Option<ObjectId>,
    /// `ok`, `modified` (checked out at another commit), `missing` (not
    /// checked out), `unpinned` (registered but not yet staged).
    pub state: String,
}

pub fn status(repo: &Repository) -> Result<Vec<SubmoduleStatus>> {
    let index = repo.load_index()?;
    let mut out = Vec::new();
    for m in read_modules(repo)? {
        let pinned = index
            .get(&m.path)
            .filter(|e| e.mode.is_submodule())
            .map(|e| e.id);
        let actual = nested_head(repo, &m.path)?;
        let state = match (pinned, actual) {
            (_, None) => "missing",
            (None, Some(_)) => "unpinned",
            (Some(p), Some(a)) if p == a => "ok",
            _ => "modified",
        };
        out.push(SubmoduleStatus {
            path: m.path,
            url: m.url,
            pinned,
            actual,
            state: state.into(),
        });
    }
    Ok(out)
}
