//! Object identifiers.
//!
//! agit uses BLAKE3 (256-bit). It is cryptographically secure, several times
//! faster than SHA-1/SHA-256 on a single core, and parallelizes over large
//! inputs, which matters when agents snapshot big generated files.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

pub const HASH_LEN: usize = 32;

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectId([u8; HASH_LEN]);

impl ObjectId {
    pub const ZERO: ObjectId = ObjectId([0u8; HASH_LEN]);

    pub fn from_bytes(bytes: [u8; HASH_LEN]) -> Self {
        ObjectId(bytes)
    }

    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != HASH_LEN {
            return Err(Error::InvalidObjectId(hex::encode(bytes)));
        }
        let mut out = [0u8; HASH_LEN];
        out.copy_from_slice(bytes);
        Ok(ObjectId(out))
    }

    pub fn as_bytes(&self) -> &[u8; HASH_LEN] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// Short prefix for human display (agents should always use the full id).
    pub fn short(&self) -> String {
        hex::encode(&self.0[..6])
    }

    pub fn is_zero(&self) -> bool {
        self.0 == [0u8; HASH_LEN]
    }
}

impl fmt::Debug for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ObjectId({})", self.to_hex())
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for ObjectId {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        let bytes = hex::decode(s).map_err(|_| Error::InvalidObjectId(s.to_string()))?;
        ObjectId::from_slice(&bytes)
    }
}

impl Serialize for ObjectId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        if s.is_human_readable() {
            s.serialize_str(&self.to_hex())
        } else {
            s.serialize_bytes(&self.0)
        }
    }
}

impl<'de> Deserialize<'de> for ObjectId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        if d.is_human_readable() {
            let s = String::deserialize(d)?;
            s.parse().map_err(serde::de::Error::custom)
        } else {
            let b: Vec<u8> = serde_bytes_compat::deserialize(d)?;
            ObjectId::from_slice(&b).map_err(serde::de::Error::custom)
        }
    }
}

mod serde_bytes_compat {
    use serde::de::{Deserializer, Visitor};
    use std::fmt;

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Vec<u8>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("bytes")
            }
            fn visit_bytes<E>(self, v: &[u8]) -> Result<Vec<u8>, E> {
                Ok(v.to_vec())
            }
            fn visit_byte_buf<E>(self, v: Vec<u8>) -> Result<Vec<u8>, E> {
                Ok(v)
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Vec<u8>, A::Error> {
                let mut out = Vec::with_capacity(seq.size_hint().unwrap_or(32));
                while let Some(b) = seq.next_element::<u8>()? {
                    out.push(b);
                }
                Ok(out)
            }
        }
        d.deserialize_bytes(V)
    }
}

/// Incremental hasher used by the object store.
pub struct Hasher(blake3::Hasher);

impl Hasher {
    pub fn new() -> Self {
        Hasher(blake3::Hasher::new())
    }
    pub fn update(&mut self, data: &[u8]) -> &mut Self {
        if data.len() >= 128 * 1024 {
            self.0.update_rayon(data);
        } else {
            self.0.update(data);
        }
        self
    }
    pub fn finalize(&self) -> ObjectId {
        ObjectId(*self.0.finalize().as_bytes())
    }
}

impl Default for Hasher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_hex() {
        let id = Hasher::new().update(b"hello").finalize();
        let parsed: ObjectId = id.to_hex().parse().unwrap();
        assert_eq!(id, parsed);
        assert_eq!(id.to_hex().len(), 64);
    }

    #[test]
    fn rejects_bad_hex() {
        assert!("zz".parse::<ObjectId>().is_err());
        assert!("abcd".parse::<ObjectId>().is_err());
    }
}
