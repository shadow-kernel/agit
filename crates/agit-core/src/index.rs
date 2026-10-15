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

const MAGIC: &[u8; 4] = b"AGI1";

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
        if bytes.len() < 4 + 32 || &bytes[..4] != MAGIC {
            return Err(Error::Invalid(format!("corrupt index at {}", path.display())));
        }
        let (body, sum) = bytes.split_at(bytes.len() - 32);
        let actual = Hasher::new().update(body).finalize();
        if actual.as_bytes() != sum {
            return Err(Error::Invalid(format!(
                "index checksum mismatch at {}",
                path.display()
            )));
        }
        let list: Vec<IndexEntry> = bincode::deserialize(&body[4..])?;
        Ok(Index {
            entries: list.into_iter().map(|e| (e.path.clone(), e)).collect(),
        })
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let list: Vec<&IndexEntry> = self.entries.values().collect();
        let mut body = Vec::with_capacity(list.len() * 96 + 4);
        body.extend_from_slice(MAGIC);
        bincode::serialize_into(&mut body, &list)?;
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
        self.entries.insert(entry.path.clone(), entry);
    }

    pub fn remove(&mut self, path: &str) -> Option<IndexEntry> {
        self.entries.remove(path)
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
        builder.write(store)
    }

    /// Replace the index with the contents of `tree_id` (stat info cleared,
    /// so the next status will re-hash every file once).
    pub fn from_tree(store: &ObjectStore, tree_id: &ObjectId) -> Result<Index> {
        let mut idx = Index::new();
        let mut stack = vec![(String::new(), *tree_id)];
        while let Some((prefix, tid)) = stack.pop() {
            let tree = store.read_tree(&tid)?;
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

    pub fn write(mut self, store: &ObjectStore) -> Result<ObjectId> {
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
        root.write(store)
    }
}

#[derive(Default)]
struct Node {
    files: Vec<TreeEntry>,
    dirs: BTreeMap<String, Node>,
}

impl Node {
    fn write(self, store: &ObjectStore) -> Result<ObjectId> {
        let mut entries = self.files;
        for (name, node) in self.dirs {
            let count = node.files.len() + node.dirs.len();
            let id = node.write(store)?;
            entries.push(TreeEntry {
                name,
                mode: EntryMode::Dir,
                id,
                size: count as u64,
            });
        }
        store.write_tree(&Tree::from_entries(entries))
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
        let again = Index::from_tree(&store, &tree_id).unwrap();
        assert_eq!(again.len(), 3);
        assert_eq!(again.write_tree(&store).unwrap(), tree_id);
    }
}
