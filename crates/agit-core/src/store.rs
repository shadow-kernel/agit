//! Content-addressed object store: pack segments plus legacy loose objects.
//!
//! Writes go to an append-only **segment** owned by this process
//! (`objects/pack/seg-<writer>.agp`). Objects become visible to other
//! processes only when [`ObjectStore::flush`] publishes the segment's index
//! (`.idx`, written atomically). `Repository` flushes before every ref update
//! and index save, so a reference never points at an unpublished object.
//! Crash before flush: the appended bytes are garbage at the tail of a segment
//! that nothing references; the next flush rewrites the index from what the
//! writer knows, so they are simply ignored.
//!
//! Segment record: `kind:u8 | raw_len:u32le | comp_len:u32le | id:32 | zstd`.
//! Index: `"AGX1" | count:u32le | entries sorted by id
//! (`id:32 | offset:u64le | comp_len:u32le | raw_len:u32le | kind:u8`) |
//! blake3 checksum:32`.
//!
//! Loose objects (`objects/xx/yyyy…`, format `AGO1`) are still read, so
//! repositories written by earlier versions keep working; `gc` repacks them.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use crate::error::{io_at, Error, Result};
use crate::hash::{Hasher, ObjectId, HASH_LEN};
use crate::object::{hash_object, Commit, Object, ObjectKind, Tree};

const LOOSE_MAGIC: &[u8; 4] = b"AGO1";
const LOOSE_HEADER_LEN: usize = 4 + 1 + 8;
const SEG_MAGIC: &[u8; 4] = b"AGP1";
const IDX_MAGIC: &[u8; 4] = b"AGX1";
const REC_HEADER_LEN: usize = 1 + 4 + 4 + HASH_LEN;
const IDX_ENTRY_LEN: usize = HASH_LEN + 8 + 4 + 4 + 1;
const ZSTD_LEVEL: i32 = 3;
/// Rescan the pack directory for other writers' segments at most this often.
const RESCAN_INTERVAL: Duration = Duration::from_millis(50);
/// Seal a segment and start a new one beyond these limits, so each index
/// rewrite on flush stays small even in long-running processes.
const SEGMENT_MAX_OBJECTS: usize = 1024;
const SEGMENT_MAX_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy)]
struct Entry {
    offset: u64,
    comp_len: u32,
    raw_len: u32,
    kind: ObjectKind,
}

struct PackIndex {
    name: String,
    idx_len: u64,
    entries: HashMap<ObjectId, Entry>,
    file: Mutex<Option<File>>,
}

struct SegmentWriter {
    name: String,
    file: File,
    offset: u64,
    /// Objects appended since the last flush (plus all earlier ones; the
    /// index is always rewritten in full from this map).
    entries: HashMap<ObjectId, Entry>,
    dirty: bool,
    _lock: crate::lock::Lock,
}

struct Inner {
    root: PathBuf,
    pack_dir: PathBuf,
    packs: RwLock<HashMap<String, Arc<PackIndex>>>,
    writer: Mutex<Option<SegmentWriter>>,
    /// (time of last scan, pack dir mtime seen then).
    last_scan: Mutex<Option<(Instant, Option<std::time::SystemTime>)>>,
    use_packs: bool,
}

#[derive(Clone)]
pub struct ObjectStore {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for ObjectStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObjectStore")
            .field("root", &self.inner.root)
            .finish()
    }
}

fn use_packs_from_env() -> bool {
    !matches!(
        std::env::var("AGIT_PACK").as_deref(),
        Ok("0") | Ok("off") | Ok("false")
    )
}

impl ObjectStore {
    pub fn open(root: impl Into<PathBuf>) -> Self {
        let root: PathBuf = root.into();
        ObjectStore {
            inner: Arc::new(Inner {
                pack_dir: root.join("pack"),
                root,
                packs: RwLock::new(HashMap::new()),
                writer: Mutex::new(None),
                last_scan: Mutex::new(None),
                use_packs: use_packs_from_env(),
            }),
        }
    }

    pub fn init(root: impl Into<PathBuf>) -> Result<Self> {
        let s = ObjectStore::open(root);
        fs::create_dir_all(&s.inner.pack_dir).map_err(io_at(&s.inner.pack_dir))?;
        Ok(s)
    }

    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    fn loose_path(&self, id: &ObjectId) -> PathBuf {
        let hex = id.to_hex();
        self.inner.root.join(&hex[..2]).join(&hex[2..])
    }

    // -- pack discovery -----------------------------------------------------

    /// Load or refresh indexes of all published segments.
    ///
    /// A publish renames a new `.idx` into the pack directory, which bumps
    /// the directory's mtime, so one `stat` tells whether anything changed.
    /// On file systems with coarse mtimes the time-based rescan interval is
    /// the fallback.
    fn scan_packs(&self, force: bool) -> Result<()> {
        let dir_mtime = fs::metadata(&self.inner.pack_dir)
            .ok()
            .and_then(|m| m.modified().ok());
        {
            let mut last = self.inner.last_scan.lock().unwrap();
            if !force {
                if let Some((at, seen)) = *last {
                    let unchanged = dir_mtime.is_some() && seen == dir_mtime;
                    if unchanged && at.elapsed() < RESCAN_INTERVAL {
                        return Ok(());
                    }
                    if unchanged
                        && dir_mtime
                            .is_some_and(|m| m.elapsed().is_ok_and(|e| e > Duration::from_secs(2)))
                    {
                        // mtime is old and unchanged: nothing new was published.
                        *last = Some((Instant::now(), dir_mtime));
                        return Ok(());
                    }
                }
            }
            *last = Some((Instant::now(), dir_mtime));
        }
        let rd = match fs::read_dir(&self.inner.pack_dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                return Err(Error::IoAt {
                    path: self.inner.pack_dir.clone(),
                    source: e,
                })
            }
        };
        let own = self
            .inner
            .writer
            .lock()
            .unwrap()
            .as_ref()
            .map(|w| w.name.clone());
        let mut seen = Vec::new();
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_none_or(|x| x != "idx") {
                continue;
            }
            let name = p.file_stem().unwrap().to_string_lossy().to_string();
            if own.as_deref() == Some(name.as_str()) {
                continue; // our own segment is served from the writer map
            }
            let len = e.metadata().map(|m| m.len()).unwrap_or(0);
            seen.push(name.clone());
            let needs_load = {
                let packs = self.inner.packs.read().unwrap();
                packs.get(&name).is_none_or(|pi| pi.idx_len != len)
            };
            if needs_load {
                match self.load_index(&name, &p, len) {
                    Ok(pi) => {
                        self.inner.packs.write().unwrap().insert(name, Arc::new(pi));
                    }
                    Err(err) => {
                        // A writer may be replacing the index right now; retry next scan.
                        let _ = err;
                    }
                }
            }
        }
        // Drop indexes of segments that disappeared (repacked).
        let mut packs = self.inner.packs.write().unwrap();
        packs.retain(|n, _| seen.contains(n));
        Ok(())
    }

    fn load_index(&self, name: &str, idx_path: &Path, idx_len: u64) -> Result<PackIndex> {
        let bytes = fs::read(idx_path).map_err(io_at(idx_path))?;
        let corrupt =
            |why: &str| Error::Invalid(format!("corrupt pack index {}: {why}", idx_path.display()));
        if bytes.len() < 8 + HASH_LEN || &bytes[..4] != IDX_MAGIC {
            return Err(corrupt("bad magic"));
        }
        let (body, sum) = bytes.split_at(bytes.len() - HASH_LEN);
        if Hasher::new().update(body).finalize().as_bytes() != sum {
            return Err(corrupt("checksum mismatch"));
        }
        let count = u32::from_le_bytes(body[4..8].try_into().unwrap()) as usize;
        if body.len() != 8 + count * IDX_ENTRY_LEN {
            return Err(corrupt("length mismatch"));
        }
        let mut entries = HashMap::with_capacity(count);
        for i in 0..count {
            let e = &body[8 + i * IDX_ENTRY_LEN..8 + (i + 1) * IDX_ENTRY_LEN];
            let id = ObjectId::from_slice(&e[..HASH_LEN])?;
            let offset = u64::from_le_bytes(e[32..40].try_into().unwrap());
            let comp_len = u32::from_le_bytes(e[40..44].try_into().unwrap());
            let raw_len = u32::from_le_bytes(e[44..48].try_into().unwrap());
            let kind = ObjectKind::from_tag(e[48]).ok_or_else(|| corrupt("bad kind"))?;
            entries.insert(
                id,
                Entry {
                    offset,
                    comp_len,
                    raw_len,
                    kind,
                },
            );
        }
        Ok(PackIndex {
            name: name.to_string(),
            idx_len,
            entries,
            file: Mutex::new(None),
        })
    }

    fn segment_path(&self, name: &str) -> PathBuf {
        self.inner.pack_dir.join(format!("{name}.agp"))
    }

    fn idx_path(&self, name: &str) -> PathBuf {
        self.inner.pack_dir.join(format!("{name}.idx"))
    }

    /// Locate an object: own writer, published packs (rescanning when
    /// missing), then loose storage.
    fn locate(&self, id: &ObjectId) -> Option<Location> {
        if let Some(w) = self.inner.writer.lock().unwrap().as_ref() {
            if let Some(e) = w.entries.get(id) {
                return Some(Location::Own(*e));
            }
        }
        if let Some(l) = self.locate_in_packs(id) {
            return Some(l);
        }
        let _ = self.scan_packs(false);
        if let Some(l) = self.locate_in_packs(id) {
            return Some(l);
        }
        if self.loose_path(id).is_file() {
            return Some(Location::Loose);
        }
        None
    }

    fn locate_in_packs(&self, id: &ObjectId) -> Option<Location> {
        let packs = self.inner.packs.read().unwrap();
        for p in packs.values() {
            if let Some(e) = p.entries.get(id) {
                return Some(Location::Pack(p.clone(), *e));
            }
        }
        None
    }

    pub fn contains(&self, id: &ObjectId) -> bool {
        self.locate(id).is_some()
    }

    // -- writing ------------------------------------------------------------

    fn open_writer(&self) -> Result<std::sync::MutexGuard<'_, Option<SegmentWriter>>> {
        let mut guard = self.inner.writer.lock().unwrap();
        if guard.is_none() {
            fs::create_dir_all(&self.inner.pack_dir).map_err(io_at(&self.inner.pack_dir))?;
            let name = format!(
                "seg-{}-{}",
                std::process::id(),
                &uuid::Uuid::new_v4().simple().to_string()[..8]
            );
            let lock = crate::lock::Lock::acquire(
                &self.inner.pack_dir.join(format!("{name}.lock")),
                Duration::from_secs(1),
            )?;
            let path = self.segment_path(&name);
            let mut file = OpenOptions::new()
                .create_new(true)
                .append(true)
                .read(true)
                .open(&path)
                .map_err(io_at(&path))?;
            file.write_all(SEG_MAGIC).map_err(io_at(&path))?;
            *guard = Some(SegmentWriter {
                name,
                file,
                offset: 4,
                entries: HashMap::new(),
                dirty: false,
                _lock: lock,
            });
        }
        Ok(guard)
    }

    /// Store `payload` as an object of `kind`. Idempotent.
    pub fn write(&self, kind: ObjectKind, payload: &[u8]) -> Result<ObjectId> {
        let id = hash_object(kind, payload);
        if self.contains(&id) {
            return Ok(id);
        }
        if !self.inner.use_packs {
            return self.write_loose(id, kind, payload);
        }
        self.append(id, kind, payload)
    }

    /// Append to our segment without checking other locations (used by
    /// `write` after its existence check and by `repack`, which must copy
    /// objects that still exist in the segments it is about to delete).
    fn append(&self, id: ObjectId, kind: ObjectKind, payload: &[u8]) -> Result<ObjectId> {
        let compressed = zstd::bulk::compress(payload, ZSTD_LEVEL)?;
        let mut guard = self.open_writer()?;
        let w = guard.as_mut().unwrap();
        if w.entries.contains_key(&id) {
            return Ok(id);
        }
        let mut rec = Vec::with_capacity(REC_HEADER_LEN + compressed.len());
        rec.push(kind.tag());
        rec.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        rec.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        rec.extend_from_slice(id.as_bytes());
        rec.extend_from_slice(&compressed);
        w.file.write_all(&rec)?;
        let offset = w.offset + REC_HEADER_LEN as u64;
        w.offset += rec.len() as u64;
        w.entries.insert(
            id,
            Entry {
                offset,
                comp_len: compressed.len() as u32,
                raw_len: payload.len() as u32,
                kind,
            },
        );
        w.dirty = true;
        Ok(id)
    }

    fn write_loose(&self, id: ObjectId, kind: ObjectKind, payload: &[u8]) -> Result<ObjectId> {
        let path = self.loose_path(&id);
        let dir = path.parent().unwrap();
        fs::create_dir_all(dir).map_err(io_at(dir))?;
        let mut tmp = tempfile::Builder::new()
            .prefix(".tmp-")
            .tempfile_in(dir)
            .map_err(io_at(dir))?;
        {
            let f = tmp.as_file_mut();
            f.write_all(LOOSE_MAGIC)?;
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
            Err(e) if path.exists() => drop(e),
            Err(e) => {
                return Err(Error::IoAt {
                    path,
                    source: e.error,
                })
            }
        }
        Ok(id)
    }

    fn encode_index(entries: &HashMap<ObjectId, Entry>) -> Vec<u8> {
        let mut ids: Vec<(&ObjectId, &Entry)> = entries.iter().collect();
        ids.sort_by(|a, b| a.0.cmp(b.0));
        let mut body = Vec::with_capacity(8 + ids.len() * IDX_ENTRY_LEN + HASH_LEN);
        body.extend_from_slice(IDX_MAGIC);
        body.extend_from_slice(&(ids.len() as u32).to_le_bytes());
        for (id, e) in ids {
            body.extend_from_slice(id.as_bytes());
            body.extend_from_slice(&e.offset.to_le_bytes());
            body.extend_from_slice(&e.comp_len.to_le_bytes());
            body.extend_from_slice(&e.raw_len.to_le_bytes());
            body.push(e.kind.tag());
        }
        let sum = Hasher::new().update(&body).finalize();
        body.extend_from_slice(sum.as_bytes());
        body
    }

    fn publish_writer(pack_dir: &Path, w: &mut SegmentWriter) -> Result<()> {
        if !w.dirty {
            return Ok(());
        }
        if crate::durability::sync_data() {
            w.file.sync_data()?;
        }
        let body = Self::encode_index(&w.entries);
        crate::refs::write_atomic_bulk(&pack_dir.join(format!("{}.idx", w.name)), &body)?;
        w.dirty = false;
        Ok(())
    }

    /// Publish everything written so far: sync the segment (per durability
    /// level) and atomically rewrite its index so other processes see it.
    pub fn flush(&self) -> Result<()> {
        let mut guard = self.inner.writer.lock().unwrap();
        if let Some(w) = guard.as_mut() {
            Self::publish_writer(&self.inner.pack_dir, w)?;
            if w.entries.len() >= SEGMENT_MAX_OBJECTS || w.offset >= SEGMENT_MAX_BYTES {
                self.seal(&mut guard);
            }
        }
        Ok(())
    }

    /// Turn the current writer into a read-only pack and start fresh on the
    /// next write. The index was just published, so nothing becomes invisible.
    fn seal(&self, guard: &mut std::sync::MutexGuard<'_, Option<SegmentWriter>>) {
        if let Some(w) = guard.take() {
            let name = w.name.clone();
            let entries = w.entries.clone();
            let idx_len = fs::metadata(self.inner.pack_dir.join(format!("{name}.idx")))
                .map(|m| m.len())
                .unwrap_or(0);
            drop(w);
            self.inner.packs.write().unwrap().insert(
                name.clone(),
                Arc::new(PackIndex {
                    name,
                    idx_len,
                    entries,
                    file: Mutex::new(None),
                }),
            );
        }
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

    // -- reading ------------------------------------------------------------

    fn read_at(file: &File, offset: u64, len: usize) -> std::io::Result<Vec<u8>> {
        let mut buf = vec![0u8; len];
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileExt;
            file.read_exact_at(&mut buf, offset)?;
        }
        #[cfg(not(unix))]
        {
            use std::io::{Seek, SeekFrom};
            let mut f = file.try_clone()?;
            f.seek(SeekFrom::Start(offset))?;
            f.read_exact(&mut buf)?;
        }
        Ok(buf)
    }

    fn read_entry(&self, id: &ObjectId, loc: Location) -> Result<(ObjectKind, Vec<u8>)> {
        let (kind, compressed, raw_len) = match loc {
            Location::Own(e) => {
                let guard = self.inner.writer.lock().unwrap();
                match guard.as_ref() {
                    // The writer may have been sealed by another thread
                    // between locate and read; the segment is now a pack.
                    Some(w) if w.entries.contains_key(id) => (
                        e.kind,
                        Self::read_at(&w.file, e.offset, e.comp_len as usize)?,
                        e.raw_len,
                    ),
                    _ => {
                        drop(guard);
                        let _ = self.scan_packs(true);
                        return match self.locate_in_packs(id) {
                            Some(loc) => self.read_entry(id, loc),
                            None => Err(Error::ObjectNotFound(*id)),
                        };
                    }
                }
            }
            Location::Pack(p, e) => {
                let mut f = p.file.lock().unwrap();
                if f.is_none() {
                    let path = self.segment_path(&p.name);
                    *f = Some(File::open(&path).map_err(io_at(&path))?);
                }
                (
                    e.kind,
                    Self::read_at(f.as_ref().unwrap(), e.offset, e.comp_len as usize)?,
                    e.raw_len,
                )
            }
            Location::Loose => return self.read_loose(id),
        };
        let payload = zstd::bulk::decompress(&compressed, raw_len as usize)?;
        if payload.len() != raw_len as usize {
            return Err(Error::CorruptObject {
                id: *id,
                reason: "length mismatch in pack".into(),
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

    fn read_loose(&self, id: &ObjectId) -> Result<(ObjectKind, Vec<u8>)> {
        let path = self.loose_path(id);
        let mut f = match File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::ObjectNotFound(*id))
            }
            Err(e) => return Err(Error::IoAt { path, source: e }),
        };
        let (kind, len) = self.read_loose_header(id, &mut f)?;
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

    fn read_loose_header(&self, id: &ObjectId, f: &mut File) -> Result<(ObjectKind, u64)> {
        let mut hdr = [0u8; LOOSE_HEADER_LEN];
        f.read_exact(&mut hdr).map_err(|_| Error::CorruptObject {
            id: *id,
            reason: "short header".into(),
        })?;
        if &hdr[..4] != LOOSE_MAGIC {
            return Err(Error::CorruptObject {
                id: *id,
                reason: "bad magic".into(),
            });
        }
        let kind = ObjectKind::from_tag(hdr[4]).ok_or_else(|| Error::CorruptObject {
            id: *id,
            reason: "unknown kind".into(),
        })?;
        Ok((kind, u64::from_le_bytes(hdr[5..13].try_into().unwrap())))
    }

    pub fn read_raw(&self, id: &ObjectId) -> Result<(ObjectKind, Vec<u8>)> {
        match self.locate(id) {
            Some(loc) => self.read_entry(id, loc),
            None => Err(Error::ObjectNotFound(*id)),
        }
    }

    pub fn kind(&self, id: &ObjectId) -> Result<ObjectKind> {
        match self.locate(id) {
            Some(Location::Own(e)) | Some(Location::Pack(_, e)) => Ok(e.kind),
            Some(Location::Loose) => {
                let path = self.loose_path(id);
                let mut f = File::open(&path).map_err(|e| Error::IoAt { path, source: e })?;
                Ok(self.read_loose_header(id, &mut f)?.0)
            }
            None => Err(Error::ObjectNotFound(*id)),
        }
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

    // -- enumeration ----------------------------------------------------------

    /// All object ids (own writer, published packs, loose).
    pub fn iter_ids(&self) -> Result<Vec<ObjectId>> {
        self.scan_packs(true)?;
        let mut out: Vec<ObjectId> = Vec::new();
        if let Some(w) = self.inner.writer.lock().unwrap().as_ref() {
            out.extend(w.entries.keys().copied());
        }
        for p in self.inner.packs.read().unwrap().values() {
            out.extend(p.entries.keys().copied());
        }
        if let Ok(rd) = fs::read_dir(&self.inner.root) {
            for shard in rd.flatten() {
                let shard_name = shard.file_name().to_string_lossy().to_string();
                if shard_name.len() != 2 {
                    continue;
                }
                for obj in fs::read_dir(shard.path())
                    .map_err(io_at(shard.path()))?
                    .flatten()
                {
                    let name = obj.file_name().to_string_lossy().to_string();
                    if name.len() == 62 {
                        if let Ok(id) = format!("{shard_name}{name}").parse() {
                            out.push(id);
                        }
                    }
                }
            }
        }
        out.sort();
        out.dedup();
        Ok(out)
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
        let mut found: Vec<ObjectId> = self
            .iter_ids()?
            .into_iter()
            .filter(|id| id.to_hex().starts_with(&prefix))
            .collect();
        found.dedup();
        match found.len() {
            0 => Err(Error::UnknownRevision(prefix)),
            1 => Ok(found[0]),
            _ => Err(Error::AmbiguousRevision(prefix)),
        }
    }

    // -- maintenance ------------------------------------------------------------

    /// Seconds since the object was written (segment or loose file mtime).
    pub fn age_secs(&self, id: &ObjectId) -> Option<u64> {
        let path = match self.locate(id)? {
            Location::Own(_) => return Some(0),
            Location::Pack(p, _) => self.segment_path(&p.name),
            Location::Loose => self.loose_path(id),
        };
        let m = fs::metadata(path).ok()?.modified().ok()?;
        std::time::SystemTime::now()
            .duration_since(m)
            .ok()
            .map(|d| d.as_secs())
    }

    /// Stored (compressed) size in bytes.
    pub fn disk_size(&self, id: &ObjectId) -> u64 {
        match self.locate(id) {
            Some(Location::Own(e)) | Some(Location::Pack(_, e)) => {
                e.comp_len as u64 + REC_HEADER_LEN as u64
            }
            Some(Location::Loose) => fs::metadata(self.loose_path(id))
                .map(|m| m.len())
                .unwrap_or(0),
            None => 0,
        }
    }

    /// Is the object stored loose (true) or in a segment (false)?
    pub fn is_loose(&self, id: &ObjectId) -> bool {
        matches!(self.locate(id), Some(Location::Loose))
    }

    /// Delete a loose object. Packed objects are removed by [`repack`](Self::repack).
    pub fn remove(&self, id: &ObjectId) -> Result<()> {
        let path = self.loose_path(id);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::IoAt { path, source: e }),
        }
    }

    /// Rewrite all segments whose writers are gone (and all loose objects)
    /// into one fresh segment containing only `live` objects. Returns
    /// (objects dropped, segments removed).
    pub fn repack(&self, live: &std::collections::HashSet<ObjectId>) -> Result<(usize, usize)> {
        self.flush()?;
        // Retire our own writer so its segment can be compacted as well; a
        // fresh segment is opened for the surviving objects.
        {
            let mut guard = self.inner.writer.lock().unwrap();
            self.seal(&mut guard);
        }
        self.scan_packs(true)?;
        // Segments we may take over: lock acquirable => writer is gone.
        let mut victims: Vec<(String, crate::lock::Lock)> = Vec::new();
        let names: Vec<String> = self.inner.packs.read().unwrap().keys().cloned().collect();
        for name in names {
            let lock_path = self.inner.pack_dir.join(format!("{name}.lock"));
            if let Ok(l) = crate::lock::Lock::acquire(&lock_path, Duration::from_millis(10)) {
                victims.push((name, l));
            }
        }
        let loose: Vec<ObjectId> = self
            .iter_ids()?
            .into_iter()
            .filter(|id| self.is_loose(id))
            .collect();
        if victims.is_empty() && loose.is_empty() {
            return Ok((0, 0));
        }
        let mut dropped = 0usize;
        let mut moved = 0usize;
        // Copy live objects into our own writer.
        let victim_names: Vec<String> = victims.iter().map(|(n, _)| n.clone()).collect();
        let packs: Vec<Arc<PackIndex>> = {
            let map = self.inner.packs.read().unwrap();
            victim_names
                .iter()
                .filter_map(|n| map.get(n).cloned())
                .collect()
        };
        for p in &packs {
            let ids: Vec<ObjectId> = p.entries.keys().copied().collect();
            for id in ids {
                if !live.contains(&id) {
                    dropped += 1;
                    continue;
                }
                let e = p.entries[&id];
                let (kind, payload) = self.read_entry(&id, Location::Pack(p.clone(), e))?;
                self.append(id, kind, &payload)?;
                moved += 1;
            }
        }
        for id in &loose {
            if live.contains(id) {
                let (kind, payload) = self.read_loose(id)?;
                self.append(*id, kind, &payload)?;
                moved += 1;
            } else {
                dropped += 1;
            }
        }
        let _ = moved;
        self.flush()?;
        // Remove the old segments and loose files.
        for (name, lock) in victims {
            let _ = fs::remove_file(self.idx_path(&name));
            let _ = fs::remove_file(self.segment_path(&name));
            drop(lock);
            let _ = fs::remove_file(self.inner.pack_dir.join(format!("{name}.lock")));
            self.inner.packs.write().unwrap().remove(&name);
        }
        for id in &loose {
            self.remove(id)?;
        }
        Ok((dropped, victim_names.len()))
    }
}

enum Location {
    Own(Entry),
    Pack(Arc<PackIndex>, Entry),
    Loose,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // Best effort: publish what this process wrote.
        if let Ok(mut guard) = self.writer.lock() {
            if let Some(w) = guard.as_mut() {
                let _ = ObjectStore::publish_writer(&self.pack_dir, w);
            }
        }
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
    fn visibility_requires_flush() {
        let dir = tempfile::tempdir().unwrap();
        let a = ObjectStore::init(dir.path().join("objects")).unwrap();
        let id = a.write(ObjectKind::Blob, b"shared").unwrap();
        let b = ObjectStore::open(dir.path().join("objects"));
        assert!(
            !b.contains(&id),
            "unpublished object must stay invisible to other handles"
        );
        a.flush().unwrap();
        std::thread::sleep(RESCAN_INTERVAL);
        assert!(b.contains(&id));
        assert_eq!(b.read_blob(&id).unwrap(), b"shared");
        // Second flush with more objects updates the same index.
        let id2 = a.write(ObjectKind::Blob, b"more").unwrap();
        a.flush().unwrap();
        std::thread::sleep(RESCAN_INTERVAL);
        assert_eq!(b.read_blob(&id2).unwrap(), b"more");
        assert_eq!(b.iter_ids().unwrap().len(), 2);
    }

    #[test]
    fn detects_tampering_in_segment() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::init(dir.path().join("objects")).unwrap();
        let id = store.write(ObjectKind::Blob, b"hello world").unwrap();
        store.flush().unwrap();
        let name = store
            .inner
            .writer
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .name
            .clone();
        drop(store);
        let seg = dir.path().join("objects/pack").join(format!("{name}.agp"));
        let mut bytes = fs::read(&seg).unwrap();
        let n = bytes.len();
        bytes[n - 1] ^= 0xff;
        fs::write(&seg, bytes).unwrap();
        let store = ObjectStore::open(dir.path().join("objects"));
        assert!(matches!(
            store.read_blob(&id),
            Err(Error::HashMismatch { .. }) | Err(Error::Io(_)) | Err(Error::CorruptObject { .. })
        ));
    }

    #[test]
    fn loose_objects_still_readable_and_repacked() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::init(dir.path().join("objects")).unwrap();
        let id = store
            .write_loose(
                hash_object(ObjectKind::Blob, b"old"),
                ObjectKind::Blob,
                b"old",
            )
            .unwrap();
        assert!(store.is_loose(&id));
        assert_eq!(store.read_blob(&id).unwrap(), b"old");
        let dead = store
            .write_loose(
                hash_object(ObjectKind::Blob, b"dead"),
                ObjectKind::Blob,
                b"dead",
            )
            .unwrap();
        let live: std::collections::HashSet<ObjectId> = [id].into_iter().collect();
        let (dropped, _) = store.repack(&live).unwrap();
        assert_eq!(dropped, 1);
        assert!(!store.is_loose(&id));
        assert_eq!(store.read_blob(&id).unwrap(), b"old");
        assert!(!store.contains(&dead));
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

#[cfg(test)]
mod segment_fuzz {
    use super::*;

    #[test]
    fn random_blobs_roundtrip_across_handles_and_repack() {
        let dir = tempfile::tempdir().unwrap();
        let a = ObjectStore::init(dir.path().join("objects")).unwrap();
        let mut state = 12345u64;
        let mut next = || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            state >> 11
        };
        let mut blobs: Vec<(ObjectId, Vec<u8>)> = Vec::new();
        for i in 0..1500 {
            let len = match i % 7 {
                0 => 0,
                1 => 1,
                2 => next() as usize % 64,
                3 => next() as usize % 4096,
                4 => 70_000 + next() as usize % 1000,
                _ => next() as usize % 300,
            };
            let data: Vec<u8> = (0..len).map(|_| (next() & 0xff) as u8).collect();
            let id = a.write(ObjectKind::Blob, &data).unwrap();
            blobs.push((id, data));
        }
        a.flush().unwrap();
        // Segments rolled (1500 > 1024) and everything reads back from a fresh handle.
        std::thread::sleep(RESCAN_INTERVAL);
        let b = ObjectStore::open(dir.path().join("objects"));
        for (id, data) in &blobs {
            assert_eq!(&b.read_blob(id).unwrap(), data);
        }
        assert_eq!(
            b.iter_ids().unwrap().len(),
            blobs
                .iter()
                .map(|(i, _)| *i)
                .collect::<std::collections::HashSet<_>>()
                .len()
        );
        // Repack keeping half; the kept half still reads, the rest is gone.
        let live: std::collections::HashSet<ObjectId> =
            blobs.iter().step_by(2).map(|(i, _)| *i).collect();
        let (dropped, segs) = b.repack(&live).unwrap();
        assert!(segs >= 1);
        assert!(dropped > 0);
        for (k, (id, data)) in blobs.iter().enumerate() {
            if k % 2 == 0 {
                assert_eq!(&b.read_blob(id).unwrap(), data);
            } else if !live.contains(id) {
                assert!(!b.contains(id));
            }
        }
    }
}
