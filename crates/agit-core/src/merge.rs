//! Three-way merge of trees, plus merge-base computation.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::diff::{is_binary, tree_to_sides, Side};
use crate::error::Result;
use crate::hash::ObjectId;
use crate::index::TreeBuilder;
use crate::object::{EntryMode, ObjectKind};
use crate::store::ObjectStore;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    /// Both sides changed the content and the hunks overlap.
    Content,
    /// One side deleted, the other modified.
    DeleteModify,
    /// Both sides added different content.
    AddAdd,
    /// Mode/type changed in incompatible ways.
    Mode,
    /// Binary files changed on both sides.
    Binary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conflict {
    pub path: String,
    pub kind: ConflictKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<ObjectId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ours: Option<ObjectId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theirs: Option<ObjectId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeOutcome {
    /// Merged tree. When there are conflicts, conflicted text files contain
    /// conflict markers and deleted/modified paths keep the modified side.
    pub tree: ObjectId,
    pub conflicts: Vec<Conflict>,
    /// Paths that were merged automatically by combining both sides' hunks.
    pub auto_merged: Vec<String>,
}

impl MergeOutcome {
    pub fn is_clean(&self) -> bool {
        self.conflicts.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct MergeLabels {
    pub ours: String,
    pub theirs: String,
}

/// Merge `ours` and `theirs` relative to `base` (which may be `None` for
/// unrelated histories).
pub fn merge_trees(
    store: &ObjectStore,
    base: Option<&ObjectId>,
    ours: &ObjectId,
    theirs: &ObjectId,
    labels: &MergeLabels,
) -> Result<MergeOutcome> {
    let b = tree_to_sides(store, base)?;
    let o = tree_to_sides(store, Some(ours))?;
    let t = tree_to_sides(store, Some(theirs))?;

    let mut paths: Vec<&String> = b.keys().chain(o.keys()).chain(t.keys()).collect();
    paths.sort();
    paths.dedup();

    let mut builder = TreeBuilder::default();
    let mut conflicts = Vec::new();
    let mut auto_merged = Vec::new();

    for path in paths {
        let bs = b.get(path);
        let os = o.get(path);
        let ts = t.get(path);
        match resolve(store, path, bs, os, ts, labels)? {
            Resolution::Keep(Some(side)) => builder.insert(path, side.mode, side.id, side.size),
            Resolution::Keep(None) => {}
            Resolution::Merged(side) => {
                auto_merged.push(path.clone());
                builder.insert(path, side.mode, side.id, side.size);
            }
            Resolution::Conflict(c, side) => {
                if let Some(s) = side {
                    builder.insert(path, s.mode, s.id, s.size);
                }
                conflicts.push(c);
            }
        }
    }

    Ok(MergeOutcome {
        tree: builder.write(store)?,
        conflicts,
        auto_merged,
    })
}

enum Resolution {
    Keep(Option<Side>),
    Merged(Side),
    Conflict(Conflict, Option<Side>),
}

fn resolve(
    store: &ObjectStore,
    path: &str,
    base: Option<&Side>,
    ours: Option<&Side>,
    theirs: Option<&Side>,
    labels: &MergeLabels,
) -> Result<Resolution> {
    // Trivial cases first.
    if ours == theirs {
        return Ok(Resolution::Keep(ours.cloned()));
    }
    if ours == base {
        return Ok(Resolution::Keep(theirs.cloned()));
    }
    if theirs == base {
        return Ok(Resolution::Keep(ours.cloned()));
    }
    let conflict = |kind, side: Option<&Side>| {
        Resolution::Conflict(
            Conflict {
                path: path.to_string(),
                kind,
                base: base.map(|s| s.id),
                ours: ours.map(|s| s.id),
                theirs: theirs.map(|s| s.id),
            },
            side.cloned(),
        )
    };

    match (ours, theirs) {
        (None, None) => Ok(Resolution::Keep(None)),
        (Some(o), None) => Ok(conflict(ConflictKind::DeleteModify, Some(o))),
        (None, Some(t)) => Ok(conflict(ConflictKind::DeleteModify, Some(t))),
        (Some(o), Some(t)) => {
            if o.mode != t.mode && base.map(|b| b.mode) != Some(o.mode) && base.map(|b| b.mode) != Some(t.mode) {
                return Ok(conflict(ConflictKind::Mode, Some(o)));
            }
            if o.mode == EntryMode::Symlink || t.mode == EntryMode::Symlink {
                return Ok(conflict(ConflictKind::Content, Some(o)));
            }
            // Mode: prefer whichever side changed it.
            let mode = if base.map(|b| b.mode) == Some(o.mode) { t.mode } else { o.mode };
            if o.id == t.id {
                return Ok(Resolution::Keep(Some(Side { id: o.id, mode, size: o.size })));
            }
            let ob = store.read_blob(&o.id)?;
            let tb = store.read_blob(&t.id)?;
            let bb = match base {
                Some(b) => store.read_blob(&b.id)?,
                None => Vec::new(),
            };
            if is_binary(&ob) || is_binary(&tb) || is_binary(&bb) {
                return Ok(conflict(ConflictKind::Binary, Some(o)));
            }
            let os = String::from_utf8_lossy(&ob);
            let ts = String::from_utf8_lossy(&tb);
            let bs = String::from_utf8_lossy(&bb);
            let mut opts = diffy::MergeOptions::new();
            opts.set_conflict_style(diffy::ConflictStyle::Diff3);
            match opts.merge(&bs, &os, &ts) {
                Ok(merged) => {
                    let id = store.write(ObjectKind::Blob, merged.as_bytes())?;
                    Ok(Resolution::Merged(Side { id, mode, size: merged.len() as u64 }))
                }
                Err(with_markers) => {
                    let text = with_markers
                        .replace("<<<<<<< ours", &format!("<<<<<<< {}", labels.ours))
                        .replace(">>>>>>> theirs", &format!(">>>>>>> {}", labels.theirs));
                    let id = store.write(ObjectKind::Blob, text.as_bytes())?;
                    let kind = if base.is_none() { ConflictKind::AddAdd } else { ConflictKind::Content };
                    Ok(conflict(kind, Some(&Side { id, mode, size: text.len() as u64 })))
                }
            }
        }
    }
}

/// Best common ancestor of two commits (first found in a breadth-first walk
/// from both sides, preferring the one with the latest timestamp among ties).
pub fn merge_base(store: &ObjectStore, a: &ObjectId, b: &ObjectId) -> Result<Option<ObjectId>> {
    if a == b {
        return Ok(Some(*a));
    }
    let mut seen_a: HashSet<ObjectId> = HashSet::new();
    let mut seen_b: HashSet<ObjectId> = HashSet::new();
    let mut qa = VecDeque::from([*a]);
    let mut qb = VecDeque::from([*b]);
    let mut candidates: Vec<ObjectId> = Vec::new();
    let mut parents_cache: HashMap<ObjectId, Vec<ObjectId>> = HashMap::new();

    let mut parents = |id: &ObjectId| -> Result<Vec<ObjectId>> {
        if let Some(p) = parents_cache.get(id) {
            return Ok(p.clone());
        }
        let p = store.read_commit(id)?.parents;
        parents_cache.insert(*id, p.clone());
        Ok(p)
    };

    while !qa.is_empty() || !qb.is_empty() {
        if let Some(x) = qa.pop_front() {
            if seen_a.insert(x) {
                if seen_b.contains(&x) {
                    candidates.push(x);
                } else {
                    for p in parents(&x)? {
                        qa.push_back(p);
                    }
                }
            }
        }
        if let Some(y) = qb.pop_front() {
            if seen_b.insert(y) {
                if seen_a.contains(&y) {
                    candidates.push(y);
                } else {
                    for p in parents(&y)? {
                        qb.push_back(p);
                    }
                }
            }
        }
        if !candidates.is_empty() && qa.is_empty() && qb.is_empty() {
            break;
        }
        if candidates.len() >= 1 && (qa.len() + qb.len()) > 64 {
            break;
        }
    }
    if candidates.is_empty() {
        return Ok(None);
    }
    // Drop candidates that are ancestors of other candidates.
    let mut best: Vec<ObjectId> = Vec::new();
    'outer: for c in &candidates {
        for other in &candidates {
            if c != other && is_ancestor(store, c, other)? {
                continue 'outer;
            }
        }
        best.push(*c);
    }
    let mut best_with_time: Vec<(i64, ObjectId)> = best
        .into_iter()
        .map(|id| Ok((store.read_commit(&id)?.time_ms, id)))
        .collect::<Result<_>>()?;
    best_with_time.sort();
    Ok(best_with_time.last().map(|(_, id)| *id))
}

/// Is `anc` reachable from `desc`?
pub fn is_ancestor(store: &ObjectStore, anc: &ObjectId, desc: &ObjectId) -> Result<bool> {
    if anc == desc {
        return Ok(true);
    }
    let mut seen = HashSet::new();
    let mut q = VecDeque::from([*desc]);
    while let Some(x) = q.pop_front() {
        if !seen.insert(x) {
            continue;
        }
        for p in store.read_commit(&x)?.parents {
            if p == *anc {
                return Ok(true);
            }
            q.push_back(p);
        }
    }
    Ok(false)
}

/// Convenience: all paths touched between two trees, used to pre-check
/// leases before a merge.
pub fn changed_paths(store: &ObjectStore, from: Option<&ObjectId>, to: &ObjectId) -> Result<Vec<String>> {
    let a: BTreeMap<String, Side> = tree_to_sides(store, from)?;
    let b = tree_to_sides(store, Some(to))?;
    Ok(crate::diff::diff_maps(&a, &b).into_iter().map(|c| c.path).collect())
}
