//! Reachability: which objects must travel for a fetch or push.

use std::collections::{HashSet, VecDeque};

use agit_core::object::Object;
use agit_core::store::ObjectStore;
use agit_core::{ObjectId, Result};

/// All commits reachable from `roots` (bounded by `limit` to keep pathological
/// histories from blowing up a single request; None = unbounded).
pub fn commit_closure(store: &ObjectStore, roots: &[ObjectId], limit: Option<usize>) -> Result<HashSet<ObjectId>> {
    let mut seen = HashSet::new();
    let mut q: VecDeque<ObjectId> = roots.iter().copied().collect();
    while let Some(id) = q.pop_front() {
        if !seen.insert(id) {
            continue;
        }
        if limit.is_some_and(|l| seen.len() >= l) {
            break;
        }
        if let Ok(c) = store.read_commit(&id) {
            for p in c.parents {
                if !seen.contains(&p) {
                    q.push_back(p);
                }
            }
        }
    }
    Ok(seen)
}

/// Collect every object (tree + blob) under `tree`, skipping subtrees whose
/// id is in `known` (and adding what it visits to `known`).
fn collect_tree(store: &ObjectStore, tree: ObjectId, known: &mut HashSet<ObjectId>, out: &mut Vec<ObjectId>) -> Result<()> {
    if !known.insert(tree) {
        return Ok(());
    }
    out.push(tree);
    let t = store.read_tree(&tree)?;
    for e in t.entries() {
        if e.mode.is_dir() {
            collect_tree(store, e.id, known, out)?;
        } else if !e.mode.is_submodule() && known.insert(e.id) {
            out.push(e.id);
        }
    }
    Ok(())
}

/// Mark every object under `tree` as known without emitting it.
fn mark_tree(store: &ObjectStore, tree: ObjectId, known: &mut HashSet<ObjectId>) -> Result<()> {
    if !known.insert(tree) {
        return Ok(());
    }
    if let Ok(t) = store.read_tree(&tree) {
        for e in t.entries() {
            if e.mode.is_dir() {
                mark_tree(store, e.id, known)?;
            } else {
                known.insert(e.id);
            }
        }
    }
    Ok(())
}

/// Objects reachable from `wants` that the other side (which has everything
/// reachable from `haves`) is missing. Commits come after their trees and
/// blobs so a receiver can verify as it goes; order within is topological
/// enough for import (import does not require order anyway).
pub fn objects_to_send(store: &ObjectStore, wants: &[ObjectId], haves: &[ObjectId]) -> Result<Vec<ObjectId>> {
    let have_commits = commit_closure(store, haves, None)?;
    // Commits to send: BFS from wants, stop at have_commits.
    let mut send_commits = Vec::new();
    let mut seen = HashSet::new();
    let mut q: VecDeque<ObjectId> = wants.iter().copied().collect();
    while let Some(id) = q.pop_front() {
        if have_commits.contains(&id) || !seen.insert(id) {
            continue;
        }
        match store.read(&id)? {
            Object::Commit(c) => {
                send_commits.push((id, c.clone()));
                for p in c.parents {
                    q.push_back(p);
                }
            }
            _ => {
                // A want that is not a commit (tag of a tree etc.): send as-is.
                send_commits.push((id, agit_core::Commit {
                    tree: id,
                    parents: vec![],
                    author: agit_core::Signature { name: String::new(), email: String::new() },
                    time_ms: 0,
                    message: String::new(),
                    intent: None,
                    task: None,
                    agent: None,
                    checkpoint: false,
                    meta: Default::default(),
                }));
            }
        }
    }
    // Known objects: trees of boundary parents that the receiver has.
    let mut known: HashSet<ObjectId> = HashSet::new();
    for (_, c) in &send_commits {
        for p in &c.parents {
            if have_commits.contains(p) {
                if let Ok(pc) = store.read_commit(p) {
                    mark_tree(store, pc.tree, &mut known)?;
                }
            }
        }
    }
    let mut out = Vec::new();
    // Oldest first so parents precede children.
    for (id, c) in send_commits.iter().rev() {
        if c.tree != *id {
            collect_tree(store, c.tree, &mut known, &mut out)?;
            out.push(*id);
        } else if known.insert(*id) {
            out.push(*id);
        }
    }
    Ok(out)
}
