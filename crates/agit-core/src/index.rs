//! The index (staging area) and its stat cache.
//!
//! Paths are repository-relative, `/`-separated, UTF-8. Entries are kept in a
//! sorted map so building trees is a single ordered pass.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{io_at, Error, Result};
use crate::hash::{Hasher, ObjectId};
use crate::object::{EntryMode, Tree, TreeEntry};
use crate::refs::write_atomic_bulk;
use crate::store::ObjectStore;

const MAGIC_V1: &[u8; 4] = b"AGI1";
const MAGIC: &[u8; 4] = b"AGI2";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexEntry {
    pub path: String,
    pub id: ObjectId,
    pub mode: EntryMode,
    pub size: u64,
    /// Modification time in nanoseconds since the epoch, as observed when the
    /// entry was added. Used to skip re-hashing unchanged files.
    pub mtime_ns: i128,
    pub ino: u64,
    /// Set while a merge left this path conflicted.
    #[serde(default)]
    pub conflict: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Index {
    entries: BTreeMap<String, IndexEntry>,
    /// Cached tree ids per directory ("" is the root). An entry is dropped
    /// whenever a path under that directory changes, so a valid entry means
    /// the directory's tree object is already in the store. This makes
    /// `write_tree` cost proportional to the change, not the repository.
    tree_cache: BTreeMap<String, (ObjectId, u64)>,
}

#[derive(Serialize, Deserialize)]
struct IndexFile<'a> {
    entries: Vec<std::borrow::Cow<'a, IndexEntry>>,
    tree_cache: Vec<(String, ObjectId, u64)>,
}

fn ancestors(path: &str) -> impl Iterator<Item = &str> {
    let mut cur = path;
    std::iter::from_fn(move || {
        match cur.rfind('/') {
            Some(i) => {
                cur = &cur[..i];
                Some(cur)
            }
            None if !cur.is_empty() => {
                cur = "";
                Some("")
            }
            None => None,
        }
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStat {
    pub size: u64,
    pub mtime_ns: i128,
    pub ino: u64,
    pub mode: EntryMode,
}

impl FileStat {
    pub fn from_metadata(md: &std::fs::Metadata) -> FileStat {
        use std::time::UNIX_EPOCH;
        let mtime_ns = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as i128)
            .unwrap_or(0);
        #[cfg(unix)]
        let (ino, mode) = {
            use std::os::unix::fs::MetadataExt;
            let mode = if md.file_type().is_symlink() {
                EntryMode::Symlink
            } else if md.mode() & 0o111 != 0 {
                EntryMode::Executable
            } else {
                EntryMode::File
            };
            (md.ino(), mode)
        };
        #[cfg(not(unix))]
        let (ino, mode) = (
            0u64,
            if md.file_type().is_symlink() {
                EntryMode::Symlink
            } else {
                EntryMode::File
            },
        );
        FileStat {
            size: md.len(),
            mtime_ns,
            ino,
            mode,
        }
    }
}

impl Index {
    pub fn new() -> Self {
        Index::default()
    }

    pub fn load(path: &Path) -> Result<Index> {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Index::new()),
            Err(e) => return Err(Error::IoAt { path: path.to_path_buf(), source: e }),
        };
        if bytes.len() < 4 + 32 || (&bytes[..4] != MAGIC && &bytes[..4] != MAGIC_V1) {
            return Err(Error::Invalid(format!("corrupt index at {}", path.display())));
        }
        let v1 = &bytes[..4] == MAGIC_V1;
        let (body, sum) = bytes.split_at(bytes.len() - 32);
        let actual = Hasher::new().update(body).finalize();
        if actual.as_bytes() != sum {
            return Err(Error::Invalid(format!(
                "index checksum mismatch at {}",
                path.display()
            )));
        }
        if v1 {
            let list: Vec<IndexEntry> = bincode::deserialize(&body[4..])?;
            return Ok(Index {
                entries: list.into_iter().map(|e| (e.path.clone(), e)).collect(),
                tree_cache: BTreeMap::new(),
            });
        }
        let file: IndexFile<'static> = bincode::deserialize(&body[4..])?;
        Ok(Index {
            entries: file.entries.into_iter().map(|e| (e.path.clone(), e.into_owned())).collect(),
            tree_cache: file.tree_cache.into_iter().map(|(p, id, n)| (p, (id, n))).collect(),
        })
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let file = IndexFile {
            entries: self.entries.values().map(std::borrow::Cow::Borrowed).collect(),
            tree_cache: self.tree_cache.iter().map(|(p, (id, n))| (p.clone(), *id, *n)).collect(),
        };
        let mut body = Vec::with_capacity(self.entries.len() * 96 + 4);
        body.extend_from_slice(MAGIC);
        bincode::serialize_into(&mut body, &file)?;
        let sum = Hasher::new().update(&body).finalize();
        body.extend_from_slice(sum.as_bytes());
        write_atomic_bulk(path, &body)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, path: &str) -> Option<&IndexEntry> {
        self.entries.get(path)
    }

    pub fn insert(&mut self, entry: IndexEntry) {
        let changed = self.entries.get(&entry.path).is_none_or(|e| e.id != entry.id || e.mode != entry.mode || e.size != entry.size);
        if changed {
            self.invalidate(&entry.path);
        }
        self.entries.insert(entry.path.clone(), entry);
    }

    pub fn remove(&mut self, path: &str) -> Option<IndexEntry> {
        let e = self.entries.remove(path);
        if e.is_some() {
            self.invalidate(path);
        }
        e
    }

    fn invalidate(&mut self, path: &str) {
        for a in ancestors(path) {
            self.tree_cache.remove(a);
        }
    }

    /// Number of cached directory trees (for diagnostics and tests).
    pub fn cached_trees(&self) -> usize {
        self.tree_cache.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &IndexEntry> {
        self.entries.values()
    }

    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(|s| s.as_str())
    }

    pub fn has_conflicts(&self) -> bool {
        self.entries.values().any(|e| e.conflict)
    }

    pub fn conflicted_paths(&self) -> Vec<String> {
        self.entries
            .values()
            .filter(|e| e.conflict)
            .map(|e| e.path.clone())
            .collect()
    }

    /// Entries whose path is `dir` itself or lives under `dir/`.
    pub fn under<'a>(&'a self, dir: &'a str) -> impl Iterator<Item = &'a IndexEntry> + 'a {
        let dir = dir.trim_end_matches('/');
        self.entries.values().filter(move |e| {
            dir.is_empty() || e.path == dir || e.path.starts_with(&format!("{dir}/"))
        })
    }

    /// Build (and store) the tree objects for the current index. Returns the
    /// root tree id.
    pub fn write_tree(&self, store: &ObjectStore) -> Result<ObjectId> {
        if self.has_conflicts() {
            return Err(Error::Invalid(format!(
                "index has unresolved conflicts: {:?}",
                self.conflicted_paths()
            )));
        }
        let mut builder = TreeBuilder::default();
        for e in self.entries.values() {
            builder.insert(&e.path, e.mode, e.id, e.size);
        }
        builder.write_cached(store, &self.tree_cache)
    }

    /// Like [`write_tree`](Self::write_tree) but also refreshes the tree
    /// cache so the next call is cheap. Requires `&mut self`.
    pub fn write_tree_cached(&mut self, store: &ObjectStore) -> Result<ObjectId> {
        if self.has_conflicts() {
            return Err(Error::Invalid(format!(
                "index has unresolved conflicts: {:?}",
                self.conflicted_paths()
            )));
        }
        let mut builder = TreeBuilder::default();
        for e in self.entries.values() {
            builder.insert(&e.path, e.mode, e.id, e.size);
        }
        let mut cache = std::mem::take(&mut self.tree_cache);
        let id = builder.write_with_cache(store, &mut cache)?;
        self.tree_cache = cache;
        Ok(id)
    }

    /// Replace the index with the contents of `tree_id` (stat info cleared,
    /// so the next status will re-hash every file once).
    pub fn from_tree(store: &ObjectStore, tree_id: &ObjectId) -> Result<Index> {
        let mut idx = Index::new();
        let mut cache = BTreeMap::new();
        let mut stack = vec![(String::new(), *tree_id)];
        while let Some((prefix, tid)) = stack.pop() {
            let tree = store.read_tree(&tid)?;
            cache.insert(prefix.clone(), (tid, tree.len() as u64));
            for e in tree.into_entries() {
                let path = if prefix.is_empty() {
                    e.name
                } else {
                    format!("{prefix}/{}", e.name)
                };
                if e.mode.is_dir() {
                    stack.push((path, e.id));
                } else {
                    idx.insert(IndexEntry {
                        path,
                        id: e.id,
                        mode: e.mode,
                        size: e.size,
                        mtime_ns: 0,
                        ino: 0,
                        conflict: false,
                    });
                }
            }
        }
        idx.tree_cache = cache;
        Ok(idx)
    }
}

/// Flattened path -> entry list for a tree (recursive).
pub fn flatten_tree(store: &ObjectStore, tree_id: &ObjectId) -> Result<BTreeMap<String, TreeEntry>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![(String::new(), *tree_id)];
    while let Some((prefix, tid)) = stack.pop() {
        let tree = store.read_tree(&tid)?;
        for e in tree.into_entries() {
            let path = if prefix.is_empty() {
                e.name.clone()
            } else {
                format!("{prefix}/{}", e.name)
            };
            if e.mode.is_dir() {
                stack.push((path, e.id));
            } else {
                out.insert(path, e);
            }
        }
    }
    Ok(out)
}

/// Incrementally builds a nested tree from flat paths.
#[derive(Default)]
pub struct TreeBuilder {
    files: Vec<(String, EntryMode, ObjectId, u64)>,
}

impl TreeBuilder {
    pub fn insert(&mut self, path: &str, mode: EntryMode, id: ObjectId, size: u64) {
        self.files.push((path.to_string(), mode, id, size));
    }

    pub fn write(self, store: &ObjectStore) -> Result<ObjectId> {
        let mut cache = BTreeMap::new();
        self.write_with_cache(store, &mut cache)
    }

    /// Write using a read-only cache of known directory tree ids.
    pub fn write_cached(self, store: &ObjectStore, cache: &BTreeMap<String, (ObjectId, u64)>) -> Result<ObjectId> {
        let mut c = cache.clone();
        self.write_with_cache(store, &mut c)
    }

    /// Write, consulting and updating `cache` (dir path -> (tree id, entries)).
    pub fn write_with_cache(mut self, store: &ObjectStore, cache: &mut BTreeMap<String, (ObjectId, u64)>) -> Result<ObjectId> {
        self.files.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        let mut root = Node::default();
        for (path, mode, id, size) in self.files {
            let mut node = &mut root;
            let mut parts = path.split('/').peekable();
            while let Some(part) = parts.next() {
                if parts.peek().is_none() {
                    node.files.push(TreeEntry {
                        name: part.to_string(),
                        mode,
                        id,
                        size,
                    });
                } else {
                    node = node.dirs.entry(part.to_string()).or_default();
                }
            }
        }
        root.write(store, "", cache)
    }
}

#[derive(Default)]
struct Node {
    files: Vec<TreeEntry>,
    dirs: BTreeMap<String, Node>,
}

impl Node {
    fn write(self, store: &ObjectStore, path: &str, cache: &mut BTreeMap<String, (ObjectId, u64)>) -> Result<ObjectId> {
        if let Some((id, _)) = cache.get(path) {
            return Ok(*id);
        }
        let mut entries = self.files;
        for (name, node) in self.dirs {
            let count = node.files.len() + node.dirs.len();
            let child_path = if path.is_empty() { name.clone() } else { format!("{path}/{name}") };
            let id = node.write(store, &child_path, cache)?;
            entries.push(TreeEntry {
                name,
                mode: EntryMode::Dir,
                id,
                size: count as u64,
            });
        }
        let n = entries.len() as u64;
        let id = store.write_tree(&Tree::from_entries(entries))?;
        cache.insert(path.to_string(), (id, n));
        Ok(id)
    }
}

pub(crate) fn read_dir_sorted(dir: &Path) -> Result<Vec<std::fs::DirEntry>> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .map_err(io_at(dir))?
        .collect::<std::io::Result<_>>()
        .map_err(io_at(dir))?;
    v.sort_by_key(|e| e.file_name());
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_roundtrip_and_tree() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::init(dir.path().join("objects")).unwrap();
        let blob = store.write(crate::object::ObjectKind::Blob, b"x").unwrap();
        let mut idx = Index::new();
        for p in ["src/main.rs", "src/lib/a.rs", "README.md"] {
            idx.insert(IndexEntry {
                path: p.into(),
                id: blob,
                mode: EntryMode::File,
                size: 1,
                mtime_ns: 1,
                ino: 1,
                conflict: false,
            });
        }
        let path = dir.path().join("index");
        idx.save(&path).unwrap();
        let back = Index::load(&path).unwrap();
        assert_eq!(idx, back);

        let tree_id = idx.write_tree(&store).unwrap();
        let root = store.read_tree(&tree_id).unwrap();
        assert_eq!(root.len(), 2);
        assert!(root.get("src").unwrap().mode.is_dir());
        let flat = flatten_tree(&store, &tree_id).unwrap();
        assert_eq!(flat.len(), 3);
        let mut again = Index::from_tree(&store, &tree_id).unwrap();
        assert_eq!(again.len(), 3);
        assert_eq!(again.cached_trees(), 3); // "", src, src/lib
        assert_eq!(again.write_tree(&store).unwrap(), tree_id);
        // Changing one file invalidates exactly its ancestors.
        let blob2 = store.write(crate::object::ObjectKind::Blob, b"y").unwrap();
        again.insert(IndexEntry { path: "src/lib/a.rs".into(), id: blob2, mode: EntryMode::File, size: 1, mtime_ns: 2, ino: 1, conflict: false });
        assert_eq!(again.cached_trees(), 0);
        let t2 = again.write_tree_cached(&store).unwrap();
        assert_ne!(t2, tree_id);
        assert_eq!(again.cached_trees(), 3);
        again.save(&path).unwrap();
        let loaded = Index::load(&path).unwrap();
        assert_eq!(loaded.cached_trees(), 3);
        assert_eq!(loaded.write_tree(&store).unwrap(), t2);
    }
}
