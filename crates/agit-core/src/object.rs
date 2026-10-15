//! The three object kinds: blob, tree, commit.
//!
//! Encoding is deliberately simple and self-describing:
//! * blobs are raw bytes,
//! * trees are a compact binary record list (sorted by name, bytewise),
//! * commits are canonical JSON so agents can read them without a parser
//!   beyond what they already have.
//!
//! The object id is `blake3(kind_tag || payload)`; the kind tag prevents a
//! blob from ever colliding with a tree of identical bytes.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::hash::{Hasher, ObjectId, HASH_LEN};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ObjectKind {
    Blob = 1,
    Tree = 2,
    Commit = 3,
}

impl ObjectKind {
    pub fn tag(self) -> u8 {
        self as u8
    }
    pub fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            1 => Some(ObjectKind::Blob),
            2 => Some(ObjectKind::Tree),
            3 => Some(ObjectKind::Commit),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            ObjectKind::Blob => "blob",
            ObjectKind::Tree => "tree",
            ObjectKind::Commit => "commit",
        }
    }
}

impl std::fmt::Display for ObjectKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// Compute the id of an object from its kind and payload bytes.
pub fn hash_object(kind: ObjectKind, payload: &[u8]) -> ObjectId {
    let mut h = Hasher::new();
    h.update(&[kind.tag()]);
    h.update(payload);
    h.finalize()
}

// ---------------------------------------------------------------------------
// Tree
// ---------------------------------------------------------------------------

/// File mode. Only the distinctions that matter for a VCS are kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum EntryMode {
    File,
    Executable,
    Symlink,
    Dir,
    /// A linked repository (submodule). `id` is the commit id pinned in the
    /// linked repository; the URL lives in `.agitmodules`.
    Submodule,
}

impl EntryMode {
    pub fn code(self) -> u32 {
        match self {
            EntryMode::File => 0o100644,
            EntryMode::Executable => 0o100755,
            EntryMode::Symlink => 0o120000,
            EntryMode::Dir => 0o040000,
            EntryMode::Submodule => 0o160000,
        }
    }
    pub fn from_code(code: u32) -> Option<Self> {
        match code {
            0o100644 => Some(EntryMode::File),
            0o100755 => Some(EntryMode::Executable),
            0o120000 => Some(EntryMode::Symlink),
            0o040000 => Some(EntryMode::Dir),
            0o160000 => Some(EntryMode::Submodule),
            _ => None,
        }
    }
    pub fn is_dir(self) -> bool {
        matches!(self, EntryMode::Dir)
    }
    pub fn is_submodule(self) -> bool {
        matches!(self, EntryMode::Submodule)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeEntry {
    pub name: String,
    pub mode: EntryMode,
    pub id: ObjectId,
    /// Size of the blob, or the number of entries for a directory.
    /// Stored so `status`/`ls` never need to open blobs.
    pub size: u64,
}

/// A directory snapshot. Entries are kept sorted by name (bytewise).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tree {
    entries: Vec<TreeEntry>,
}

impl Tree {
    pub fn new() -> Self {
        Tree::default()
    }

    pub fn from_entries(mut entries: Vec<TreeEntry>) -> Self {
        entries.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
        entries.dedup_by(|a, b| a.name == b.name);
        Tree { entries }
    }

    pub fn entries(&self) -> &[TreeEntry] {
        &self.entries
    }

    pub fn into_entries(self) -> Vec<TreeEntry> {
        self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn get(&self, name: &str) -> Option<&TreeEntry> {
        self.entries
            .binary_search_by(|e| e.name.as_bytes().cmp(name.as_bytes()))
            .ok()
            .map(|i| &self.entries[i])
    }

    pub fn insert(&mut self, entry: TreeEntry) {
        match self
            .entries
            .binary_search_by(|e| e.name.as_bytes().cmp(entry.name.as_bytes()))
        {
            Ok(i) => self.entries[i] = entry,
            Err(i) => self.entries.insert(i, entry),
        }
    }

    pub fn remove(&mut self, name: &str) -> Option<TreeEntry> {
        self.entries
            .binary_search_by(|e| e.name.as_bytes().cmp(name.as_bytes()))
            .ok()
            .map(|i| self.entries.remove(i))
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.entries.len() * 64);
        out.extend_from_slice(b"AGT1");
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for e in &self.entries {
            out.extend_from_slice(&e.mode.code().to_le_bytes());
            out.extend_from_slice(e.id.as_bytes());
            out.extend_from_slice(&e.size.to_le_bytes());
            let name = e.name.as_bytes();
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(name);
        }
        out
    }

    pub fn decode(id: ObjectId, bytes: &[u8]) -> Result<Self> {
        let corrupt = |reason: &str| Error::CorruptObject {
            id,
            reason: reason.to_string(),
        };
        if bytes.len() < 8 || &bytes[..4] != b"AGT1" {
            return Err(corrupt("bad tree magic"));
        }
        let count = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
        let mut pos = 8;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            if bytes.len() < pos + 4 + HASH_LEN + 8 + 2 {
                return Err(corrupt("truncated tree entry"));
            }
            let mode_code = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
            pos += 4;
            let mode = EntryMode::from_code(mode_code).ok_or_else(|| corrupt("bad entry mode"))?;
            let oid = ObjectId::from_slice(&bytes[pos..pos + HASH_LEN])?;
            pos += HASH_LEN;
            let size = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
            pos += 8;
            let name_len = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap()) as usize;
            pos += 2;
            if bytes.len() < pos + name_len {
                return Err(corrupt("truncated entry name"));
            }
            let name = std::str::from_utf8(&bytes[pos..pos + name_len])
                .map_err(|_| corrupt("entry name is not utf-8"))?
                .to_string();
            pos += name_len;
            entries.push(TreeEntry {
                name,
                mode,
                id: oid,
                size,
            });
        }
        if pos != bytes.len() {
            return Err(corrupt("trailing bytes after tree entries"));
        }
        // Trust but verify ordering so lookups stay correct.
        for w in entries.windows(2) {
            if w[0].name.as_bytes() >= w[1].name.as_bytes() {
                return Err(corrupt("tree entries not sorted"));
            }
        }
        Ok(Tree { entries })
    }
}

// ---------------------------------------------------------------------------
// Commit
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature {
    pub name: String,
    pub email: String,
}

/// Identity of the agent (or tool) that produced a commit. Entirely optional
/// for human commits; first-class for agentic workflows so that provenance is
/// queryable instead of buried in free text.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentInfo {
    /// Stable agent identifier, e.g. `planner-1` or `claude-code`.
    pub id: String,
    /// Model name/version, if applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Run / session / trace identifier for correlating with external logs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Human (or agent) that this agent acts on behalf of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_behalf_of: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Commit {
    pub tree: ObjectId,
    #[serde(default)]
    pub parents: Vec<ObjectId>,
    pub author: Signature,
    /// Unix time in milliseconds (UTC).
    pub time_ms: i64,
    pub message: String,
    /// Short statement of *why* this change exists. Distinct from the message
    /// so tooling can show goals separately from descriptions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
    /// External task / ticket / issue identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentInfo>,
    /// Checkpoints are cheap, disposable snapshots agents take between
    /// "real" commits. They can be squashed or garbage collected freely.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub checkpoint: bool,
    /// Free-form, schema-less extension point. Keys should be namespaced
    /// (`tool.name`), values must be JSON.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub meta: BTreeMap<String, serde_json::Value>,
    /// Detached signature over the canonical bytes of this commit with
    /// `signature` removed. See [`crate::sign`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<CommitSignature>,
}

/// Ed25519 signature attached to a commit. The public key travels with the
/// commit so verification needs no lookup; trust (which key belongs to which
/// agent) is decided by the server's key registry and policies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitSignature {
    /// Always `ed25519` for now.
    pub alg: String,
    /// Short id: first 16 hex chars of blake3(public_key).
    pub key_id: String,
    /// Hex-encoded 32-byte public key.
    pub public_key: String,
    /// Who claims the key: agent id or author name.
    pub signer: String,
    /// Hex-encoded 64-byte signature.
    pub sig: String,
}

impl Commit {
    /// Canonical bytes that a signature covers (the commit without its
    /// `signature` field).
    pub fn signable_bytes(&self) -> Vec<u8> {
        let mut c = self.clone();
        c.signature = None;
        c.encode()
    }

    pub fn encode(&self) -> Vec<u8> {
        // serde_json on a struct emits fields in declaration order, which is
        // deterministic. Maps are BTreeMaps so key order is canonical too.
        serde_json::to_vec(self).expect("commit serialization cannot fail")
    }

    pub fn decode(id: ObjectId, bytes: &[u8]) -> Result<Self> {
        serde_json::from_slice(bytes).map_err(|e| Error::CorruptObject {
            id,
            reason: format!("bad commit json: {e}"),
        })
    }

    pub fn summary(&self) -> &str {
        self.message.lines().next().unwrap_or("")
    }
}

// ---------------------------------------------------------------------------
// Unified object
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum Object {
    Blob(Vec<u8>),
    Tree(Tree),
    Commit(Commit),
}

impl Object {
    pub fn kind(&self) -> ObjectKind {
        match self {
            Object::Blob(_) => ObjectKind::Blob,
            Object::Tree(_) => ObjectKind::Tree,
            Object::Commit(_) => ObjectKind::Commit,
        }
    }

    pub fn encode(&self) -> std::borrow::Cow<'_, [u8]> {
        match self {
            Object::Blob(b) => std::borrow::Cow::Borrowed(b.as_slice()),
            Object::Tree(t) => std::borrow::Cow::Owned(t.encode()),
            Object::Commit(c) => std::borrow::Cow::Owned(c.encode()),
        }
    }

    pub fn id(&self) -> ObjectId {
        hash_object(self.kind(), &self.encode())
    }

    pub fn decode(id: ObjectId, kind: ObjectKind, payload: Vec<u8>) -> Result<Self> {
        Ok(match kind {
            ObjectKind::Blob => Object::Blob(payload),
            ObjectKind::Tree => Object::Tree(Tree::decode(id, &payload)?),
            ObjectKind::Commit => Object::Commit(Commit::decode(id, &payload)?),
        })
    }

    pub fn into_tree(self) -> Result<Tree> {
        match self {
            Object::Tree(t) => Ok(t),
            other => Err(Error::Invalid(format!(
                "expected tree, found {}",
                other.kind()
            ))),
        }
    }

    pub fn into_commit(self) -> Result<Commit> {
        match self {
            Object::Commit(c) => Ok(c),
            other => Err(Error::Invalid(format!(
                "expected commit, found {}",
                other.kind()
            ))),
        }
    }

    pub fn into_blob(self) -> Result<Vec<u8>> {
        match self {
            Object::Blob(b) => Ok(b),
            other => Err(Error::Invalid(format!(
                "expected blob, found {}",
                other.kind()
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_roundtrip_and_sorted() {
        let mut t = Tree::new();
        t.insert(TreeEntry {
            name: "b.txt".into(),
            mode: EntryMode::File,
            id: ObjectId::ZERO,
            size: 3,
        });
        t.insert(TreeEntry {
            name: "a".into(),
            mode: EntryMode::Dir,
            id: ObjectId::ZERO,
            size: 1,
        });
        let bytes = t.encode();
        let back = Tree::decode(ObjectId::ZERO, &bytes).unwrap();
        assert_eq!(t, back);
        assert_eq!(back.entries()[0].name, "a");
        assert!(back.get("b.txt").is_some());
    }

    #[test]
    fn commit_roundtrip_is_deterministic() {
        let c = Commit {
            tree: ObjectId::ZERO,
            parents: vec![],
            author: Signature {
                name: "a".into(),
                email: "a@x".into(),
            },
            time_ms: 42,
            message: "hi".into(),
            intent: Some("test".into()),
            task: None,
            agent: Some(AgentInfo {
                id: "agent-1".into(),
                ..Default::default()
            }),
            checkpoint: false,
            meta: BTreeMap::new(),
            signature: None,
        };
        let a = c.encode();
        let b = Commit::decode(ObjectId::ZERO, &a).unwrap().encode();
        assert_eq!(a, b);
        assert_ne!(
            hash_object(ObjectKind::Blob, &a),
            hash_object(ObjectKind::Commit, &a)
        );
    }
}

#[cfg(test)]
mod format_fuzz {
    use super::*;

    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.0 >> 11
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n.max(1)
        }
    }

    fn random_tree(rng: &mut Lcg) -> Tree {
        let n = rng.below(40) as usize;
        let mut entries = Vec::new();
        for i in 0..n {
            let len = 1 + rng.below(30) as usize;
            let name: String = (0..len)
                .map(|_| char::from(b'a' + rng.below(26) as u8))
                .collect();
            let mut id = [0u8; 32];
            for b in id.iter_mut() {
                *b = rng.below(256) as u8;
            }
            let mode = match rng.below(5) {
                0 => EntryMode::Dir,
                1 => EntryMode::Executable,
                2 => EntryMode::Symlink,
                3 => EntryMode::Submodule,
                _ => EntryMode::File,
            };
            entries.push(TreeEntry {
                name: format!("{name}{i}"),
                mode,
                id: ObjectId::from_bytes(id),
                size: rng.next(),
            });
        }
        Tree::from_entries(entries)
    }

    #[test]
    fn tree_roundtrip_random() {
        let mut rng = Lcg(42);
        for _ in 0..200 {
            let t = random_tree(&mut rng);
            let bytes = t.encode();
            let back = Tree::decode(ObjectId::ZERO, &bytes).unwrap();
            assert_eq!(t, back);
            assert_eq!(back.encode(), bytes);
        }
    }

    #[test]
    fn tree_decode_never_panics_on_garbage() {
        let mut rng = Lcg(7);
        for _ in 0..300 {
            let t = random_tree(&mut rng);
            let mut bytes = t.encode();
            match rng.below(3) {
                0 => {
                    let cut = rng.below(bytes.len() as u64 + 1) as usize;
                    bytes.truncate(cut);
                }
                1 => {
                    if !bytes.is_empty() {
                        let i = rng.below(bytes.len() as u64) as usize;
                        bytes[i] ^= 1 << rng.below(8);
                    }
                }
                _ => {
                    bytes.extend_from_slice(b"junk");
                }
            }
            // Either decodes to something consistent or errors; never panics.
            if let Ok(d) = Tree::decode(ObjectId::ZERO, &bytes) {
                let _ = d.encode();
            }
        }
    }

    #[test]
    fn commit_roundtrip_random_unicode() {
        let mut rng = Lcg(99);
        for _ in 0..100 {
            let len = rng.below(200) as usize;
            let msg: String = (0..len)
                .map(|_| char::from_u32(0x20 + rng.below(0x2000) as u32).unwrap_or('x'))
                .collect();
            let c = Commit {
                tree: ObjectId::ZERO,
                parents: (0..rng.below(4))
                    .map(|i| ObjectId::from_bytes([i as u8; 32]))
                    .collect(),
                author: Signature {
                    name: msg.chars().take(10).collect(),
                    email: "e@x".into(),
                },
                time_ms: rng.next() as i64,
                message: msg.clone(),
                intent: if rng.below(2) == 0 {
                    Some(msg.chars().rev().collect())
                } else {
                    None
                },
                task: None,
                agent: None,
                checkpoint: rng.below(2) == 0,
                meta: [("k".to_string(), serde_json::json!({ "s": msg }))]
                    .into_iter()
                    .collect(),
                signature: None,
            };
            let bytes = c.encode();
            let back = Commit::decode(ObjectId::ZERO, &bytes).unwrap();
            assert_eq!(c, back);
            assert_eq!(back.encode(), bytes);
        }
    }
}
