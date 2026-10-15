//! Tree-level and content-level diffs.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::hash::ObjectId;
use crate::index::flatten_tree;
use crate::object::{EntryMode, TreeEntry};
use crate::store::ObjectStore;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    TypeChanged,
}

impl ChangeKind {
    pub fn letter(self) -> char {
        match self {
            ChangeKind::Added => 'A',
            ChangeKind::Modified => 'M',
            ChangeKind::Deleted => 'D',
            ChangeKind::TypeChanged => 'T',
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Side {
    pub id: ObjectId,
    pub mode: EntryMode,
    pub size: u64,
}

impl From<&TreeEntry> for Side {
    fn from(e: &TreeEntry) -> Self {
        Side {
            id: e.id,
            mode: e.mode,
            size: e.size,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub path: String,
    pub kind: ChangeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old: Option<Side>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new: Option<Side>,
}

/// Diff two flattened path maps.
pub fn diff_maps(old: &BTreeMap<String, Side>, new: &BTreeMap<String, Side>) -> Vec<Change> {
    let mut out = Vec::new();
    let mut oi = old.iter().peekable();
    let mut ni = new.iter().peekable();
    loop {
        match (oi.peek(), ni.peek()) {
            (None, None) => break,
            (Some((op, os)), None) => {
                out.push(Change {
                    path: (*op).clone(),
                    kind: ChangeKind::Deleted,
                    old: Some((*os).clone()),
                    new: None,
                });
                oi.next();
            }
            (None, Some((np, ns))) => {
                out.push(Change {
                    path: (*np).clone(),
                    kind: ChangeKind::Added,
                    old: None,
                    new: Some((*ns).clone()),
                });
                ni.next();
            }
            (Some((op, os)), Some((np, ns))) => match op.as_bytes().cmp(np.as_bytes()) {
                std::cmp::Ordering::Less => {
                    out.push(Change {
                        path: (*op).clone(),
                        kind: ChangeKind::Deleted,
                        old: Some((*os).clone()),
                        new: None,
                    });
                    oi.next();
                }
                std::cmp::Ordering::Greater => {
                    out.push(Change {
                        path: (*np).clone(),
                        kind: ChangeKind::Added,
                        old: None,
                        new: Some((*ns).clone()),
                    });
                    ni.next();
                }
                std::cmp::Ordering::Equal => {
                    if os != ns {
                        let kind = if os.mode != ns.mode
                            && (os.mode == EntryMode::Symlink || ns.mode == EntryMode::Symlink)
                        {
                            ChangeKind::TypeChanged
                        } else {
                            ChangeKind::Modified
                        };
                        out.push(Change {
                            path: (*op).clone(),
                            kind,
                            old: Some((*os).clone()),
                            new: Some((*ns).clone()),
                        });
                    }
                    oi.next();
                    ni.next();
                }
            },
        }
    }
    out
}

pub fn tree_to_sides(store: &ObjectStore, tree: Option<&ObjectId>) -> Result<BTreeMap<String, Side>> {
    Ok(match tree {
        None => BTreeMap::new(),
        Some(t) => flatten_tree(store, t)?
            .into_iter()
            .map(|(p, e)| (p, Side::from(&e)))
            .collect(),
    })
}

/// Diff two trees (either may be absent = empty).
///
/// Walks both trees recursively and skips subtrees with identical ids, so
/// the cost is proportional to the size of the change, not the repository.
pub fn diff_trees(store: &ObjectStore, old: Option<&ObjectId>, new: Option<&ObjectId>) -> Result<Vec<Change>> {
    let mut out = Vec::new();
    diff_tree_rec(store, "", old, new, &mut out)?;
    Ok(out)
}

fn join(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}/{name}")
    }
}

/// Emit every file under `tree` as Added or Deleted.
fn emit_subtree(store: &ObjectStore, prefix: &str, tree: &ObjectId, deleted: bool, out: &mut Vec<Change>) -> Result<()> {
    let t = store.read_tree(tree)?;
    for e in t.entries() {
        let path = join(prefix, &e.name);
        if e.mode.is_dir() {
            emit_subtree(store, &path, &e.id, deleted, out)?;
        } else {
            let side = Some(Side::from(e));
            out.push(if deleted {
                Change { path, kind: ChangeKind::Deleted, old: side, new: None }
            } else {
                Change { path, kind: ChangeKind::Added, old: None, new: side }
            });
        }
    }
    Ok(())
}

fn diff_tree_rec(
    store: &ObjectStore,
    prefix: &str,
    old: Option<&ObjectId>,
    new: Option<&ObjectId>,
    out: &mut Vec<Change>,
) -> Result<()> {
    if old == new {
        return Ok(());
    }
    let empty = crate::object::Tree::new();
    let ot = match old {
        Some(id) => store.read_tree(id)?,
        None => empty.clone(),
    };
    let nt = match new {
        Some(id) => store.read_tree(id)?,
        None => empty,
    };
    let (oe, ne) = (ot.entries(), nt.entries());
    let (mut i, mut j) = (0, 0);
    while i < oe.len() || j < ne.len() {
        let ord = match (oe.get(i), ne.get(j)) {
            (Some(a), Some(b)) => a.name.as_bytes().cmp(b.name.as_bytes()),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => break,
        };
        match ord {
            std::cmp::Ordering::Less => {
                let e = &oe[i];
                let path = join(prefix, &e.name);
                if e.mode.is_dir() {
                    emit_subtree(store, &path, &e.id, true, out)?;
                } else {
                    out.push(Change { path, kind: ChangeKind::Deleted, old: Some(Side::from(e)), new: None });
                }
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                let e = &ne[j];
                let path = join(prefix, &e.name);
                if e.mode.is_dir() {
                    emit_subtree(store, &path, &e.id, false, out)?;
                } else {
                    out.push(Change { path, kind: ChangeKind::Added, old: None, new: Some(Side::from(e)) });
                }
                j += 1;
            }
            std::cmp::Ordering::Equal => {
                let (a, b) = (&oe[i], &ne[j]);
                let path = join(prefix, &a.name);
                match (a.mode.is_dir(), b.mode.is_dir()) {
                    (true, true) => {
                        if a.id != b.id {
                            diff_tree_rec(store, &path, Some(&a.id), Some(&b.id), out)?;
                        }
                    }
                    (true, false) => {
                        emit_subtree(store, &path, &a.id, true, out)?;
                        out.push(Change { path, kind: ChangeKind::Added, old: None, new: Some(Side::from(b)) });
                    }
                    (false, true) => {
                        out.push(Change { path: path.clone(), kind: ChangeKind::Deleted, old: Some(Side::from(a)), new: None });
                        emit_subtree(store, &path, &b.id, false, out)?;
                    }
                    (false, false) => {
                        if a.id != b.id || a.mode != b.mode {
                            let kind = if a.mode != b.mode
                                && (a.mode == EntryMode::Symlink || b.mode == EntryMode::Symlink)
                            {
                                ChangeKind::TypeChanged
                            } else {
                                ChangeKind::Modified
                            };
                            out.push(Change { path, kind, old: Some(Side::from(a)), new: Some(Side::from(b)) });
                        }
                    }
                }
                i += 1;
                j += 1;
            }
        }
    }
    Ok(())
}

pub fn is_binary(data: &[u8]) -> bool {
    let probe = &data[..data.len().min(8000)];
    probe.contains(&0)
}

/// Unified diff for one file. Returns `None` for binary content.
pub fn unified(path: &str, old: &[u8], new: &[u8], context: usize) -> Option<String> {
    if is_binary(old) || is_binary(new) {
        return None;
    }
    let old_s = String::from_utf8_lossy(old);
    let new_s = String::from_utf8_lossy(new);
    let patch = diffy::DiffOptions::new()
        .set_context_len(context)
        .create_patch(&old_s, &new_s);
    let mut text = diffy::PatchFormatter::new().fmt_patch(&patch).to_string();
    // diffy emits `--- original` / `+++ modified`; replace with real paths.
    let header = format!("--- a/{path}\n+++ b/{path}\n");
    if let Some(idx) = text.find("\n@@") {
        text = format!("{header}{}", &text[idx + 1..]);
    } else if !text.is_empty() {
        text = header;
    }
    Some(text)
}

/// Line statistics (added, removed) for a textual change.
pub fn line_stats(old: &[u8], new: &[u8]) -> (usize, usize) {
    if is_binary(old) || is_binary(new) {
        return (0, 0);
    }
    let old_s = String::from_utf8_lossy(old);
    let new_s = String::from_utf8_lossy(new);
    let patch = diffy::create_patch(&old_s, &new_s);
    let mut add = 0;
    let mut del = 0;
    for h in patch.hunks() {
        for l in h.lines() {
            match l {
                diffy::Line::Insert(_) => add += 1,
                diffy::Line::Delete(_) => del += 1,
                diffy::Line::Context(_) => {}
            }
        }
    }
    (add, del)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn side(n: u8) -> Side {
        Side {
            id: ObjectId::from_bytes([n; 32]),
            mode: EntryMode::File,
            size: n as u64,
        }
    }

    #[test]
    fn map_diff() {
        let mut a = BTreeMap::new();
        a.insert("a".to_string(), side(1));
        a.insert("b".to_string(), side(2));
        let mut b = BTreeMap::new();
        b.insert("b".to_string(), side(3));
        b.insert("c".to_string(), side(4));
        let d = diff_maps(&a, &b);
        assert_eq!(d.len(), 3);
        assert_eq!((d[0].path.as_str(), d[0].kind), ("a", ChangeKind::Deleted));
        assert_eq!((d[1].path.as_str(), d[1].kind), ("b", ChangeKind::Modified));
        assert_eq!((d[2].path.as_str(), d[2].kind), ("c", ChangeKind::Added));
    }

    #[test]
    fn unified_has_paths() {
        let u = unified("x.txt", b"a\nb\n", b"a\nc\n", 3).unwrap();
        assert!(u.starts_with("--- a/x.txt\n+++ b/x.txt\n@@"));
        assert!(u.contains("-b\n+c\n"));
        assert_eq!(line_stats(b"a\nb\n", b"a\nc\n"), (1, 1));
        assert!(unified("bin", b"\0\x01", b"\0\x02", 3).is_none());
    }
}
