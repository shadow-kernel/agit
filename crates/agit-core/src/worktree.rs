//! Working tree operations: walking, status, add/remove, checkout.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::diff::{diff_maps, tree_to_sides, Change, ChangeKind, Side};
use crate::error::{io_at, Error, Result};
use crate::hash::ObjectId;
use crate::index::{FileStat, Index, IndexEntry};
use crate::object::{EntryMode, ObjectKind};
use crate::repo::{Repository, AGIT_DIR};
use crate::store::ObjectStore;

/// A file found in the working tree.
#[derive(Debug, Clone)]
pub struct WorkFile {
    pub rel: String,
    pub stat: FileStat,
}

/// Walk the working tree in parallel, honoring `.agitignore` and
/// `.gitignore`. Returns files only (no directories), sorted by path.
pub fn walk(repo: &Repository, roots: Option<&[String]>) -> Result<Vec<WorkFile>> {
    let work_dir = repo.work_dir().to_path_buf();
    let mut builder = match roots {
        Some(rs) if !rs.is_empty() => {
            let mut it = rs.iter().map(|r| repo.abs_path(r));
            let mut b = ignore::WalkBuilder::new(it.next().unwrap());
            for p in it {
                b.add(p);
            }
            b
        }
        _ => ignore::WalkBuilder::new(&work_dir),
    };
    builder
        .hidden(false)
        .parents(true)
        .require_git(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(false)
        .ignore(false)
        .follow_links(false)
        .add_custom_ignore_filename(".agitignore");
    let root_for_filter = work_dir.clone();
    builder.filter_entry(move |e| {
        if e.file_name() == AGIT_DIR {
            return false;
        }
        // Never descend into a nested repository (submodule or stray clone).
        if e.file_type().is_some_and(|t| t.is_dir()) && e.path() != root_for_filter {
            return !e.path().join(AGIT_DIR).exists();
        }
        true
    });
    let out = Mutex::new(Vec::new());
    let errors = Mutex::new(Vec::new());
    builder.build_parallel().run(|| {
        Box::new(|entry| {
            match entry {
                Ok(e) => {
                    let ft = match e.file_type() {
                        Some(ft) => ft,
                        None => return ignore::WalkState::Continue,
                    };
                    if ft.is_dir() {
                        return ignore::WalkState::Continue;
                    }
                    let md = match fs::symlink_metadata(e.path()) {
                        Ok(m) => m,
                        Err(_) => return ignore::WalkState::Continue,
                    };
                    if let Ok(rel) = e.path().strip_prefix(&work_dir) {
                        let rel = rel
                            .components()
                            .map(|c| c.as_os_str().to_string_lossy().to_string())
                            .collect::<Vec<_>>()
                            .join("/");
                        if rel.is_empty() {
                            return ignore::WalkState::Continue;
                        }
                        out.lock().unwrap().push(WorkFile {
                            rel,
                            stat: FileStat::from_metadata(&md),
                        });
                    }
                }
                Err(e) => errors.lock().unwrap().push(e.to_string()),
            }
            ignore::WalkState::Continue
        })
    });
    let mut files = out.into_inner().unwrap();
    files.sort_by(|a, b| a.rel.as_bytes().cmp(b.rel.as_bytes()));
    Ok(files)
}

fn is_ignored(repo: &Repository, rel: &str) -> bool {
    // Build a matcher rooted at the work dir; cheap for single checks.
    let mut b = ignore::WalkBuilder::new(repo.work_dir());
    b.hidden(false)
        .require_git(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(false)
        .ignore(false)
        .add_custom_ignore_filename(".agitignore");
    let abs = repo.abs_path(rel);
    // The ignore crate exposes matching through the walker only; walk just
    // the parent directory non-recursively and look for the file.
    let parent = abs.parent().unwrap_or(repo.work_dir()).to_path_buf();
    b.max_depth(Some(1));
    let mut w = ignore::WalkBuilder::new(&parent);
    w.hidden(false)
        .require_git(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(false)
        .ignore(false)
        .parents(true)
        .max_depth(Some(1))
        .add_custom_ignore_filename(".agitignore");
    for e in w.build().flatten() {
        if e.path() == abs {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub workspace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<ObjectId>,
    pub staged: Vec<Change>,
    pub unstaged: Vec<Change>,
    pub untracked: Vec<String>,
    pub conflicts: Vec<String>,
    pub merge_in_progress: bool,
}

impl Status {
    pub fn is_clean(&self) -> bool {
        self.staged.is_empty() && self.unstaged.is_empty() && self.conflicts.is_empty()
    }
}

fn index_sides(index: &Index) -> BTreeMap<String, Side> {
    index
        .iter()
        .map(|e| {
            (
                e.path.clone(),
                Side {
                    id: e.id,
                    mode: e.mode,
                    size: e.size,
                },
            )
        })
        .collect()
}

/// Compute status. Also refreshes the index stat cache for files whose
/// content turned out to be unchanged (so the next call is faster).
pub fn status(repo: &Repository, refresh_cache: bool) -> Result<Status> {
    let mut index = repo.load_index()?;
    let head = repo.head_commit()?;
    let head_tree = repo.head_tree()?;

    // staged: HEAD tree vs index
    let head_sides = tree_to_sides(repo.store(), head_tree.as_ref())?;
    let idx_sides = index_sides(&index);
    let staged = diff_maps(&head_sides, &idx_sides);

    // unstaged: index vs working tree
    let files = walk(repo, None)?;
    let on_disk: BTreeMap<&str, &FileStat> =
        files.iter().map(|f| (f.rel.as_str(), &f.stat)).collect();

    let (changed, cache_updates) = compare_index_to_disk(repo, &index, &on_disk)?;

    let mut untracked: Vec<String> = files
        .iter()
        .filter(|f| index.get(&f.rel).is_none())
        .map(|f| f.rel.clone())
        .collect();
    untracked.sort();

    if refresh_cache && !cache_updates.is_empty() {
        for (path, stat) in cache_updates {
            if let Some(e) = index.get(&path).cloned() {
                index.insert(IndexEntry {
                    mtime_ns: stat.mtime_ns,
                    ino: stat.ino,
                    size: stat.size,
                    ..e
                });
            }
        }
        // Best effort: a concurrent writer may hold the index; ignore errors.
        let _ = repo.save_index(&index);
    }

    Ok(Status {
        workspace: repo.workspace().to_string(),
        branch: repo.current_branch()?,
        head,
        staged,
        unstaged: changed,
        untracked,
        conflicts: index.conflicted_paths(),
        merge_in_progress: repo.merge_head()?.is_some(),
    })
}

/// Stat cache refreshes produced by a status pass: (path, fresh stat).
type StatRefresh = (String, FileStat);

/// Compare every index entry against the files on disk. Returns the changes
/// and a list of (path, stat) whose cache entries can be refreshed.
fn compare_index_to_disk(
    repo: &Repository,
    index: &Index,
    on_disk: &BTreeMap<&str, &FileStat>,
) -> Result<(Vec<Change>, Vec<StatRefresh>)> {
    let entries: Vec<&IndexEntry> = index.iter().collect();
    let results: Vec<(Option<Change>, Option<StatRefresh>)> = entries
        .par_iter()
        .map(|e| {
            let old = Side {
                id: e.id,
                mode: e.mode,
                size: e.size,
            };
            if e.mode == EntryMode::Submodule {
                return match crate::submodule::nested_head(repo, &e.path) {
                    Ok(Some(head)) if head == e.id => (None, None),
                    Ok(Some(head)) => (
                        Some(Change {
                            path: e.path.clone(),
                            kind: ChangeKind::Modified,
                            old: Some(old),
                            new: Some(Side {
                                id: head,
                                mode: EntryMode::Submodule,
                                size: 0,
                            }),
                        }),
                        None,
                    ),
                    _ => (
                        Some(Change {
                            path: e.path.clone(),
                            kind: ChangeKind::Deleted,
                            old: Some(old),
                            new: None,
                        }),
                        None,
                    ),
                };
            }
            let stat = match on_disk.get(e.path.as_str()) {
                Some(s) => **s,
                None => {
                    // Not found by the walker: either deleted or ignored.
                    let abs = repo.abs_path(&e.path);
                    match fs::symlink_metadata(&abs) {
                        Ok(md) if !md.is_dir() => FileStat::from_metadata(&md),
                        _ => {
                            return (
                                Some(Change {
                                    path: e.path.clone(),
                                    kind: ChangeKind::Deleted,
                                    old: Some(old),
                                    new: None,
                                }),
                                None,
                            )
                        }
                    }
                }
            };
            let stat_same = stat.size == e.size
                && stat.mtime_ns == e.mtime_ns
                && stat.ino == e.ino
                && stat.mode == e.mode
                && e.mtime_ns != 0;
            if stat_same {
                return (None, None);
            }
            let abs = repo.abs_path(&e.path);
            let (id, size) = match hash_path(&abs, stat.mode) {
                Ok(x) => x,
                Err(_) => {
                    return (
                        Some(Change {
                            path: e.path.clone(),
                            kind: ChangeKind::Deleted,
                            old: Some(old),
                            new: None,
                        }),
                        None,
                    )
                }
            };
            if id == e.id && stat.mode == e.mode {
                (None, Some((e.path.clone(), stat)))
            } else {
                let kind = if stat.mode != e.mode
                    && (stat.mode == EntryMode::Symlink || e.mode == EntryMode::Symlink)
                {
                    ChangeKind::TypeChanged
                } else {
                    ChangeKind::Modified
                };
                (
                    Some(Change {
                        path: e.path.clone(),
                        kind,
                        old: Some(old),
                        new: Some(Side {
                            id,
                            mode: stat.mode,
                            size,
                        }),
                    }),
                    None,
                )
            }
        })
        .collect();
    let mut changes = Vec::new();
    let mut refresh = Vec::new();
    for (c, r) in results {
        if let Some(c) = c {
            changes.push(c);
        }
        if let Some(r) = r {
            refresh.push(r);
        }
    }
    Ok((changes, refresh))
}

fn hash_path(abs: &Path, mode: EntryMode) -> Result<(ObjectId, u64)> {
    if mode == EntryMode::Symlink {
        let target = fs::read_link(abs).map_err(io_at(abs))?;
        let bytes = target.to_string_lossy().as_bytes().to_vec();
        Ok((
            crate::object::hash_object(ObjectKind::Blob, &bytes),
            bytes.len() as u64,
        ))
    } else {
        ObjectStore::hash_file(abs)
    }
}

fn store_path(store: &ObjectStore, abs: &Path, mode: EntryMode) -> Result<(ObjectId, u64)> {
    if mode == EntryMode::Symlink {
        let target = fs::read_link(abs).map_err(io_at(abs))?;
        let bytes = target.to_string_lossy().as_bytes().to_vec();
        Ok((store.write(ObjectKind::Blob, &bytes)?, bytes.len() as u64))
    } else {
        store.write_blob_from_path(abs)
    }
}

// ---------------------------------------------------------------------------
// Add / remove
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AddResult {
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub removed: Vec<String>,
    pub unchanged: usize,
}

/// Stage the given repo-relative paths (files or directories). Missing paths
/// that are tracked are staged as deletions. An empty list means everything.
pub fn add(repo: &Repository, rel_paths: &[String], include_ignored: bool) -> Result<AddResult> {
    let mut index = repo.load_index()?;
    let mut result = AddResult::default();

    // Determine the candidate file set.
    let mut files: Vec<WorkFile> = Vec::new();
    let mut explicit_dirs: Vec<String> = Vec::new();
    if rel_paths.is_empty() {
        files = walk(repo, None)?;
        explicit_dirs.push(String::new());
    } else {
        let mut dirs = Vec::new();
        for p in rel_paths {
            let abs = repo.abs_path(p);
            match fs::symlink_metadata(&abs) {
                Ok(md) if md.is_dir() => {
                    // A nested repository is handled as a submodule entry below;
                    // never walk into it (the walker does not filter roots).
                    if !abs.join(AGIT_DIR).exists() {
                        dirs.push(p.clone());
                    }
                }
                Ok(md) => {
                    if include_ignored || !is_ignored(repo, p) || index.get(p).is_some() {
                        files.push(WorkFile {
                            rel: p.clone(),
                            stat: FileStat::from_metadata(&md),
                        });
                    }
                }
                Err(_) => {
                    // Missing: stage deletions for everything tracked under it.
                    let under: Vec<String> = index.under(p).map(|e| e.path.clone()).collect();
                    if under.is_empty() {
                        return Err(Error::PathNotTracked(PathBuf::from(p)));
                    }
                    for u in under {
                        index.remove(&u);
                        result.removed.push(u);
                    }
                }
            }
        }
        if !dirs.is_empty() {
            files.extend(walk(repo, Some(&dirs))?);
            explicit_dirs.extend(dirs);
        }
    }

    // Registered submodules under the requested roots: stage their HEAD.
    let modules = crate::submodule::read_modules(repo)?;
    let mut submodule_entries: Vec<IndexEntry> = Vec::new();
    for m in &modules {
        let requested = rel_paths.is_empty()
            || rel_paths
                .iter()
                .any(|p| p == &m.path || m.path.starts_with(&format!("{p}/")) || p.is_empty());
        if !requested {
            continue;
        }
        if let Some(head) = crate::submodule::nested_head(repo, &m.path)? {
            submodule_entries.push(IndexEntry {
                path: m.path.clone(),
                id: head,
                mode: EntryMode::Submodule,
                size: 0,
                mtime_ns: 0,
                ino: 0,
                conflict: false,
            });
        }
    }
    for e in submodule_entries {
        let existing = index.get(&e.path).cloned();
        match existing {
            None => result.added.push(e.path.clone()),
            Some(x) if x.id != e.id || x.mode != e.mode => result.updated.push(e.path.clone()),
            Some(_) => result.unchanged += 1,
        }
        index.insert(e);
    }
    let submodule_paths: BTreeSet<String> = modules.iter().map(|m| m.path.clone()).collect();

    // Files tracked under explicit dirs but missing on disk -> deletions.
    let present: BTreeSet<&str> = files.iter().map(|f| f.rel.as_str()).collect();
    for d in &explicit_dirs {
        let gone: Vec<String> = index
            .under(d)
            .filter(|e| !present.contains(e.path.as_str()))
            .filter(|e| !submodule_paths.contains(&e.path))
            .filter(|e| !repo.abs_path(&e.path).exists())
            .map(|e| e.path.clone())
            .collect();
        for g in gone {
            index.remove(&g);
            result.removed.push(g);
        }
    }

    // Hash + store changed files in parallel.
    let store = repo.store();
    let work: Vec<(WorkFile, Option<IndexEntry>)> = files
        .into_iter()
        .map(|f| {
            let existing = index.get(&f.rel).cloned();
            (f, existing)
        })
        .collect();
    let outcomes: Vec<Result<(IndexEntry, bool, bool)>> = work
        .into_par_iter()
        .map(|(f, existing)| {
            if let Some(e) = &existing {
                let same = e.size == f.stat.size
                    && e.mtime_ns == f.stat.mtime_ns
                    && e.ino == f.stat.ino
                    && e.mode == f.stat.mode
                    && e.mtime_ns != 0
                    && !e.conflict;
                if same {
                    return Ok((e.clone(), false, false));
                }
            }
            let abs = repo.abs_path(&f.rel);
            let (id, size) = store_path(store, &abs, f.stat.mode)?;
            let is_new = existing.is_none();
            let changed = existing
                .as_ref()
                .is_none_or(|e| e.id != id || e.mode != f.stat.mode || e.conflict);
            Ok((
                IndexEntry {
                    path: f.rel,
                    id,
                    mode: f.stat.mode,
                    size,
                    mtime_ns: f.stat.mtime_ns,
                    ino: f.stat.ino,
                    conflict: false,
                },
                is_new,
                changed,
            ))
        })
        .collect();
    for o in outcomes {
        let (entry, is_new, changed) = o?;
        if is_new {
            result.added.push(entry.path.clone());
        } else if changed {
            result.updated.push(entry.path.clone());
        } else {
            result.unchanged += 1;
        }
        index.insert(entry);
    }
    repo.save_index(&index)?;
    Ok(result)
}

/// Unstage and (optionally) delete paths.
pub fn remove(repo: &Repository, rel_paths: &[String], delete_files: bool) -> Result<Vec<String>> {
    let mut index = repo.load_index()?;
    let mut removed = Vec::new();
    for p in rel_paths {
        let under: Vec<String> = index.under(p).map(|e| e.path.clone()).collect();
        if under.is_empty() {
            return Err(Error::PathNotTracked(PathBuf::from(p)));
        }
        for u in under {
            index.remove(&u);
            if delete_files {
                let abs = repo.abs_path(&u);
                if abs.exists() {
                    fs::remove_file(&abs).map_err(io_at(&abs))?;
                    remove_empty_parents(repo.work_dir(), &abs);
                }
            }
            removed.push(u);
        }
    }
    repo.save_index(&index)?;
    Ok(removed)
}

/// Reset index entries for `paths` back to HEAD (unstage).
pub fn unstage(repo: &Repository, rel_paths: &[String]) -> Result<Vec<String>> {
    let mut index = repo.load_index()?;
    let head_tree = repo.head_tree()?;
    let head = tree_to_sides(repo.store(), head_tree.as_ref())?;
    let mut touched = Vec::new();
    let targets: Vec<String> = if rel_paths.is_empty() {
        index
            .paths()
            .map(str::to_string)
            .chain(head.keys().cloned())
            .collect()
    } else {
        let mut v = Vec::new();
        for p in rel_paths {
            v.extend(index.under(p).map(|e| e.path.clone()));
            v.extend(
                head.keys()
                    .filter(|k| *k == p || k.starts_with(&format!("{p}/")))
                    .cloned(),
            );
        }
        v
    };
    let mut seen = BTreeSet::new();
    for t in targets {
        if !seen.insert(t.clone()) {
            continue;
        }
        match head.get(&t) {
            Some(side) => index.insert(IndexEntry {
                path: t.clone(),
                id: side.id,
                mode: side.mode,
                size: side.size,
                mtime_ns: 0,
                ino: 0,
                conflict: false,
            }),
            None => {
                index.remove(&t);
            }
        }
        touched.push(t);
    }
    repo.save_index(&index)?;
    Ok(touched)
}

// ---------------------------------------------------------------------------
// Checkout
// ---------------------------------------------------------------------------

/// Make the working tree and index match `target` tree.
///
/// `from` is the tree the working tree is currently based on (HEAD). Only
/// paths that differ between `from` and `target` are touched; if any of those
/// paths have local modifications and `force` is false, nothing is changed and
/// `DirtyWorktree` is returned.
pub fn checkout_tree(
    repo: &Repository,
    from: Option<&ObjectId>,
    target: &ObjectId,
    force: bool,
) -> Result<Vec<Change>> {
    let store = repo.store();
    let from_sides = tree_to_sides(store, from)?;
    let target_sides = tree_to_sides(store, Some(target))?;
    let changes = diff_maps(&from_sides, &target_sides);
    if changes.is_empty() {
        // Still make sure the index reflects the target.
        let mut index = Index::from_tree(store, target)?;
        refresh_stats(repo, &mut index);
        repo.save_index(&index)?;
        return Ok(changes);
    }

    if !force {
        let index = repo.load_index()?;
        let mut dirty = Vec::new();
        for c in &changes {
            let abs = repo.abs_path(&c.path);
            let from_side = from_sides.get(&c.path);
            if from_side.is_some_and(|s| s.mode == EntryMode::Submodule)
                || c.new
                    .as_ref()
                    .is_some_and(|s| s.mode == EntryMode::Submodule)
            {
                continue;
            }
            match (fs::symlink_metadata(&abs), from_side) {
                (Ok(md), Some(fs_side)) => {
                    let stat = FileStat::from_metadata(&md);
                    let cached_ok = index.get(&c.path).is_some_and(|e| {
                        e.id == fs_side.id
                            && e.mtime_ns == stat.mtime_ns
                            && e.size == stat.size
                            && e.ino == stat.ino
                            && e.mtime_ns != 0
                    });
                    if !cached_ok {
                        let (id, _) = hash_path(&abs, stat.mode)?;
                        if id != fs_side.id {
                            dirty.push(PathBuf::from(&c.path));
                        }
                    }
                }
                (Ok(_), None) => {
                    // Untracked file would be overwritten.
                    if target_sides.contains_key(&c.path) {
                        dirty.push(PathBuf::from(&c.path));
                    }
                }
                (Err(_), _) => {}
            }
        }
        if !dirty.is_empty() {
            return Err(Error::DirtyWorktree(dirty));
        }
    }

    // Apply.
    changes.par_iter().try_for_each(|c| -> Result<()> {
        let abs = repo.abs_path(&c.path);
        match c.kind {
            ChangeKind::Deleted => {
                if c.old
                    .as_ref()
                    .is_some_and(|s| s.mode == EntryMode::Submodule)
                {
                    // Leave the nested repository in place; `submodule` manages it.
                } else if abs.exists() || fs::symlink_metadata(&abs).is_ok() {
                    fs::remove_file(&abs).map_err(io_at(&abs))?;
                }
            }
            _ => {
                let side = c.new.as_ref().unwrap();
                write_entry(store, &abs, side)?;
            }
        }
        Ok(())
    })?;
    for c in &changes {
        if c.kind == ChangeKind::Deleted {
            remove_empty_parents(repo.work_dir(), &repo.abs_path(&c.path));
        }
    }

    let mut index = Index::from_tree(store, target)?;
    refresh_stats(repo, &mut index);
    repo.save_index(&index)?;
    Ok(changes)
}

fn refresh_stats(repo: &Repository, index: &mut Index) {
    let paths: Vec<String> = index.paths().map(str::to_string).collect();
    let stats: Vec<(String, Option<FileStat>)> = paths
        .into_par_iter()
        .map(|p| {
            let s = fs::symlink_metadata(repo.abs_path(&p))
                .ok()
                .map(|md| FileStat::from_metadata(&md));
            (p, s)
        })
        .collect();
    for (p, s) in stats {
        if let (Some(s), Some(e)) = (s, index.get(&p).cloned()) {
            index.insert(IndexEntry {
                mtime_ns: s.mtime_ns,
                ino: s.ino,
                size: s.size,
                ..e
            });
        }
    }
}

fn write_entry(store: &ObjectStore, abs: &Path, side: &Side) -> Result<()> {
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent).map_err(io_at(parent))?;
    }
    if fs::symlink_metadata(abs).is_ok() {
        fs::remove_file(abs).map_err(io_at(abs))?;
    }
    match side.mode {
        EntryMode::Symlink => {
            let target = store.read_blob(&side.id)?;
            let target = String::from_utf8_lossy(&target).to_string();
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, abs).map_err(io_at(abs))?;
            #[cfg(not(unix))]
            fs::write(abs, target).map_err(io_at(abs))?;
        }
        EntryMode::Submodule => {
            fs::create_dir_all(abs).map_err(io_at(abs))?;
        }
        _ => {
            let data = store.read_blob(&side.id)?;
            crate::refs::write_atomic(abs, &data)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = if side.mode == EntryMode::Executable {
                    0o755
                } else {
                    0o644
                };
                fs::set_permissions(abs, fs::Permissions::from_mode(mode)).map_err(io_at(abs))?;
            }
        }
    }
    Ok(())
}

fn remove_empty_parents(root: &Path, abs: &Path) {
    let mut cur = abs.parent();
    while let Some(dir) = cur {
        if dir == root {
            break;
        }
        if fs::remove_dir(dir).is_err() {
            break;
        }
        cur = dir.parent();
    }
}

/// Write the given tree into the working tree without touching paths that
/// are identical, and *without* dirty checks (used by merge to materialize
/// results and by checkpoint restore).
pub fn write_tree_to_worktree(repo: &Repository, target: &ObjectId) -> Result<Vec<Change>> {
    let current = repo.head_tree()?;
    checkout_tree(repo, current.as_ref(), target, true)
}

/// Snapshot the complete working tree (tracked + untracked, ignoring ignored
/// files) into a tree object without modifying the index.
pub fn snapshot_worktree(repo: &Repository) -> Result<ObjectId> {
    let files = walk(repo, None)?;
    let index = repo.load_index()?;
    let store = repo.store();
    let entries: Vec<Result<(String, EntryMode, ObjectId, u64)>> = files
        .into_par_iter()
        .map(|f| {
            if let Some(e) = index.get(&f.rel) {
                let same = e.size == f.stat.size
                    && e.mtime_ns == f.stat.mtime_ns
                    && e.ino == f.stat.ino
                    && e.mode == f.stat.mode
                    && e.mtime_ns != 0;
                if same {
                    return Ok((f.rel, e.mode, e.id, e.size));
                }
            }
            let (id, size) = store_path(store, &repo.abs_path(&f.rel), f.stat.mode)?;
            Ok((f.rel, f.stat.mode, id, size))
        })
        .collect();
    let mut builder = crate::index::TreeBuilder::default();
    for e in entries {
        let (p, m, id, s) = e?;
        builder.insert(&p, m, id, s);
    }
    for m in crate::submodule::read_modules(repo)? {
        if let Some(head) = crate::submodule::nested_head(repo, &m.path)? {
            builder.insert(&m.path, EntryMode::Submodule, head, 0);
        }
    }
    builder.write(store)
}
