//! Content-addressed object store.
//!
//! Layout: `objects/<2 hex>/<62 hex>`. Each file is:
//!
//! ```text
//! "AGO1" | kind:u8 | raw_len:u64le | zstd(payload)
//! ```
//!
//! Writes go to a temp file in the same directory and are renamed into
//! place, so a reader never observes a partial object. Reads re-hash the
//! payload and refuse to return data that does not match its id.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::error::{io_at, Error, Result};
use crate::hash::ObjectId;
use crate::object::{hash_object, Commit, Object, ObjectKind, Tree};

const MAGIC: &[u8; 4] = b"AGO1";
const HEADER_LEN: usize = 4 + 1 + 8;
const ZSTD_LEVEL: i32 = 3;

#[derive(Debug, Clone)]
pub struct ObjectStore {
    root: PathBuf,
}

impl ObjectStore {
    pub fn open(root: impl Into<PathBuf>) -> Self {
        ObjectStore { root: root.into() }
    }

    pub fn init(root: impl Into<PathBuf>) -> Result<Self> {
        let s = ObjectStore::open(root);
        fs::create_dir_all(&s.root).map_err(io_at(&s.root))?;
        Ok(s)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn path_for(&self, id: &ObjectId) -> PathBuf {
        let hex = id.to_hex();
        self.root.join(&hex[..2]).join(&hex[2..])
    }

    pub fn contains(&self, id: &ObjectId) -> bool {
        self.path_for(id).is_file()
    }

    /// Store `payload` as an object of `kind`. Idempotent.
    pub fn write(&self, kind: ObjectKind, payload: &[u8]) -> Result<ObjectId> {
        let id = hash_object(kind, payload);
        let path = self.path_for(&id);
        if path.exists() {
            return Ok(id);
        }
        let dir = path.parent().unwrap();
        fs::create_dir_all(dir).map_err(io_at(dir))?;

        let mut tmp = tempfile::Builder::new()
            .prefix(".tmp-")
            .tempfile_in(dir)
            .map_err(io_at(dir))?;
        {
            let f = tmp.as_file_mut();
            f.write_all(MAGIC)?;
            f.write_all(&[kind.tag()])?;
            f.write_all(&(payload.len() as u64).to_le_bytes())?;
            let mut enc = zstd::Encoder::new(f, ZSTD_LEVEL)?;
            enc.write_all(payload)?;
            let f = enc.finish()?;
            if crate::durability::sync_data() {
                f.sync_data()?;
            }
        }
        match tmp.persist_noclobber(&path) {
            Ok(_) => {}
            Err(e) if path.exists() => {
                // Lost a race with a concurrent writer of the same object.
                drop(e);
            }
            Err(e) => return Err(Error::IoAt { path, source: e.error }),
        }
        Ok(id)
    }

    pub fn write_object(&self, obj: &Object) -> Result<ObjectId> {
        self.write(obj.kind(), &obj.encode())
    }

    pub fn write_tree(&self, tree: &Tree) -> Result<ObjectId> {
        self.write(ObjectKind::Tree, &tree.encode())
    }

    pub fn write_commit(&self, commit: &Commit) -> Result<ObjectId> {
        self.write(ObjectKind::Commit, &commit.encode())
    }

    /// Hash a file on disk as a blob and store it. Returns (id, size).
    pub fn write_blob_from_path(&self, path: &Path) -> Result<(ObjectId, u64)> {
        let data = fs::read(path).map_err(io_at(path))?;
        let id = self.write(ObjectKind::Blob, &data)?;
        Ok((id, data.len() as u64))
    }

    /// Hash file contents without storing anything (used by `status`).
    pub fn hash_file(path: &Path) -> Result<(ObjectId, u64)> {
        let data = fs::read(path).map_err(io_at(path))?;
        Ok((hash_object(ObjectKind::Blob, &data), data.len() as u64))
    }

    fn read_header(&self, id: &ObjectId, f: &mut File) -> Result<(ObjectKind, u64)> {
        let mut hdr = [0u8; HEADER_LEN];
        f.read_exact(&mut hdr).map_err(|_| Error::CorruptObject {
            id: *id,
            reason: "short header".into(),
        })?;
        if &hdr[..4] != MAGIC {
            return Err(Error::CorruptObject {
                id: *id,
                reason: "bad magic".into(),
            });
        }
        let kind = ObjectKind::from_tag(hdr[4]).ok_or_else(|| Error::CorruptObject {
            id: *id,
            reason: "unknown kind".into(),
        })?;
        let len = u64::from_le_bytes(hdr[5..13].try_into().unwrap());
        Ok((kind, len))
    }

    pub fn read_raw(&self, id: &ObjectId) -> Result<(ObjectKind, Vec<u8>)> {
        let path = self.path_for(id);
        let mut f = match File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::ObjectNotFound(*id))
            }
            Err(e) => return Err(Error::IoAt { path, source: e }),
        };
        let (kind, len) = self.read_header(id, &mut f)?;
        let mut payload = Vec::with_capacity(len as usize);
        zstd::Decoder::new(f)?.read_to_end(&mut payload)?;
        if payload.len() as u64 != len {
            return Err(Error::CorruptObject {
                id: *id,
                reason: format!("length mismatch: header {len}, payload {}", payload.len()),
            });
        }
        let actual = hash_object(kind, &payload);
        if actual != *id {
            return Err(Error::HashMismatch {
                expected: *id,
                actual,
            });
        }
        Ok((kind, payload))
    }

    pub fn kind(&self, id: &ObjectId) -> Result<ObjectKind> {
        let path = self.path_for(id);
        let mut f = File::open(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::ObjectNotFound(*id)
            } else {
                Error::IoAt { path, source: e }
            }
        })?;
        Ok(self.read_header(id, &mut f)?.0)
    }

    pub fn read(&self, id: &ObjectId) -> Result<Object> {
        let (kind, payload) = self.read_raw(id)?;
        Object::decode(*id, kind, payload)
    }

    pub fn read_tree(&self, id: &ObjectId) -> Result<Tree> {
        self.read(id)?.into_tree()
    }

    pub fn read_commit(&self, id: &ObjectId) -> Result<Commit> {
        self.read(id)?.into_commit()
    }

    pub fn read_blob(&self, id: &ObjectId) -> Result<Vec<u8>> {
        self.read(id)?.into_blob()
    }

    /// Resolve an unambiguous hex prefix to a full id.
    pub fn resolve_prefix(&self, prefix: &str) -> Result<ObjectId> {
        if prefix.len() < 4 || !prefix.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(Error::UnknownRevision(prefix.to_string()));
        }
        let prefix = prefix.to_ascii_lowercase();
        if prefix.len() == 64 {
            let id: ObjectId = prefix.parse()?;
            return if self.contains(&id) {
                Ok(id)
            } else {
                Err(Error::UnknownRevision(prefix))
            };
        }
        let dir = self.root.join(&prefix[..2]);
        let rest = &prefix[2..];
        let mut found = Vec::new();
        if let Ok(rd) = fs::read_dir(&dir) {
            for entry in rd.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(rest) && name.len() == 62 {
                    found.push(format!("{}{}", &prefix[..2], name));
                }
            }
        }
        match found.len() {
            0 => Err(Error::UnknownRevision(prefix)),
            1 => found[0].parse(),
            _ => Err(Error::AmbiguousRevision(prefix)),
        }
    }

    /// All object ids in the store (slow; for fsck/gc/debugging).
    pub fn iter_ids(&self) -> Result<Vec<ObjectId>> {
        let mut out = Vec::new();
        for shard in fs::read_dir(&self.root).map_err(io_at(&self.root))?.flatten() {
            let shard_name = shard.file_name().to_string_lossy().to_string();
            if shard_name.len() != 2 {
                continue;
            }
            for obj in fs::read_dir(shard.path()).map_err(io_at(shard.path()))?.flatten() {
                let name = obj.file_name().to_string_lossy().to_string();
                if name.len() == 62 {
                    if let Ok(id) = format!("{shard_name}{name}").parse() {
                        out.push(id);
                    }
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_read_verify() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::init(dir.path().join("objects")).unwrap();
        let id = store.write(ObjectKind::Blob, b"hello world").unwrap();
        let id2 = store.write(ObjectKind::Blob, b"hello world").unwrap();
        assert_eq!(id, id2);
        assert_eq!(store.read_blob(&id).unwrap(), b"hello world");
        assert_eq!(store.kind(&id).unwrap(), ObjectKind::Blob);
        assert_eq!(store.resolve_prefix(&id.to_hex()[..8]).unwrap(), id);
    }

    #[test]
    fn detects_tampering() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::init(dir.path().join("objects")).unwrap();
        let id = store.write(ObjectKind::Blob, b"hello world").unwrap();
        let other = store.write(ObjectKind::Blob, b"evil").unwrap();
        fs::copy(store.path_for(&other), store.path_for(&id)).unwrap();
        assert!(matches!(store.read_blob(&id), Err(Error::HashMismatch { .. })));
    }

    #[test]
    fn missing_object() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::init(dir.path().join("objects")).unwrap();
        assert!(matches!(
            store.read(&ObjectId::ZERO),
            Err(Error::ObjectNotFound(_))
        ));
    }
}
