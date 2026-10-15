//! Object stream: `kind:u8 | len:u32 big-endian | payload`, repeated.

use agit_core::object::ObjectKind;
use agit_core::store::ObjectStore;
use agit_core::{Error, ObjectId, Result};

pub const CONTENT_TYPE: &str = "application/x-agit-objects";

pub fn encode_frame(out: &mut Vec<u8>, kind: ObjectKind, payload: &[u8]) {
    out.push(kind.tag());
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
}

pub struct Frame<'a> {
    pub kind: ObjectKind,
    pub payload: &'a [u8],
}

pub struct Decoder<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Decoder<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Decoder { bytes, pos: 0 }
    }
}

impl<'a> Iterator for Decoder<'a> {
    type Item = Result<Frame<'a>>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.bytes.len() {
            return None;
        }
        let rest = &self.bytes[self.pos..];
        if rest.len() < 5 {
            self.pos = self.bytes.len();
            return Some(Err(Error::Invalid("truncated object stream header".into())));
        }
        let kind = match ObjectKind::from_tag(rest[0]) {
            Some(k) => k,
            None => {
                self.pos = self.bytes.len();
                return Some(Err(Error::Invalid(format!("bad object kind {}", rest[0]))));
            }
        };
        let len = u32::from_be_bytes(rest[1..5].try_into().unwrap()) as usize;
        if rest.len() < 5 + len {
            self.pos = self.bytes.len();
            return Some(Err(Error::Invalid(
                "truncated object stream payload".into(),
            )));
        }
        let payload = &rest[5..5 + len];
        self.pos += 5 + len;
        Some(Ok(Frame { kind, payload }))
    }
}

/// Store every frame of a stream. Returns (stored, skipped).
pub fn import(store: &ObjectStore, bytes: &[u8]) -> Result<(usize, usize)> {
    let (mut stored, mut skipped) = (0, 0);
    for frame in Decoder::new(bytes) {
        let f = frame?;
        let id = agit_core::object::hash_object(f.kind, f.payload);
        if store.contains(&id) {
            skipped += 1;
        } else {
            store.write(f.kind, f.payload)?;
            stored += 1;
        }
    }
    Ok((stored, skipped))
}

/// Encode the given objects from a store into a stream.
pub fn export(store: &ObjectStore, ids: &[ObjectId]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for id in ids {
        let (kind, payload) = store.read_raw(id)?;
        encode_frame(&mut out, kind, &payload);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let a = ObjectStore::init(dir.path().join("a")).unwrap();
        let b = ObjectStore::init(dir.path().join("b")).unwrap();
        let id1 = a.write(ObjectKind::Blob, b"one").unwrap();
        let id2 = a.write(ObjectKind::Blob, b"two").unwrap();
        let bytes = export(&a, &[id1, id2]).unwrap();
        assert_eq!(import(&b, &bytes).unwrap(), (2, 0));
        assert_eq!(import(&b, &bytes).unwrap(), (0, 2));
        assert_eq!(b.read_blob(&id1).unwrap(), b"one");
    }
}
