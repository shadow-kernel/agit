//! History operations beyond commit/merge: blame, revert, cherry-pick,
//! stash and grep.

use std::collections::BTreeMap;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::diff::is_binary;
use crate::error::{Error, Result};
use crate::events::now_ms;
use crate::hash::ObjectId;
use crate::index::{flatten_tree, IndexEntry};
use crate::merge::{merge_trees, Conflict, MergeLabels};
use crate::object::{AgentInfo, Commit};
use crate::ops::{log_store, LogOptions};
use crate::refs::Expect;
use crate::repo::Repository;
use crate::store::ObjectStore;
use crate::worktree;

// ---------------------------------------------------------------------------
// Blame
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlameLine {
    /// 1-based line number in the blamed revision.
    pub line: usize,
    pub commit: ObjectId,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlameCommit {
    pub id: ObjectId,
    pub author: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub time_ms: i64,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Blame {
    pub path: String,
    pub rev: ObjectId,
    pub lines: Vec<BlameLine>,
    /// Commits referenced by `lines`, keyed by id.
    pub commits: BTreeMap<ObjectId, BlameCommit>,
    /// True when the history walk hit its limit and older lines are
    /// attributed to the oldest commit examined.
    pub truncated: bool,
}

fn blob_at(store: &ObjectStore, commit: &ObjectId, path: &str) -> Result<Option<Vec<u8>>> {
    let tree = store.read_commit(commit)?.tree;
    let flat = flatten_tree(store, &tree)?;
    match flat.get(path) {
        Some(e) if !e.mode.is_dir() && !e.mode.is_submodule() => Ok(Some(store.read_blob(&e.id)?)),
        _ => Ok(None),
    }
}

fn lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .split_inclusive('\n')
        .map(|l| l.trim_end_matches(['\n', '\r']).to_string())
        .collect()
}

/// Line-by-line attribution of `path` at `rev`, following the first-parent
/// history of commits that touched the path (at most `max_commits`).
pub fn blame(store: &ObjectStore, rev: ObjectId, path: &str, max_commits: usize) -> Result<Blame> {
    let Some(current) = blob_at(store, &rev, path)? else {
        return Err(Error::PathNotTracked(path.into()));
    };
    if is_binary(&current) {
        return Err(Error::Invalid(format!("{path} is binary")));
    }
    let cur_lines = lines(&current);
    let n = cur_lines.len();
    let mut owner: Vec<Option<ObjectId>> = vec![None; n];

    // Commits touching the path, newest first, first-parent only.
    let touching = log_store(
        store,
        &[rev],
        &LogOptions {
            limit: Some(max_commits + 1),
            path: Some(path.to_string()),
            first_parent: true,
            ..Default::default()
        },
    )?;
    let truncated = touching.len() > max_commits;
    let touching: Vec<_> = touching.into_iter().take(max_commits).collect();

    // `map[i]` = index into the version we are currently comparing, for
    // original line i (None once the line has been attributed).
    let mut map: Vec<Option<usize>> = (0..n).map(Some).collect();
    let mut newer_content = cur_lines.clone();
    let mut commits: BTreeMap<ObjectId, BlameCommit> = BTreeMap::new();
    let mut last_commit = rev;

    for entry in &touching {
        let cid = entry.id;
        last_commit = cid;
        let c = &entry.commit;
        commits.entry(cid).or_insert_with(|| BlameCommit {
            id: cid,
            author: c.author.name.clone(),
            agent: c.agent.as_ref().map(|a| a.id.clone()),
            time_ms: c.time_ms,
            summary: c.summary().to_string(),
            intent: c.intent.clone(),
        });
        // Content of the path in this commit and in its first parent.
        let this_content = if cid == rev {
            newer_content.clone()
        } else {
            blob_at(store, &cid, path)?
                .map(|b| lines(&b))
                .unwrap_or_default()
        };
        let older_content: Vec<String> = match c.parents.first() {
            Some(p) => blob_at(store, p, path)?
                .map(|b| lines(&b))
                .unwrap_or_default(),
            None => vec![],
        };
        // Alignment this_content -> older_content.
        let align = align_lines(&older_content, &this_content);
        // Lines of `this_content` that are inserts belong to `cid`.
        // `map` currently points into `newer_content` which equals
        // this_content (we rebase it each iteration).
        let mut new_map = vec![None; n];
        for (orig, pos) in map.iter().enumerate() {
            let Some(j) = *pos else { continue };
            match align.get(j).copied().flatten() {
                Some(i_old) => new_map[orig] = Some(i_old),
                None => {
                    if owner[orig].is_none() {
                        owner[orig] = Some(cid);
                    }
                }
            }
        }
        map = new_map;
        newer_content = older_content;
        if map.iter().all(Option::is_none) {
            break;
        }
    }
    // Anything still unattributed belongs to the oldest commit examined.
    for o in owner.iter_mut() {
        if o.is_none() {
            *o = Some(last_commit);
        }
    }
    let out = cur_lines
        .into_iter()
        .enumerate()
        .map(|(i, text)| BlameLine {
            line: i + 1,
            commit: owner[i].unwrap(),
            text,
        })
        .collect();
    Ok(Blame {
        path: path.to_string(),
        rev,
        lines: out,
        commits,
        truncated,
    })
}

/// For each line index of `new`, the index of the equal line in `old`
/// (None when inserted). Uses diffy's line diff.
fn align_lines(old: &[String], new: &[String]) -> Vec<Option<usize>> {
    let old_s: String = old.iter().map(|l| format!("{l}\n")).collect();
    let new_s: String = new.iter().map(|l| format!("{l}\n")).collect();
    let patch = diffy::create_patch(&old_s, &new_s);
    let mut out = vec![None; new.len()];
    let (mut i, mut j) = (0usize, 0usize);
    for hunk in patch.hunks() {
        let (os, ns) = (
            hunk.old_range().start().saturating_sub(1),
            hunk.new_range().start().saturating_sub(1),
        );
        // Equal run before the hunk.
        while j < ns && i < os {
            out[j] = Some(i);
            i += 1;
            j += 1;
        }
        for line in hunk.lines() {
            match line {
                diffy::Line::Context(_) => {
                    if j < new.len() {
                        out[j] = Some(i);
                    }
                    i += 1;
                    j += 1;
                }
                diffy::Line::Delete(_) => i += 1,
                diffy::Line::Insert(_) => j += 1,
            }
        }
    }
    while j < new.len() && i < old.len() {
        out[j] = Some(i);
        i += 1;
        j += 1;
    }
    out
}

// ---------------------------------------------------------------------------
// Revert / cherry-pick
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ApplyResult {
    Committed {
        commit: ObjectId,
        files_changed: usize,
    },
    Conflicts {
        conflicts: Vec<Conflict>,
        auto_merged: Vec<String>,
    },
}

fn require_clean(repo: &Repository) -> Result<()> {
    let st = worktree::status(repo, false)?;
    if !st.is_clean() {
        return Err(Error::DirtyWorktree(
            st.staged
                .iter()
                .chain(st.unstaged.iter())
                .map(|c| std::path::PathBuf::from(&c.path))
                .collect(),
        ));
    }
    Ok(())
}

/// Apply a three-way change (base -> theirs) onto HEAD and commit it with
/// the given message, or leave conflicts in the working tree.
fn apply_three_way(
    repo: &Repository,
    base_tree: Option<&ObjectId>,
    theirs_tree: &ObjectId,
    label: &str,
    commit_template: Commit,
) -> Result<ApplyResult> {
    let store = repo.store();
    let head = repo
        .head_commit()?
        .ok_or_else(|| Error::UnknownRevision("HEAD (unborn branch)".into()))?;
    let head_tree = store.read_commit(&head)?.tree;
    let labels = MergeLabels {
        ours: repo.current_branch()?.unwrap_or_else(|| "HEAD".into()),
        theirs: label.to_string(),
    };
    let outcome = merge_trees(store, base_tree, &head_tree, theirs_tree, &labels)?;
    let changes = worktree::checkout_tree(repo, Some(&head_tree), &outcome.tree, true)?;
    if !outcome.is_clean() {
        let mut index = repo.load_index()?;
        for c in &outcome.conflicts {
            if let Some(e) = index.get(&c.path).cloned() {
                index.insert(IndexEntry {
                    conflict: true,
                    ..e
                });
            }
        }
        repo.save_index(&index)?;
        return Ok(ApplyResult::Conflicts {
            conflicts: outcome.conflicts,
            auto_merged: outcome.auto_merged,
        });
    }
    if outcome.tree == head_tree {
        return Err(Error::NothingToCommit);
    }
    let commit = Commit {
        tree: outcome.tree,
        parents: vec![head],
        time_ms: now_ms(),
        ..commit_template
    };
    let id = store.write_commit(&commit)?;
    repo.advance_head_with(Some(head), id, &format!("{label}: {}", commit.summary()))?;
    repo.events().append(
        "commit",
        Some(repo.workspace()),
        commit.agent.as_ref().map(|a| a.id.as_str()),
        serde_json::json!({ "id": id, "summary": commit.summary(), "kind": label, "files": changes.len() }),
    )?;
    Ok(ApplyResult::Committed {
        commit: id,
        files_changed: changes.len(),
    })
}

/// Undo the changes of `rev` on top of HEAD.
pub fn revert(repo: &Repository, rev: &str, agent: Option<AgentInfo>) -> Result<ApplyResult> {
    require_clean(repo)?;
    let (id, c) = repo.resolve_commit(rev)?;
    let parent_tree = match c.parents.first() {
        Some(p) => Some(repo.store().read_commit(p)?.tree),
        None => None,
    };
    let mut meta = BTreeMap::new();
    meta.insert("agit.reverts".into(), serde_json::json!(id));
    let template = Commit {
        tree: ObjectId::ZERO,
        parents: vec![],
        author: repo.author()?,
        time_ms: 0,
        message: format!("Revert \"{}\"\n\nThis reverts commit {id}.", c.summary()),
        intent: c.intent.as_ref().map(|i| format!("undo: {i}")),
        task: c.task.clone(),
        agent,
        checkpoint: false,
        meta,
        signature: None,
    };
    // base = the commit itself, theirs = its parent: applying (rev -> parent) undoes it.
    let empty;
    let theirs = match &parent_tree {
        Some(t) => t,
        None => {
            empty = crate::index::TreeBuilder::default().write(repo.store())?;
            &empty
        }
    };
    apply_three_way(repo, Some(&c.tree), theirs, "revert", template)
}

/// Apply the changes of `rev` on top of HEAD.
pub fn cherry_pick(repo: &Repository, rev: &str, agent: Option<AgentInfo>) -> Result<ApplyResult> {
    require_clean(repo)?;
    let (id, c) = repo.resolve_commit(rev)?;
    let parent_tree = match c.parents.first() {
        Some(p) => Some(repo.store().read_commit(p)?.tree),
        None => None,
    };
    let mut meta = c.meta.clone();
    meta.insert("agit.cherry_picked_from".into(), serde_json::json!(id));
    let template = Commit {
        tree: ObjectId::ZERO,
        parents: vec![],
        author: c.author.clone(),
        time_ms: 0,
        message: c.message.clone(),
        intent: c.intent.clone(),
        task: c.task.clone(),
        agent: agent.or_else(|| c.agent.clone()),
        checkpoint: false,
        meta,
        signature: None,
    };
    apply_three_way(repo, parent_tree.as_ref(), &c.tree, "cherry-pick", template)
}

// ---------------------------------------------------------------------------
// Stash
// ---------------------------------------------------------------------------

pub const STASH_REF: &str = "refs/stash";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StashEntry {
    pub index: usize,
    pub id: ObjectId,
    pub message: String,
    pub time_ms: i64,
    pub base: Option<ObjectId>,
}

fn stash_chain(repo: &Repository) -> Result<Vec<(ObjectId, Commit)>> {
    let mut out = Vec::new();
    let mut cur = repo.refs().read(STASH_REF)?;
    while let Some(id) = cur {
        let c = repo.store().read_commit(&id)?;
        cur = c
            .meta
            .get("agit.stash_previous")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok());
        out.push((id, c));
    }
    Ok(out)
}

pub fn stash_list(repo: &Repository) -> Result<Vec<StashEntry>> {
    Ok(stash_chain(repo)?
        .into_iter()
        .enumerate()
        .map(|(i, (id, c))| StashEntry {
            index: i,
            id,
            message: c.message.clone(),
            time_ms: c.time_ms,
            base: c.parents.first().copied(),
        })
        .collect())
}

/// Save the working tree (tracked and untracked changes) and reset it to HEAD.
pub fn stash_push(repo: &Repository, message: Option<String>) -> Result<StashEntry> {
    let head = repo
        .head_commit()?
        .ok_or_else(|| Error::UnknownRevision("HEAD (unborn branch)".into()))?;
    let head_tree = repo.store().read_commit(&head)?.tree;
    let tree = worktree::snapshot_worktree(repo)?;
    if tree == head_tree {
        return Err(Error::NothingToCommit);
    }
    let previous = repo.refs().read(STASH_REF)?;
    let branch = repo.current_branch()?.unwrap_or_else(|| "HEAD".into());
    let mut meta = BTreeMap::new();
    if let Some(p) = previous {
        meta.insert("agit.stash_previous".into(), serde_json::json!(p));
    }
    let commit = Commit {
        tree,
        parents: vec![head],
        author: repo.author()?,
        time_ms: now_ms(),
        message: message.unwrap_or_else(|| format!("WIP on {branch}")),
        intent: None,
        task: None,
        agent: None,
        checkpoint: true,
        meta,
        signature: None,
    };
    let id = repo.store().write_commit(&commit)?;
    repo.refs()
        .update_with(STASH_REF, Expect::Any, Some(id), "stash: push")?;
    worktree::checkout_tree(repo, Some(&tree), &head_tree, true)?;
    repo.events().append(
        "stash.push",
        Some(repo.workspace()),
        None,
        serde_json::json!({ "id": id }),
    )?;
    Ok(StashEntry {
        index: 0,
        id,
        message: commit.message,
        time_ms: commit.time_ms,
        base: Some(head),
    })
}

/// Re-apply stash entry `index` onto the current working tree (three-way:
/// base = the stash's parent). `drop` removes it from the stack on success.
pub fn stash_apply(repo: &Repository, index: usize, drop: bool) -> Result<ApplyResult> {
    let chain = stash_chain(repo)?;
    let (id, c) = chain
        .get(index)
        .cloned()
        .ok_or_else(|| Error::NotFound(format!("stash@{{{index}}}")))?;
    let base_tree = match c.parents.first() {
        Some(p) => Some(repo.store().read_commit(p)?.tree),
        None => None,
    };
    let current = worktree::snapshot_worktree(repo)?;
    let labels = MergeLabels {
        ours: "working tree".into(),
        theirs: format!("stash@{{{index}}}"),
    };
    let outcome = merge_trees(repo.store(), base_tree.as_ref(), &current, &c.tree, &labels)?;
    let applied = worktree::checkout_tree(repo, Some(&current), &outcome.tree, true)?.len();
    // Keep the index at HEAD so the restored changes show as unstaged.
    if let Some(head_tree) = repo.head_tree()? {
        let mut idx = crate::index::Index::from_tree(repo.store(), &head_tree)?;
        if !outcome.is_clean() {
            for cf in &outcome.conflicts {
                if let Some(e) = idx.get(&cf.path).cloned() {
                    idx.insert(IndexEntry {
                        conflict: true,
                        ..e
                    });
                }
            }
        }
        repo.save_index(&idx)?;
    }
    if !outcome.is_clean() {
        return Ok(ApplyResult::Conflicts {
            conflicts: outcome.conflicts,
            auto_merged: outcome.auto_merged,
        });
    }
    if drop {
        stash_drop(repo, index)?;
    }
    repo.events().append(
        "stash.apply",
        Some(repo.workspace()),
        None,
        serde_json::json!({ "id": id, "dropped": drop }),
    )?;
    Ok(ApplyResult::Committed {
        commit: id,
        files_changed: applied,
    })
}

pub fn stash_drop(repo: &Repository, index: usize) -> Result<ObjectId> {
    let chain = stash_chain(repo)?;
    if index >= chain.len() {
        return Err(Error::NotFound(format!("stash@{{{index}}}")));
    }
    let dropped = chain[index].0;
    // Re-chain the entries above `index`.
    let tail = chain.get(index + 1).map(|(id, _)| *id);
    let mut prev = tail;
    for (_, c) in chain[..index].iter().rev() {
        let mut c = c.clone();
        match prev {
            Some(p) => {
                c.meta
                    .insert("agit.stash_previous".into(), serde_json::json!(p));
            }
            None => {
                c.meta.remove("agit.stash_previous");
            }
        }
        prev = Some(repo.store().write_commit(&c)?);
    }
    repo.refs()
        .update_with(STASH_REF, Expect::Any, prev, "stash: drop")?;
    Ok(dropped)
}

// ---------------------------------------------------------------------------
// Grep
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrepMatch {
    pub path: String,
    pub line: usize,
    pub text: String,
}

/// Regex search over the working tree (rev = None) or a commit's tree.
pub fn grep(
    repo: &Repository,
    pattern: &str,
    rev: Option<&str>,
    ignore_case: bool,
    max_file_bytes: u64,
) -> Result<Vec<GrepMatch>> {
    let re = regex::RegexBuilder::new(pattern)
        .case_insensitive(ignore_case)
        .build()
        .map_err(|e| Error::Invalid(format!("bad pattern: {e}")))?;
    let files: Vec<(String, Vec<u8>)> = match rev {
        None => {
            let walked = worktree::walk(repo, None)?;
            walked
                .into_par_iter()
                .filter(|f| f.stat.size <= max_file_bytes)
                .filter_map(|f| {
                    std::fs::read(repo.abs_path(&f.rel))
                        .ok()
                        .map(|b| (f.rel, b))
                })
                .collect()
        }
        Some(r) => {
            let tree = repo.resolve_commit(r)?.1.tree;
            let flat = flatten_tree(repo.store(), &tree)?;
            let store = repo.store();
            flat.into_par_iter()
                .filter(|(_, e)| !e.mode.is_submodule() && e.size <= max_file_bytes)
                .filter_map(|(p, e)| store.read_blob(&e.id).ok().map(|b| (p, b)))
                .collect()
        }
    };
    let mut out: Vec<GrepMatch> = files
        .into_par_iter()
        .filter(|(_, b)| !is_binary(b))
        .flat_map_iter(|(path, bytes)| {
            let text = String::from_utf8_lossy(&bytes).to_string();
            let re = re.clone();
            text.lines()
                .enumerate()
                .filter(move |(_, l)| re.is_match(l))
                .map(|(i, l)| GrepMatch {
                    path: path.clone(),
                    line: i + 1,
                    text: l.to_string(),
                })
                .collect::<Vec<_>>()
        })
        .collect();
    out.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
    Ok(out)
}

// ---------------------------------------------------------------------------
// Graph rendering helper (used by `log --graph`)
// ---------------------------------------------------------------------------

/// Render an ASCII commit graph for commits given newest first with their
/// parents. Returns one prefix string per commit (e.g. `* `, `|\\ `), plus
/// connector lines to print after it.
pub fn graph_prefixes(commits: &[(ObjectId, Vec<ObjectId>)]) -> Vec<(String, Vec<String>)> {
    let mut lanes: Vec<Option<ObjectId>> = Vec::new();
    let mut out = Vec::new();
    for (id, parents) in commits {
        let lane = match lanes.iter().position(|l| *l == Some(*id)) {
            Some(i) => i,
            None => {
                lanes.push(Some(*id));
                lanes.len() - 1
            }
        };
        let mut row = String::new();
        for (i, l) in lanes.iter().enumerate() {
            if i == lane {
                row.push('*');
            } else if l.is_some() {
                row.push('|');
            } else {
                row.push(' ');
            }
            row.push(' ');
        }
        // Replace this lane with the first parent; extra parents open new lanes.
        let mut connectors = Vec::new();
        match parents.first() {
            Some(p) => {
                // If the first parent already has a lane, this lane ends (merge into it).
                if let Some(existing) = lanes.iter().position(|l| *l == Some(*p)) {
                    if existing != lane {
                        lanes[lane] = None;
                        let mut c = String::new();
                        for (i, l) in lanes.iter().enumerate() {
                            if i == lane {
                                c.push_str("|/");
                            } else if l.is_some() {
                                c.push_str("| ");
                            } else {
                                c.push_str("  ");
                            }
                        }
                        connectors.push(c.trim_end().to_string());
                    }
                } else {
                    lanes[lane] = Some(*p);
                }
            }
            None => lanes[lane] = None,
        }
        for p in parents.iter().skip(1) {
            if !lanes.contains(&Some(*p)) {
                lanes.push(Some(*p));
                let mut c = String::new();
                for (i, l) in lanes.iter().enumerate() {
                    if i == lane {
                        c.push_str("|\\");
                    } else if l.is_some() && i != lanes.len() - 1 {
                        c.push_str("| ");
                    } else {
                        c.push_str("  ");
                    }
                }
                connectors.push(c.trim_end().to_string());
            }
        }
        while lanes.last().is_some_and(|l| l.is_none()) {
            lanes.pop();
        }
        out.push((row.trim_end().to_string(), connectors));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn align_basic() {
        let old = vec!["a".to_string(), "b".into(), "c".into()];
        let new = vec![
            "a".to_string(),
            "x".into(),
            "b".into(),
            "c".into(),
            "d".into(),
        ];
        let a = align_lines(&old, &new);
        assert_eq!(a, vec![Some(0), None, Some(1), Some(2), None]);
    }
}
