//! High-level operations. These are what the CLI (and later the server and
//! SDKs) call; each returns a serializable result so `--json` is trivial.

use std::collections::{BTreeMap, BinaryHeap, HashSet};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::diff::{diff_trees, Change};
use crate::error::{Error, Result};
use crate::events::now_ms;
use crate::hash::ObjectId;
use crate::index::IndexEntry;
use crate::lease::LeaseStore;
use crate::merge::{is_ancestor, merge_base, merge_trees, Conflict, MergeLabels};
use crate::object::{AgentInfo, Commit, Signature};
use crate::refs::{Expect, CHECKPOINTS_PREFIX, HEADS_PREFIX, TAGS_PREFIX};
use crate::repo::{Head, Repository};
use crate::worktree;

// ---------------------------------------------------------------------------
// Commit
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct CommitOptions {
    pub message: String,
    pub intent: Option<String>,
    pub task: Option<String>,
    pub agent: Option<AgentInfo>,
    pub meta: BTreeMap<String, serde_json::Value>,
    pub author: Option<Signature>,
    pub allow_empty: bool,
    /// Skip lease enforcement.
    pub force: bool,
    /// Stage all tracked modifications first (`-a`).
    pub all: bool,
    /// Sign with this key name from `~/.agit/keys` (see [`crate::sign`]).
    pub sign_with: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitResult {
    pub id: ObjectId,
    pub tree: ObjectId,
    pub parents: Vec<ObjectId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub changes: Vec<Change>,
    pub summary: String,
}

fn actor_of(opts_agent: Option<&AgentInfo>, author: &Signature) -> String {
    opts_agent
        .map(|a| a.id.clone())
        .unwrap_or_else(|| author.name.clone())
}

/// Refuse when any of `paths` is under another holder's exclusive lease.
pub fn enforce_leases(leases: &LeaseStore, holder: &str, paths: &[String]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    if let Some((lease, path)) = leases.find_conflict(holder, paths)? {
        return Err(Error::LeaseConflict {
            path,
            holder: lease.holder,
            lease_id: lease.id,
        });
    }
    Ok(())
}

pub fn commit(repo: &Repository, opts: CommitOptions) -> Result<CommitResult> {
    if opts.all {
        let st = worktree::status(repo, false)?;
        let paths: Vec<String> = st.unstaged.iter().map(|c| c.path.clone()).collect();
        if !paths.is_empty() {
            worktree::add(repo, &paths, true)?;
        }
    }
    let mut index = repo.load_index()?;
    if index.has_conflicts() {
        return Err(Error::Invalid(format!(
            "unresolved conflicts: {}",
            index.conflicted_paths().join(", ")
        )));
    }
    let head = repo.head_commit()?;
    let head_tree = repo.head_tree()?;
    let merge_head = repo.merge_head()?;
    let cached_before = index.cached_trees();
    let tree = index.write_tree_cached(repo.store())?;
    if index.cached_trees() != cached_before {
        // Persist the refreshed tree cache; best effort (it is only a cache).
        let _ = repo.save_index(&index);
    }
    let changes = diff_trees(repo.store(), head_tree.as_ref(), Some(&tree))?;
    if changes.is_empty() && merge_head.is_none() && !opts.allow_empty {
        return Err(Error::NothingToCommit);
    }
    let author = match opts.author {
        Some(a) => a,
        None => repo.author()?,
    };
    let actor = actor_of(opts.agent.as_ref(), &author);
    if !opts.force {
        let paths: Vec<String> = changes.iter().map(|c| c.path.clone()).collect();
        enforce_leases(repo.leases(), &actor, &paths)?;
    }
    let mut parents = Vec::new();
    if let Some(h) = head {
        parents.push(h);
    }
    if let Some(m) = merge_head {
        parents.push(m);
    }
    let mut commit = Commit {
        tree,
        parents: parents.clone(),
        author,
        time_ms: now_ms(),
        message: opts.message,
        intent: opts.intent,
        task: opts.task,
        agent: opts.agent,
        checkpoint: false,
        meta: opts.meta,
        signature: None,
    };
    if let Some(key) = &opts.sign_with {
        let kp = crate::sign::Keypair::load(key)?;
        commit.signature = Some(kp.sign_commit(&commit, &actor));
    }
    let id = repo.store().write_commit(&commit)?;
    repo.advance_head_with(head, id, &format!("commit: {}", commit.summary()))?;
    repo.set_merge_head(None)?;
    let branch = repo.current_branch()?;
    repo.events().append(
        "commit",
        Some(repo.workspace()),
        Some(&actor),
        serde_json::json!({
            "id": id,
            "branch": branch,
            "parents": parents,
            "summary": commit.summary(),
            "intent": commit.intent,
            "task": commit.task,
            "files": changes.len(),
        }),
    )?;
    Ok(CommitResult {
        id,
        tree,
        parents,
        branch,
        changes,
        summary: commit.summary().to_string(),
    })
}

// ---------------------------------------------------------------------------
// Checkpoints
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointResult {
    pub id: ObjectId,
    pub tree: ObjectId,
    pub previous: Option<ObjectId>,
    pub changes_vs_head: usize,
    /// False when `if_changed` found nothing new.
    #[serde(default = "default_true")]
    pub created: bool,
}

fn default_true() -> bool {
    true
}

/// Snapshot the whole working tree (tracked and untracked) into a checkpoint
/// commit on `refs/checkpoints/<workspace>`. HEAD and the index are untouched.
pub fn checkpoint(
    repo: &Repository,
    message: Option<String>,
    agent: Option<AgentInfo>,
) -> Result<CheckpointResult> {
    checkpoint_opts(repo, message, agent, false)
}

/// Like [`checkpoint`]; with `if_changed` an unchanged tree (equal to the
/// last checkpoint, or to HEAD when there is none) returns the existing
/// checkpoint instead of creating a duplicate (`created == false`).
pub fn checkpoint_opts(
    repo: &Repository,
    message: Option<String>,
    agent: Option<AgentInfo>,
    if_changed: bool,
) -> Result<CheckpointResult> {
    let tree = worktree::snapshot_worktree(repo)?;
    let head = repo.head_commit()?;
    let ref_name = format!("{CHECKPOINTS_PREFIX}{}", repo.workspace());
    let previous = repo.refs().read(&ref_name)?;
    if if_changed {
        let last_tree = match previous {
            Some(p) => Some(repo.store().read_commit(&p)?.tree),
            None => repo.head_tree()?,
        };
        if last_tree == Some(tree) {
            return Ok(CheckpointResult {
                id: previous.unwrap_or(ObjectId::ZERO),
                tree,
                previous,
                changes_vs_head: 0,
                created: false,
            });
        }
    }
    let author = repo.author()?;
    let actor = actor_of(agent.as_ref(), &author);
    let mut meta = BTreeMap::new();
    if let Some(p) = previous {
        meta.insert("agit.previous_checkpoint".into(), serde_json::json!(p));
    }
    let changes_vs_head = diff_trees(repo.store(), repo.head_tree()?.as_ref(), Some(&tree))?.len();
    let commit = Commit {
        tree,
        parents: head.into_iter().collect(),
        author,
        time_ms: now_ms(),
        message: message.unwrap_or_else(|| "checkpoint".into()),
        intent: None,
        task: None,
        agent,
        checkpoint: true,
        meta,
        signature: None,
    };
    let id = repo.store().write_commit(&commit)?;
    repo.refs().update(&ref_name, Expect::Any, Some(id))?;
    repo.events().append(
        "checkpoint",
        Some(repo.workspace()),
        Some(&actor),
        serde_json::json!({ "id": id, "previous": previous, "files_changed": changes_vs_head }),
    )?;
    Ok(CheckpointResult {
        id,
        tree,
        previous,
        changes_vs_head,
        created: true,
    })
}

/// List checkpoints of this workspace, newest first.
pub fn list_checkpoints(repo: &Repository, limit: usize) -> Result<Vec<(ObjectId, Commit)>> {
    let ref_name = format!("{CHECKPOINTS_PREFIX}{}", repo.workspace());
    let mut cur = repo.refs().read(&ref_name)?;
    let mut out = Vec::new();
    while let Some(id) = cur {
        if out.len() >= limit {
            break;
        }
        let c = repo.store().read_commit(&id)?;
        cur = c
            .meta
            .get("agit.previous_checkpoint")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok());
        out.push((id, c));
    }
    Ok(out)
}

/// Restore the working tree to a checkpoint (or any commit) tree. Untracked
/// files that the checkpoint knows about are restored too.
pub fn restore(repo: &Repository, rev: &str) -> Result<Vec<Change>> {
    let (id, c) = repo.resolve_commit(rev)?;
    let current = worktree::snapshot_worktree(repo)?;
    let changes = worktree::checkout_tree(repo, Some(&current), &c.tree, true)?;
    // Keep the index aligned with HEAD semantics: entries now reflect the
    // restored tree, which is what the user sees on disk.
    repo.events().append(
        "restore",
        Some(repo.workspace()),
        None,
        serde_json::json!({ "to": id, "files": changes.len() }),
    )?;
    Ok(changes)
}

// ---------------------------------------------------------------------------
// Log
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub id: ObjectId,
    #[serde(flatten)]
    pub commit: Commit,
}

#[derive(Debug, Clone, Default)]
pub struct LogOptions {
    pub limit: Option<usize>,
    pub agent: Option<String>,
    pub task: Option<String>,
    pub path: Option<String>,
    pub first_parent: bool,
}

#[derive(PartialEq, Eq)]
struct ByTime(i64, ObjectId);
impl Ord for ByTime {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        self.0.cmp(&o.0).then_with(|| self.1.cmp(&o.1))
    }
}
impl PartialOrd for ByTime {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}

pub fn log(repo: &Repository, start: &[ObjectId], opts: &LogOptions) -> Result<Vec<LogEntry>> {
    log_store(repo.store(), start, opts)
}

/// Same as [`log`] but for a bare object store (server side).
pub fn log_store(
    store: &crate::store::ObjectStore,
    start: &[ObjectId],
    opts: &LogOptions,
) -> Result<Vec<LogEntry>> {
    let mut heap = BinaryHeap::new();
    let mut seen = HashSet::new();
    for s in start {
        if seen.insert(*s) {
            let c = store.read_commit(s)?;
            heap.push(ByTime(c.time_ms, *s));
        }
    }
    let mut out = Vec::new();
    while let Some(ByTime(_, id)) = heap.pop() {
        let c = store.read_commit(&id)?;
        let parents: Vec<ObjectId> = if opts.first_parent {
            c.parents.iter().take(1).copied().collect()
        } else {
            c.parents.clone()
        };
        for p in parents {
            if seen.insert(p) {
                let pc = store.read_commit(&p)?;
                heap.push(ByTime(pc.time_ms, p));
            }
        }
        let mut keep = true;
        if let Some(a) = &opts.agent {
            keep &= c.agent.as_ref().is_some_and(|x| &x.id == a);
        }
        if let Some(t) = &opts.task {
            keep &= c.task.as_deref() == Some(t.as_str());
        }
        if let Some(p) = &opts.path {
            let parent_tree = match c.parents.first() {
                Some(pid) => Some(store.read_commit(pid)?.tree),
                None => None,
            };
            let changes = diff_trees(store, parent_tree.as_ref(), Some(&c.tree))?;
            keep &= changes
                .iter()
                .any(|ch| ch.path == *p || ch.path.starts_with(&format!("{p}/")));
        }
        if keep {
            out.push(LogEntry { id, commit: c });
            if opts.limit.is_some_and(|l| out.len() >= l) {
                break;
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Branches, tags
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchInfo {
    pub name: String,
    pub id: ObjectId,
    pub current: bool,
}

pub fn list_branches(repo: &Repository) -> Result<Vec<BranchInfo>> {
    let current = repo.current_branch()?;
    Ok(repo
        .refs()
        .list(HEADS_PREFIX)?
        .into_iter()
        .map(|(n, id)| {
            let name = n[HEADS_PREFIX.len()..].to_string();
            BranchInfo {
                current: current.as_deref() == Some(name.as_str()),
                name,
                id,
            }
        })
        .collect())
}

pub fn create_branch(repo: &Repository, name: &str, start: Option<&str>) -> Result<BranchInfo> {
    crate::refs::RefStore::validate_branch_name(name)?;
    let id = match start {
        Some(r) => repo.resolve(r)?,
        None => repo
            .head_commit()?
            .ok_or_else(|| Error::UnknownRevision("HEAD (unborn branch)".into()))?,
    };
    repo.refs().update_with(
        &format!("{HEADS_PREFIX}{name}"),
        Expect::Absent,
        Some(id),
        "branch: created",
    )?;
    repo.events().append(
        "ref.update",
        Some(repo.workspace()),
        None,
        serde_json::json!({ "ref": format!("{HEADS_PREFIX}{name}"), "old": null, "new": id, "op": "create" }),
    )?;
    Ok(BranchInfo {
        name: name.to_string(),
        id,
        current: false,
    })
}

pub fn delete_branch(repo: &Repository, name: &str, force: bool) -> Result<ObjectId> {
    let full = format!("{HEADS_PREFIX}{name}");
    if repo.current_branch()?.as_deref() == Some(name) {
        return Err(Error::Invalid(format!(
            "cannot delete the current branch {name}"
        )));
    }
    let id = repo
        .refs()
        .read(&full)?
        .ok_or_else(|| Error::RefNotFound(full.clone()))?;
    if !force {
        if let Some(head) = repo.head_commit()? {
            if !is_ancestor(repo.store(), &id, &head)? {
                return Err(Error::Invalid(format!(
                    "branch {name} is not merged into HEAD; use --force to delete"
                )));
            }
        }
    }
    repo.refs().update(&full, Expect::Is(id), None)?;
    repo.events().append(
        "ref.update",
        Some(repo.workspace()),
        None,
        serde_json::json!({ "ref": full, "old": id, "new": null, "op": "delete" }),
    )?;
    Ok(id)
}

pub fn list_tags(repo: &Repository) -> Result<Vec<(String, ObjectId)>> {
    Ok(repo
        .refs()
        .list(TAGS_PREFIX)?
        .into_iter()
        .map(|(n, id)| (n[TAGS_PREFIX.len()..].to_string(), id))
        .collect())
}

pub fn create_tag(repo: &Repository, name: &str, rev: Option<&str>) -> Result<ObjectId> {
    crate::refs::RefStore::validate_name(&format!("{TAGS_PREFIX}{name}"))?;
    let id = repo.resolve(rev.unwrap_or("HEAD"))?;
    repo.refs()
        .update(&format!("{TAGS_PREFIX}{name}"), Expect::Absent, Some(id))?;
    repo.events().append(
        "ref.update",
        Some(repo.workspace()),
        None,
        serde_json::json!({ "ref": format!("{TAGS_PREFIX}{name}"), "old": null, "new": id, "op": "create" }),
    )?;
    Ok(id)
}

pub fn delete_tag(repo: &Repository, name: &str) -> Result<ObjectId> {
    let full = format!("{TAGS_PREFIX}{name}");
    let id = repo
        .refs()
        .read(&full)?
        .ok_or_else(|| Error::RefNotFound(full.clone()))?;
    repo.refs().update(&full, Expect::Is(id), None)?;
    Ok(id)
}

// ---------------------------------------------------------------------------
// Switch / checkout
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwitchResult {
    pub head: Head2,
    pub id: ObjectId,
    pub files_changed: usize,
}

/// Serializable mirror of [`Head`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Head2 {
    Branch { name: String },
    Detached,
}

/// Switch to a branch (creating it when `create` is set) or detach at a
/// revision.
pub fn switch(repo: &Repository, target: &str, create: bool, force: bool) -> Result<SwitchResult> {
    if repo.merge_head()?.is_some() && !force {
        return Err(Error::Invalid(
            "merge in progress; commit or abort it first".into(),
        ));
    }
    let branch_ref = format!("{HEADS_PREFIX}{target}");
    let (new_head, id) = if create {
        let info = create_branch(repo, target, None)?;
        (Head::Branch(target.to_string()), info.id)
    } else if let Some(id) = repo.refs().read(&branch_ref)? {
        (Head::Branch(target.to_string()), id)
    } else {
        let id = repo.resolve(target)?;
        (Head::Detached(id), id)
    };
    let from = repo.head_tree()?;
    let to = repo.store().read_commit(&id)?.tree;
    let changes = worktree::checkout_tree(repo, from.as_ref(), &to, force)?;
    repo.set_head_with(&new_head, &format!("switch: {target}"))?;
    repo.events().append(
        "head.switch",
        Some(repo.workspace()),
        None,
        serde_json::json!({ "target": target, "id": id, "files": changes.len() }),
    )?;
    Ok(SwitchResult {
        head: match &new_head {
            Head::Branch(n) => Head2::Branch { name: n.clone() },
            Head::Detached(_) => Head2::Detached,
        },
        id,
        files_changed: changes.len(),
    })
}

// ---------------------------------------------------------------------------
// Merge
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MergeResult {
    AlreadyUpToDate {
        head: ObjectId,
    },
    FastForward {
        from: Option<ObjectId>,
        to: ObjectId,
        files_changed: usize,
    },
    Merged {
        commit: ObjectId,
        tree: ObjectId,
        auto_merged: Vec<String>,
    },
    Conflicts {
        tree: ObjectId,
        conflicts: Vec<Conflict>,
        auto_merged: Vec<String>,
    },
}

#[derive(Debug, Clone, Default)]
pub struct MergeOptions {
    pub message: Option<String>,
    pub intent: Option<String>,
    pub agent: Option<AgentInfo>,
    pub no_ff: bool,
    pub force: bool,
    pub sign_with: Option<String>,
}

pub fn merge(repo: &Repository, rev: &str, opts: MergeOptions) -> Result<MergeResult> {
    let store = repo.store();
    let theirs = repo.resolve(rev)?;
    let ours = repo.head_commit()?;

    let st = worktree::status(repo, false)?;
    if !st.is_clean() && !opts.force {
        return Err(Error::DirtyWorktree(
            st.staged
                .iter()
                .chain(st.unstaged.iter())
                .map(|c| std::path::PathBuf::from(&c.path))
                .collect(),
        ));
    }

    let Some(ours_id) = ours else {
        // Unborn branch: fast-forward to theirs.
        let to_tree = store.read_commit(&theirs)?.tree;
        let changes = worktree::checkout_tree(repo, None, &to_tree, opts.force)?;
        repo.advance_head(None, theirs)?;
        return Ok(MergeResult::FastForward {
            from: None,
            to: theirs,
            files_changed: changes.len(),
        });
    };

    if ours_id == theirs || is_ancestor(store, &theirs, &ours_id)? {
        return Ok(MergeResult::AlreadyUpToDate { head: ours_id });
    }
    let base = merge_base(store, &ours_id, &theirs)?;
    let ours_tree = store.read_commit(&ours_id)?.tree;
    let theirs_tree = store.read_commit(&theirs)?.tree;

    if base == Some(ours_id) && !opts.no_ff {
        let changes = worktree::checkout_tree(repo, Some(&ours_tree), &theirs_tree, opts.force)?;
        repo.advance_head_with(Some(ours_id), theirs, &format!("merge {rev}: fast-forward"))?;
        repo.events().append(
            "merge",
            Some(repo.workspace()),
            opts.agent.as_ref().map(|a| a.id.as_str()),
            serde_json::json!({ "kind": "fast_forward", "from": ours_id, "to": theirs }),
        )?;
        return Ok(MergeResult::FastForward {
            from: Some(ours_id),
            to: theirs,
            files_changed: changes.len(),
        });
    }

    let labels = MergeLabels {
        ours: repo.current_branch()?.unwrap_or_else(|| "HEAD".into()),
        theirs: rev.to_string(),
    };
    let base_tree = match base {
        Some(b) => Some(store.read_commit(&b)?.tree),
        None => None,
    };
    let outcome = merge_trees(store, base_tree.as_ref(), &ours_tree, &theirs_tree, &labels)?;

    // Lease enforcement on the paths the merge brings in.
    let author = repo.author()?;
    let actor = actor_of(opts.agent.as_ref(), &author);
    if !opts.force {
        let paths: Vec<String> = diff_trees(store, Some(&ours_tree), Some(&outcome.tree))?
            .into_iter()
            .map(|c| c.path)
            .collect();
        enforce_leases(repo.leases(), &actor, &paths)?;
    }

    // Materialize the merged tree in the working tree + index.
    worktree::checkout_tree(repo, Some(&ours_tree), &outcome.tree, true)?;

    if !outcome.is_clean() {
        let mut index = repo.load_index()?;
        for c in &outcome.conflicts {
            if let Some(e) = index.get(&c.path).cloned() {
                index.insert(IndexEntry {
                    conflict: true,
                    ..e
                });
            } else {
                // delete/modify where the surviving side is absent: record a
                // placeholder so `status` reports the conflict.
                index.insert(IndexEntry {
                    path: c.path.clone(),
                    id: ObjectId::ZERO,
                    mode: crate::object::EntryMode::File,
                    size: 0,
                    mtime_ns: 0,
                    ino: 0,
                    conflict: true,
                });
            }
        }
        repo.save_index(&index)?;
        repo.set_merge_head(Some(theirs))?;
        repo.events().append(
            "merge",
            Some(repo.workspace()),
            Some(&actor),
            serde_json::json!({ "kind": "conflicts", "ours": ours_id, "theirs": theirs, "conflicts": outcome.conflicts.len() }),
        )?;
        return Ok(MergeResult::Conflicts {
            tree: outcome.tree,
            conflicts: outcome.conflicts,
            auto_merged: outcome.auto_merged,
        });
    }

    let message = opts
        .message
        .unwrap_or_else(|| format!("Merge {rev} into {}", labels.ours));
    let mut commit = Commit {
        tree: outcome.tree,
        parents: vec![ours_id, theirs],
        author,
        time_ms: now_ms(),
        message,
        intent: opts.intent,
        task: None,
        agent: opts.agent,
        checkpoint: false,
        meta: BTreeMap::new(),
        signature: None,
    };
    if let Some(key) = &opts.sign_with {
        let kp = crate::sign::Keypair::load(key)?;
        commit.signature = Some(kp.sign_commit(&commit, &actor));
    }
    let id = store.write_commit(&commit)?;
    repo.advance_head_with(Some(ours_id), id, &format!("merge {rev}"))?;
    repo.events().append(
        "merge",
        Some(repo.workspace()),
        Some(&actor),
        serde_json::json!({ "kind": "merged", "commit": id, "ours": ours_id, "theirs": theirs, "auto_merged": outcome.auto_merged.len() }),
    )?;
    Ok(MergeResult::Merged {
        commit: id,
        tree: outcome.tree,
        auto_merged: outcome.auto_merged,
    })
}

/// Abort an in-progress merge: restore HEAD's tree and clear MERGE_HEAD.
pub fn merge_abort(repo: &Repository) -> Result<()> {
    if repo.merge_head()?.is_none() {
        return Err(Error::Invalid("no merge in progress".into()));
    }
    let head_tree = repo
        .head_tree()?
        .ok_or_else(|| Error::Invalid("no HEAD tree".into()))?;
    let current = worktree::snapshot_worktree(repo)?;
    worktree::checkout_tree(repo, Some(&current), &head_tree, true)?;
    repo.set_merge_head(None)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// fsck
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FsckReport {
    pub objects: usize,
    pub corrupt: Vec<String>,
    pub missing: Vec<String>,
    pub dangling_refs: Vec<String>,
}

pub fn fsck(repo: &Repository) -> Result<FsckReport> {
    let store = repo.store();
    let mut report = FsckReport::default();
    let ids = store.iter_ids()?;
    report.objects = ids.len();
    let mut present = HashSet::new();
    for id in &ids {
        match store.read(id) {
            Ok(_) => {
                present.insert(*id);
            }
            Err(e) => report.corrupt.push(format!("{id}: {e}")),
        }
    }
    // Reachability from all refs.
    let mut roots = Vec::new();
    for (name, id) in repo.refs().list("refs/")? {
        if !present.contains(&id) {
            report.dangling_refs.push(format!("{name} -> {id}"));
        } else {
            roots.push(id);
        }
    }
    let mut seen = HashSet::new();
    let mut stack = roots;
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        match store.read(&id) {
            Ok(crate::object::Object::Commit(c)) => {
                stack.push(c.tree);
                stack.extend(c.parents.iter().copied());
            }
            Ok(crate::object::Object::Tree(t)) => {
                for e in t.entries() {
                    if !e.mode.is_submodule() {
                        stack.push(e.id);
                    }
                }
            }
            Ok(_) => {}
            Err(Error::ObjectNotFound(_)) => report.missing.push(id.to_hex()),
            Err(e) => report.corrupt.push(format!("{id}: {e}")),
        }
    }
    Ok(report)
}

/// Convenience for callers that want a short-lived lease around a commit.
pub fn lease_ttl(secs: u64) -> Duration {
    Duration::from_secs(secs)
}

// ---------------------------------------------------------------------------
// Reset
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResetMode {
    /// Move the branch only; index and working tree stay.
    Soft,
    /// Move the branch and reset the index; working tree stays.
    Mixed,
    /// Move the branch, index and working tree.
    Hard,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResetResult {
    pub from: Option<ObjectId>,
    pub to: ObjectId,
    pub mode: ResetMode,
    pub files_changed: usize,
}

/// Move the current branch (or detached HEAD) to `rev`. The previous position
/// stays in the reflog (`agit reflog`), so a reset is always undoable with
/// another reset.
pub fn reset(repo: &Repository, rev: &str, mode: ResetMode) -> Result<ResetResult> {
    let (to, commit) = repo.resolve_commit(rev)?;
    let from = repo.head_commit()?;
    let mut files_changed = 0;
    match mode {
        ResetMode::Soft => {}
        ResetMode::Mixed => {
            let mut index = crate::index::Index::from_tree(repo.store(), &commit.tree)?;
            // keep stat info for files that are unchanged on disk
            let st = worktree::status(repo, false)?;
            let _ = st;
            if let Ok(old) = repo.load_index() {
                for e in old.iter() {
                    if let Some(n) = index.get(&e.path).cloned() {
                        if n.id == e.id && n.mode == e.mode {
                            index.insert(IndexEntry {
                                mtime_ns: e.mtime_ns,
                                ino: e.ino,
                                ..n
                            });
                        }
                    }
                }
            }
            repo.save_index(&index)?;
        }
        ResetMode::Hard => {
            let current = worktree::snapshot_worktree(repo)?;
            files_changed =
                worktree::checkout_tree(repo, Some(&current), &commit.tree, true)?.len();
        }
    }
    repo.set_merge_head(None)?;
    match repo.head()? {
        Head::Branch(b) => {
            let expect = match from {
                Some(id) => Expect::Is(id),
                None => Expect::Absent,
            };
            repo.refs().update_with(
                &format!("{HEADS_PREFIX}{b}"),
                expect,
                Some(to),
                &format!("reset: moving to {rev}"),
            )?;
            crate::reflog::append(
                repo.workspace_dir(),
                "HEAD",
                from,
                Some(to),
                &format!("reset: moving to {rev}"),
            )?;
        }
        Head::Detached(_) => {
            repo.set_head_with(&Head::Detached(to), &format!("reset: moving to {rev}"))?
        }
    }
    repo.events().append(
        "reset",
        Some(repo.workspace()),
        None,
        serde_json::json!({ "from": from, "to": to, "mode": mode }),
    )?;
    Ok(ResetResult {
        from,
        to,
        mode,
        files_changed,
    })
}

// ---------------------------------------------------------------------------
// Garbage collection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct GcOptions {
    /// Objects younger than this are never deleted (protects writers that
    /// have not published their refs yet).
    pub grace_secs: u64,
    /// Drop reflog entries older than this many seconds (None = keep all).
    pub reflog_expire_secs: Option<u64>,
    /// Drop checkpoints older than this many seconds (None = keep all).
    pub checkpoint_expire_secs: Option<u64>,
    /// Report only.
    pub dry_run: bool,
}

impl Default for GcOptions {
    fn default() -> Self {
        GcOptions {
            grace_secs: 120,
            reflog_expire_secs: Some(90 * 86_400),
            checkpoint_expire_secs: Some(30 * 86_400),
            dry_run: false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GcReport {
    pub objects_total: usize,
    pub objects_reachable: usize,
    pub objects_removed: usize,
    pub bytes_removed: u64,
    pub objects_kept_young: usize,
    pub reflog_entries_expired: usize,
    pub checkpoints_expired: usize,
    pub dry_run: bool,
}

fn mark_reachable(
    store: &crate::store::ObjectStore,
    roots: impl IntoIterator<Item = ObjectId>,
    seen: &mut HashSet<ObjectId>,
) -> Result<()> {
    let mut stack: Vec<ObjectId> = roots.into_iter().collect();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        match store.read(&id) {
            Ok(crate::object::Object::Commit(c)) => {
                stack.push(c.tree);
                stack.extend(c.parents);
                if let Some(prev) = c
                    .meta
                    .get("agit.previous_checkpoint")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<ObjectId>().ok())
                {
                    stack.push(prev);
                }
            }
            Ok(crate::object::Object::Tree(t)) => {
                for e in t.entries() {
                    if !e.mode.is_submodule() {
                        stack.push(e.id);
                    }
                }
            }
            Ok(_) => {}
            Err(Error::ObjectNotFound(_)) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Remove unreachable objects. Roots: all refs, all reflog entries, all
/// workspace HEADs and indexes, MERGE_HEADs, and checkpoint chains.
pub fn gc(repo: &Repository, opts: &GcOptions) -> Result<GcReport> {
    let store = repo.store();
    let now = now_ms();
    let mut report = GcReport {
        dry_run: opts.dry_run,
        ..Default::default()
    };
    let common = repo.common_dir();

    // 1. Expire checkpoints: cut each workspace's chain at the first entry
    //    older than the cutoff and re-chain the kept ones so the expired
    //    tail becomes unreachable.
    if let Some(secs) = opts.checkpoint_expire_secs {
        let cutoff = now - (secs as i64) * 1000;
        for (name, tip) in repo.refs().list(CHECKPOINTS_PREFIX)? {
            let mut chain: Vec<(ObjectId, Commit)> = Vec::new();
            let mut cur = Some(tip);
            while let Some(id) = cur {
                let c = store.read_commit(&id)?;
                cur = c
                    .meta
                    .get("agit.previous_checkpoint")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse().ok());
                chain.push((id, c));
            }
            let Some(first_expired) = chain.iter().position(|(_, c)| c.time_ms < cutoff) else {
                continue;
            };
            report.checkpoints_expired += chain.len() - first_expired;
            if opts.dry_run {
                continue;
            }
            if first_expired == 0 {
                repo.refs()
                    .update_with(&name, Expect::Is(tip), None, "gc: expire checkpoints")?;
                continue;
            }
            let mut prev: Option<ObjectId> = None;
            for (_, c) in chain[..first_expired].iter().rev() {
                let mut c = c.clone();
                match prev {
                    Some(p) => {
                        c.meta
                            .insert("agit.previous_checkpoint".into(), serde_json::json!(p));
                    }
                    None => {
                        c.meta.remove("agit.previous_checkpoint");
                    }
                }
                prev = Some(store.write_commit(&c)?);
            }
            repo.refs()
                .update_with(&name, Expect::Is(tip), prev, "gc: expire checkpoints")?;
        }
    }

    // 2. Expire reflogs.
    if let Some(secs) = opts.reflog_expire_secs {
        let cutoff = now - (secs as i64) * 1000;
        if !opts.dry_run {
            for name in crate::reflog::list_logs(common)? {
                report.reflog_entries_expired += crate::reflog::expire(common, &name, cutoff)?;
            }
            for ws in repo.list_workspaces()? {
                let dir = if ws.name == crate::repo::PRIMARY_WORKSPACE {
                    common.to_path_buf()
                } else {
                    common.join("workspaces").join(&ws.name)
                };
                report.reflog_entries_expired += crate::reflog::expire(&dir, "HEAD", cutoff)?;
            }
        }
    }

    // 3. Mark.
    let mut roots: Vec<ObjectId> = repo
        .refs()
        .list("refs/")?
        .into_iter()
        .map(|(_, id)| id)
        .collect();
    for name in crate::reflog::list_logs(common)? {
        for e in crate::reflog::read(common, &name)? {
            roots.extend(e.old.into_iter().chain(e.new));
        }
    }
    for ws in repo.list_workspaces()? {
        let dir = if ws.name == crate::repo::PRIMARY_WORKSPACE {
            common.to_path_buf()
        } else {
            common.join("workspaces").join(&ws.name)
        };
        for e in crate::reflog::read(&dir, "HEAD")? {
            roots.extend(e.old.into_iter().chain(e.new));
        }
        if let Ok(s) = std::fs::read_to_string(dir.join("HEAD")) {
            if let Ok(id) = s.trim().parse::<ObjectId>() {
                roots.push(id);
            }
        }
        if let Ok(s) = std::fs::read_to_string(dir.join("MERGE_HEAD")) {
            if let Ok(id) = s.trim().parse::<ObjectId>() {
                roots.push(id);
            }
        }
        if let Ok(idx) = crate::index::Index::load(&dir.join("index")) {
            roots.extend(idx.iter().map(|e| e.id));
        }
    }
    let mut reachable = HashSet::new();
    mark_reachable(store, roots, &mut reachable)?;

    // 4. Sweep. Loose objects are deleted one by one; packed objects are
    //    dropped by repacking the segments whose writers are gone. Objects
    //    younger than the grace period count as live for both.
    let all = store.iter_ids()?;
    report.objects_total = all.len();
    report.objects_reachable = all.iter().filter(|id| reachable.contains(id)).count();
    let mut live: HashSet<ObjectId> = reachable.clone();
    let mut dead_packed = 0usize;
    for id in &all {
        if reachable.contains(id) {
            continue;
        }
        if store.age_secs(id).is_none_or(|a| a < opts.grace_secs) {
            report.objects_kept_young += 1;
            live.insert(*id);
            continue;
        }
        report.bytes_removed += store.disk_size(id);
        if store.is_loose(id) {
            report.objects_removed += 1;
            if !opts.dry_run {
                store.remove(id)?;
            }
        } else {
            dead_packed += 1;
        }
    }
    if dead_packed > 0 {
        if opts.dry_run {
            report.objects_removed += dead_packed;
        } else {
            let (dropped, _) = store.repack(&live)?;
            report.objects_removed += dropped;
        }
    }
    repo.events().append(
        "gc",
        Some(repo.workspace()),
        None,
        serde_json::to_value(&report)?,
    )?;
    Ok(report)
}
