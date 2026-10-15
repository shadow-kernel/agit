//! Commit signing with Ed25519.
//!
//! Keys live in `~/.agit/keys/<name>.key` (32-byte secret, hex, mode 0600)
//! with the public key beside it in `<name>.pub`. A signature covers the
//! canonical commit bytes without the `signature` field, so any party can
//! verify a commit offline; which keys are *trusted* for which agent is the
//! server's key registry's job.

use std::path::{Path, PathBuf};

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::error::{io_at, Error, Result};
use crate::object::{Commit, CommitSignature};

pub const ALG: &str = "ed25519";

pub fn key_id(public_key: &[u8; 32]) -> String {
    blake3::hash(public_key).to_hex()[..16].to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublicKeyInfo {
    pub name: String,
    pub key_id: String,
    pub public_key: String,
}

pub struct Keypair {
    pub name: String,
    signing: SigningKey,
}

impl Keypair {
    pub fn generate(name: &str) -> Keypair {
        Keypair {
            name: name.to_string(),
            signing: SigningKey::generate(&mut rand_core::OsRng),
        }
    }

    pub fn from_secret_hex(name: &str, hex_secret: &str) -> Result<Keypair> {
        let bytes = hex::decode(hex_secret.trim())
            .map_err(|_| Error::Invalid("bad secret key encoding".into()))?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| Error::Invalid("secret key must be 32 bytes".into()))?;
        Ok(Keypair {
            name: name.to_string(),
            signing: SigningKey::from_bytes(&arr),
        })
    }

    pub fn public_key_hex(&self) -> String {
        hex::encode(self.signing.verifying_key().to_bytes())
    }

    pub fn key_id(&self) -> String {
        key_id(&self.signing.verifying_key().to_bytes())
    }

    pub fn info(&self) -> PublicKeyInfo {
        PublicKeyInfo {
            name: self.name.clone(),
            key_id: self.key_id(),
            public_key: self.public_key_hex(),
        }
    }

    pub fn sign_commit(&self, commit: &Commit, signer: &str) -> CommitSignature {
        let sig = self.signing.sign(&commit.signable_bytes());
        CommitSignature {
            alg: ALG.to_string(),
            key_id: self.key_id(),
            public_key: self.public_key_hex(),
            signer: signer.to_string(),
            sig: hex::encode(sig.to_bytes()),
        }
    }

    // -- storage -----------------------------------------------------------

    pub fn keys_dir() -> PathBuf {
        if let Ok(d) = std::env::var("AGIT_KEYS_DIR") {
            return PathBuf::from(d);
        }
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".into());
        Path::new(&home).join(".agit").join("keys")
    }

    pub fn path_for(name: &str) -> PathBuf {
        Self::keys_dir().join(format!("{name}.key"))
    }

    pub fn exists(name: &str) -> bool {
        Self::path_for(name).is_file()
    }

    pub fn save(&self) -> Result<PathBuf> {
        let dir = Self::keys_dir();
        std::fs::create_dir_all(&dir).map_err(io_at(&dir))?;
        let path = Self::path_for(&self.name);
        if path.exists() {
            return Err(Error::AlreadyExists(path));
        }
        std::fs::write(&path, format!("{}\n", hex::encode(self.signing.to_bytes())))
            .map_err(io_at(&path))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .map_err(io_at(&path))?;
        }
        let pub_path = dir.join(format!("{}.pub", self.name));
        std::fs::write(
            &pub_path,
            format!("{} {} {}\n", ALG, self.public_key_hex(), self.name),
        )
        .map_err(io_at(&pub_path))?;
        Ok(path)
    }

    pub fn load(name: &str) -> Result<Keypair> {
        let path = Self::path_for(name);
        let text = std::fs::read_to_string(&path)
            .map_err(|_| Error::NotFound(format!("signing key {name} ({})", path.display())))?;
        Keypair::from_secret_hex(name, &text)
    }

    pub fn list() -> Result<Vec<PublicKeyInfo>> {
        let dir = Self::keys_dir();
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "key") {
                    let name = p.file_stem().unwrap().to_string_lossy().to_string();
                    if let Ok(k) = Keypair::load(&name) {
                        out.push(k.info());
                    }
                }
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    pub fn delete(name: &str) -> Result<()> {
        let p = Self::path_for(name);
        std::fs::remove_file(&p).map_err(|_| Error::NotFound(format!("signing key {name}")))?;
        let _ = std::fs::remove_file(Self::keys_dir().join(format!("{name}.pub")));
        Ok(())
    }
}

/// Outcome of verifying a commit signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SignatureStatus {
    Unsigned,
    /// Cryptographically valid. Whether `key_id` is trusted for `signer`
    /// is a separate question (see the server key registry).
    Valid {
        key_id: String,
        signer: String,
        public_key: String,
    },
    Invalid {
        reason: String,
    },
}

impl SignatureStatus {
    pub fn is_valid(&self) -> bool {
        matches!(self, SignatureStatus::Valid { .. })
    }
}

/// Verify a commit's signature against the public key it carries.
pub fn verify_commit(commit: &Commit) -> SignatureStatus {
    let Some(s) = &commit.signature else {
        return SignatureStatus::Unsigned;
    };
    if s.alg != ALG {
        return SignatureStatus::Invalid {
            reason: format!("unsupported algorithm {}", s.alg),
        };
    }
    let pk_bytes = match hex::decode(&s.public_key)
        .ok()
        .and_then(|b| <[u8; 32]>::try_from(b).ok())
    {
        Some(b) => b,
        None => {
            return SignatureStatus::Invalid {
                reason: "malformed public key".into(),
            }
        }
    };
    if key_id(&pk_bytes) != s.key_id {
        return SignatureStatus::Invalid {
            reason: "key id does not match public key".into(),
        };
    }
    let vk = match VerifyingKey::from_bytes(&pk_bytes) {
        Ok(k) => k,
        Err(_) => {
            return SignatureStatus::Invalid {
                reason: "invalid public key".into(),
            }
        }
    };
    let sig_bytes = match hex::decode(&s.sig)
        .ok()
        .and_then(|b| <[u8; 64]>::try_from(b).ok())
    {
        Some(b) => b,
        None => {
            return SignatureStatus::Invalid {
                reason: "malformed signature".into(),
            }
        }
    };
    let sig = Signature::from_bytes(&sig_bytes);
    match vk.verify(&commit.signable_bytes(), &sig) {
        Ok(()) => SignatureStatus::Valid {
            key_id: s.key_id.clone(),
            signer: s.signer.clone(),
            public_key: s.public_key.clone(),
        },
        Err(_) => SignatureStatus::Invalid {
            reason: "signature does not match commit".into(),
        },
    }
}

/// Verify that a hex public key is well formed; returns its key id.
pub fn validate_public_key(public_key_hex: &str) -> Result<String> {
    let bytes = hex::decode(public_key_hex.trim())
        .map_err(|_| Error::Invalid("public key must be hex".into()))?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| Error::Invalid("public key must be 32 bytes".into()))?;
    VerifyingKey::from_bytes(&arr)
        .map_err(|_| Error::Invalid("invalid ed25519 public key".into()))?;
    Ok(key_id(&arr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::ObjectId;
    use crate::object::Signature as Sig;

    fn commit() -> Commit {
        Commit {
            tree: ObjectId::ZERO,
            parents: vec![],
            author: Sig {
                name: "a".into(),
                email: "a@x".into(),
            },
            time_ms: 1,
            message: "m".into(),
            intent: Some("i".into()),
            task: None,
            agent: None,
            checkpoint: false,
            meta: Default::default(),
            signature: None,
        }
    }

    #[test]
    fn sign_verify_tamper() {
        let k = Keypair::generate("t");
        let mut c = commit();
        c.signature = Some(k.sign_commit(&c, "agent-t"));
        assert!(verify_commit(&c).is_valid());
        // The id covers the signature too, but the signature covers the rest.
        let mut tampered = c.clone();
        tampered.message = "evil".into();
        assert!(matches!(
            verify_commit(&tampered),
            SignatureStatus::Invalid { .. }
        ));
        let mut wrong_key = c.clone();
        wrong_key.signature.as_mut().unwrap().public_key = Keypair::generate("o").public_key_hex();
        assert!(matches!(
            verify_commit(&wrong_key),
            SignatureStatus::Invalid { .. }
        ));
        assert_eq!(verify_commit(&commit()), SignatureStatus::Unsigned);
        // roundtrip through encode/decode keeps it valid
        let back = Commit::decode(ObjectId::ZERO, &c.encode()).unwrap();
        assert!(verify_commit(&back).is_valid());
    }

    #[test]
    fn key_storage() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("AGIT_KEYS_DIR", dir.path());
        let k = Keypair::generate("alpha");
        k.save().unwrap();
        assert!(Keypair::save(&Keypair::generate("alpha")).is_err());
        let l = Keypair::load("alpha").unwrap();
        assert_eq!(l.public_key_hex(), k.public_key_hex());
        assert_eq!(Keypair::list().unwrap().len(), 1);
        assert_eq!(
            validate_public_key(&k.public_key_hex()).unwrap(),
            k.key_id()
        );
        Keypair::delete("alpha").unwrap();
        assert!(Keypair::list().unwrap().is_empty());
        std::env::remove_var("AGIT_KEYS_DIR");
    }
}
