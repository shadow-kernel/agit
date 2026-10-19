//! OpenSSH Ed25519 keys for server authentication.
//!
//! agit authenticates clients with the SSH keys people already have
//! (`ssh-keygen -t ed25519`). The server keeps a registry of authorized
//! public keys with scopes; a client proves possession of the private key by
//! signing a single-use server challenge. This module parses the OpenSSH
//! public-key line (`ssh-ed25519 AAAA… comment`), the `openssh-key-v1`
//! private-key file and agit's own hex key format, computes OpenSSH-style
//! fingerprints (`SHA256:…`, as `ssh-keygen -lf` prints them) and signs or
//! verifies challenge messages. Encrypted private keys are recognised (their
//! public half is stored in the clear) so a caller can fall back to ssh-agent.

use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use base64::Engine;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

/// The only key type agit accepts for authentication.
pub const KEY_TYPE: &str = "ssh-ed25519";
/// Domain separator for challenge signatures. Commit signatures cover JSON
/// (starting with `{`), so the two can never be confused.
pub const AUTH_DOMAIN: &str = "agit-auth-v1";
const OPENSSH_MAGIC: &[u8] = b"openssh-key-v1\0";
const PEM_BEGIN: &str = "-----BEGIN OPENSSH PRIVATE KEY-----";
const PEM_END: &str = "-----END OPENSSH PRIVATE KEY-----";

/// An Ed25519 public key in SSH form.
#[derive(Clone, PartialEq, Eq)]
pub struct SshPublicKey {
    bytes: [u8; 32],
    pub comment: Option<String>,
}

impl std::fmt::Debug for SshPublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SshPublicKey({})", self.fingerprint())
    }
}

impl SshPublicKey {
    pub fn from_bytes(bytes: [u8; 32], comment: Option<String>) -> Result<SshPublicKey> {
        VerifyingKey::from_bytes(&bytes)
            .map_err(|_| Error::Invalid("invalid ed25519 public key".into()))?;
        Ok(SshPublicKey {
            bytes,
            comment: comment.filter(|c| !c.is_empty()),
        })
    }

    /// Parse any of: an OpenSSH line `ssh-ed25519 <base64> [comment]`, a
    /// 64-hex raw key (agit's `key_id`/registry format) or agit's `.pub`
    /// line `ed25519 <hex> <name>`. Other key types are rejected with a hint.
    pub fn parse(text: &str) -> Result<SshPublicKey> {
        let text = text.trim();
        let mut parts = text.split_whitespace();
        let first = parts
            .next()
            .ok_or_else(|| Error::Invalid("empty public key".into()))?;
        if first == KEY_TYPE {
            let b64 = parts
                .next()
                .ok_or_else(|| Error::Invalid("ssh-ed25519 key is missing its data".into()))?;
            let blob = STANDARD
                .decode(b64)
                .map_err(|_| Error::Invalid("ssh public key is not valid base64".into()))?;
            let comment = {
                let rest: Vec<&str> = parts.collect();
                if rest.is_empty() {
                    None
                } else {
                    Some(rest.join(" "))
                }
            };
            let mut k = SshPublicKey::from_wire(&blob)?;
            k.comment = comment;
            return Ok(k);
        }
        if first.starts_with("ssh-") || first.starts_with("ecdsa-") || first.starts_with("sk-") {
            return Err(Error::Invalid(format!(
                "unsupported key type {first}: agit accepts only ssh-ed25519 keys \
                 (generate one with `ssh-keygen -t ed25519`)"
            )));
        }
        let (hex_key, comment) = if first == crate::sign::ALG {
            let h = parts
                .next()
                .ok_or_else(|| Error::Invalid("ed25519 key is missing its data".into()))?;
            (h, parts.next().map(str::to_string))
        } else {
            (first, None)
        };
        let bytes = hex::decode(hex_key).map_err(|_| {
            Error::Invalid(
                "public key must be an `ssh-ed25519 …` line or a 64-character hex key".into(),
            )
        })?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| Error::Invalid("public key must be 32 bytes".into()))?;
        SshPublicKey::from_bytes(arr, comment)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.bytes
    }

    /// Hex form, as used by agit's commit signatures and key registry.
    pub fn to_hex(&self) -> String {
        hex::encode(self.bytes)
    }

    /// Short id shared with signing keys (`blake3(public)[..16]`).
    pub fn key_id(&self) -> String {
        crate::sign::key_id(&self.bytes)
    }

    /// SSH wire encoding: `string "ssh-ed25519" | string key`.
    pub fn wire(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + KEY_TYPE.len() + 4 + 32);
        put_string(&mut out, KEY_TYPE.as_bytes());
        put_string(&mut out, &self.bytes);
        out
    }

    pub fn from_wire(blob: &[u8]) -> Result<SshPublicKey> {
        let mut cur = blob;
        let kt = read_string(&mut cur)?;
        if kt != KEY_TYPE.as_bytes() {
            return Err(Error::Invalid(format!(
                "unsupported key type {}: agit accepts only ssh-ed25519 keys",
                String::from_utf8_lossy(kt)
            )));
        }
        let key = read_string(&mut cur)?;
        let arr: [u8; 32] = key
            .try_into()
            .map_err(|_| Error::Invalid("ssh-ed25519 key must be 32 bytes".into()))?;
        SshPublicKey::from_bytes(arr, None)
    }

    /// `SHA256:<base64>` over the wire blob, identical to `ssh-keygen -lf`.
    pub fn fingerprint(&self) -> String {
        let digest = Sha256::digest(self.wire());
        format!("SHA256:{}", STANDARD_NO_PAD.encode(digest))
    }

    /// The OpenSSH one-line form (`ssh-ed25519 <base64> [comment]`).
    pub fn to_openssh(&self) -> String {
        let mut s = format!("{KEY_TYPE} {}", STANDARD.encode(self.wire()));
        if let Some(c) = &self.comment {
            s.push(' ');
            s.push_str(c);
        }
        s
    }

    pub fn verify(&self, message: &[u8], signature: &[u8]) -> bool {
        let Ok(vk) = VerifyingKey::from_bytes(&self.bytes) else {
            return false;
        };
        let Ok(sig) = <[u8; 64]>::try_from(signature) else {
            return false;
        };
        vk.verify(message, &Signature::from_bytes(&sig)).is_ok()
    }
}

/// A parsed private key file.
pub enum SshPrivateKey {
    /// Usable directly.
    Plain {
        signing: SigningKey,
        public: SshPublicKey,
    },
    /// Passphrase-protected: only the public half is readable. Sign through
    /// ssh-agent instead.
    Encrypted {
        public: SshPublicKey,
        cipher: String,
    },
}

impl std::fmt::Debug for SshPrivateKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SshPrivateKey::Plain { public, .. } => write!(f, "SshPrivateKey::Plain({public:?})"),
            SshPrivateKey::Encrypted { public, cipher } => {
                write!(f, "SshPrivateKey::Encrypted({public:?}, {cipher})")
            }
        }
    }
}

impl SshPrivateKey {
    /// Parse an `openssh-key-v1` PEM file or agit's 64-hex secret.
    pub fn parse(text: &str) -> Result<SshPrivateKey> {
        let text = text.trim();
        if text.starts_with(PEM_BEGIN) {
            return Self::parse_openssh(text);
        }
        if text.starts_with("-----BEGIN") {
            return Err(Error::Invalid(
                "only OpenSSH-format private keys are supported (ssh-keygen -t ed25519); \
                 PEM/PKCS8 keys are not"
                    .into(),
            ));
        }
        // agit `~/.agit/keys/<name>.key`: hex seed.
        let bytes = hex::decode(text)
            .map_err(|_| Error::Invalid("private key is neither OpenSSH nor hex".into()))?;
        let seed: [u8; 32] = bytes
            .try_into()
            .map_err(|_| Error::Invalid("secret key must be 32 bytes".into()))?;
        let signing = SigningKey::from_bytes(&seed);
        let public = SshPublicKey::from_bytes(signing.verifying_key().to_bytes(), None)?;
        Ok(SshPrivateKey::Plain { signing, public })
    }

    fn parse_openssh(text: &str) -> Result<SshPrivateKey> {
        let body: String = text
            .lines()
            .filter(|l| !l.starts_with("-----"))
            .map(str::trim)
            .collect();
        let bytes = STANDARD
            .decode(body)
            .map_err(|_| Error::Invalid("OpenSSH private key is not valid base64".into()))?;
        let mut cur = bytes.as_slice();
        if !cur.starts_with(OPENSSH_MAGIC) {
            return Err(Error::Invalid("not an openssh-key-v1 file".into()));
        }
        cur = &cur[OPENSSH_MAGIC.len()..];
        let cipher = String::from_utf8_lossy(read_string(&mut cur)?).to_string();
        let _kdf = read_string(&mut cur)?;
        let _kdf_opts = read_string(&mut cur)?;
        let nkeys = read_u32(&mut cur)?;
        if nkeys != 1 {
            return Err(Error::Invalid(format!(
                "OpenSSH private key file holds {nkeys} keys; expected 1"
            )));
        }
        let pub_blob = read_string(&mut cur)?;
        let public = SshPublicKey::from_wire(pub_blob)?;
        let private = read_string(&mut cur)?;
        if cipher != "none" {
            return Ok(SshPrivateKey::Encrypted { public, cipher });
        }
        let mut p = private;
        let c1 = read_u32(&mut p)?;
        let c2 = read_u32(&mut p)?;
        if c1 != c2 {
            return Err(Error::Invalid("OpenSSH private key is corrupt".into()));
        }
        let kt = read_string(&mut p)?;
        if kt != KEY_TYPE.as_bytes() {
            return Err(Error::Invalid("unsupported key type in private key".into()));
        }
        let pub_again = read_string(&mut p)?;
        let secret = read_string(&mut p)?;
        if secret.len() != 64 || pub_again != &public.bytes[..] || &secret[32..] != pub_again {
            return Err(Error::Invalid("OpenSSH private key is inconsistent".into()));
        }
        let seed: [u8; 32] = secret[..32].try_into().unwrap();
        let signing = SigningKey::from_bytes(&seed);
        let comment = read_string(&mut p)
            .ok()
            .map(|c| String::from_utf8_lossy(c).to_string())
            .filter(|c| !c.is_empty());
        Ok(SshPrivateKey::Plain {
            signing,
            public: SshPublicKey {
                bytes: public.bytes,
                comment,
            },
        })
    }

    pub fn public(&self) -> &SshPublicKey {
        match self {
            SshPrivateKey::Plain { public, .. } | SshPrivateKey::Encrypted { public, .. } => public,
        }
    }

    pub fn is_encrypted(&self) -> bool {
        matches!(self, SshPrivateKey::Encrypted { .. })
    }

    /// Sign `message`; `None` for encrypted keys.
    pub fn sign(&self, message: &[u8]) -> Option<[u8; 64]> {
        match self {
            SshPrivateKey::Plain { signing, .. } => Some(signing.sign(message).to_bytes()),
            SshPrivateKey::Encrypted { .. } => None,
        }
    }

    /// Serialize an unencrypted key in `openssh-key-v1` form (what
    /// `ssh-keygen -t ed25519 -N ''` writes).
    pub fn to_openssh(&self) -> Result<String> {
        let SshPrivateKey::Plain { signing, public } = self else {
            return Err(Error::Invalid("cannot re-encode an encrypted key".into()));
        };
        let mut out = Vec::new();
        out.extend_from_slice(OPENSSH_MAGIC);
        put_string(&mut out, b"none");
        put_string(&mut out, b"none");
        put_string(&mut out, b"");
        out.extend_from_slice(&1u32.to_be_bytes());
        put_string(&mut out, &public.wire());
        let mut priv_section = Vec::new();
        let check: u32 = u32::from_le_bytes(public.bytes[..4].try_into().unwrap());
        priv_section.extend_from_slice(&check.to_be_bytes());
        priv_section.extend_from_slice(&check.to_be_bytes());
        put_string(&mut priv_section, KEY_TYPE.as_bytes());
        put_string(&mut priv_section, &public.bytes);
        let mut secret = signing.to_bytes().to_vec();
        secret.extend_from_slice(&public.bytes);
        put_string(&mut priv_section, &secret);
        put_string(
            &mut priv_section,
            public.comment.as_deref().unwrap_or("").as_bytes(),
        );
        let mut pad = 1u8;
        while priv_section.len() % 8 != 0 {
            priv_section.push(pad);
            pad += 1;
        }
        put_string(&mut out, &priv_section);
        let b64 = STANDARD.encode(out);
        let mut s = String::from(PEM_BEGIN);
        s.push('\n');
        for chunk in b64.as_bytes().chunks(70) {
            s.push_str(std::str::from_utf8(chunk).unwrap());
            s.push('\n');
        }
        s.push_str(PEM_END);
        s.push('\n');
        Ok(s)
    }
}

/// The bytes a client signs to redeem a challenge.
pub fn auth_message(nonce: &str) -> Vec<u8> {
    format!("{AUTH_DOMAIN}\n{nonce}").into_bytes()
}

/// Extract the raw 64-byte signature from an SSH signature blob
/// (`string "ssh-ed25519" | string sig`), as returned by ssh-agent.
pub fn signature_from_wire(blob: &[u8]) -> Result<[u8; 64]> {
    let mut cur = blob;
    let kt = read_string(&mut cur)?;
    if kt != KEY_TYPE.as_bytes() {
        return Err(Error::Invalid(format!(
            "unexpected signature type {}",
            String::from_utf8_lossy(kt)
        )));
    }
    let sig = read_string(&mut cur)?;
    sig.try_into()
        .map_err(|_| Error::Invalid("ssh signature must be 64 bytes".into()))
}

pub fn put_string(out: &mut Vec<u8>, s: &[u8]) {
    out.extend_from_slice(&(s.len() as u32).to_be_bytes());
    out.extend_from_slice(s);
}

pub fn read_u32(cur: &mut &[u8]) -> Result<u32> {
    if cur.len() < 4 {
        return Err(Error::Invalid("truncated ssh data".into()));
    }
    let v = u32::from_be_bytes(cur[..4].try_into().unwrap());
    *cur = &cur[4..];
    Ok(v)
}

pub fn read_string<'a>(cur: &mut &'a [u8]) -> Result<&'a [u8]> {
    let len = read_u32(cur)? as usize;
    if cur.len() < len {
        return Err(Error::Invalid("truncated ssh data".into()));
    }
    let (s, rest) = cur.split_at(len);
    *cur = rest;
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Generated with `ssh-keygen -t ed25519 -N '' -C test@agit` for this test.
    const PUB: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGkmTwaq4yTAX4vyc3zq/dMzitl+O9gXb9V8+ZxCfNQD test@agit";
    const FP: &str = "SHA256:NeEsyNdzgZA8xAHH2Nw7ehNrSS7b6gYEjQl2qlIHrqM";
    const PRIV: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACBpJk8GquMkwF+L8nN86v3TM4rZfjvYF2/VfPmcQnzUAwAAAJAxNZgQMTWY
EAAAAAtzc2gtZWQyNTUxOQAAACBpJk8GquMkwF+L8nN86v3TM4rZfjvYF2/VfPmcQnzUAw
AAAEBA86Xw1cxs1oMrslYVrWPVZ+YZaWe0ZG1tVH+N62CwZ2kmTwaq4yTAX4vyc3zq/dMz
itl+O9gXb9V8+ZxCfNQDAAAACXRlc3RAYWdpdAECAwQ=
-----END OPENSSH PRIVATE KEY-----
";
    // `ssh-keygen -t ed25519 -N secretpass -C enc@agit`
    const ENC_PRIV: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAACmFlczI1Ni1jdHIAAAAGYmNyeXB0AAAAGAAAABDmIG4+j+
U5e7hXkBKmFhTbAAAAGAAAAAEAAAAzAAAAC3NzaC1lZDI1NTE5AAAAIJgdKg9iEsjdS21H
XsqGMDlXPMY3vwwkyCb8o8gnfUheAAAAkB6LWOyI0U5IDu4HCFPZ10l5M1vUGorr4emEHb
gbS7UyiFeZTT9T50imdXmoxUjmazs20syBExU+i+Og93G7BZTg23iTb9iZqBS5lQckwrQA
XgP9U6oCwmc+B74gvun0JAyu5vZOJlHdzsdyDo1QByvuVpHw5w4vZO8Oxs5VciOUuqmQ0Z
ejTKonWqkejPD2rA==
-----END OPENSSH PRIVATE KEY-----
";
    const ENC_FP: &str = "SHA256:XpLT8qchEb1bfYCxfcKpTPAIkCK2gnB8E+zn9jxQEEA";

    #[test]
    fn parses_openssh_public_key_and_fingerprint() {
        let k = SshPublicKey::parse(PUB).unwrap();
        assert_eq!(k.fingerprint(), FP);
        assert_eq!(k.comment.as_deref(), Some("test@agit"));
        assert_eq!(k.to_openssh(), PUB);
        // hex round trip and agit `.pub` line
        let h = SshPublicKey::parse(&k.to_hex()).unwrap();
        assert_eq!(h.as_bytes(), k.as_bytes());
        let agit_line = format!("{} {} name", crate::sign::ALG, k.to_hex());
        let a = SshPublicKey::parse(&agit_line).unwrap();
        assert_eq!(a.as_bytes(), k.as_bytes());
        assert_eq!(a.comment.as_deref(), Some("name"));
        assert_eq!(a.key_id(), crate::sign::key_id(k.as_bytes()));
    }

    #[test]
    fn rejects_other_key_types() {
        let e = SshPublicKey::parse("ssh-rsa AAAAB3NzaC1yc2E= x").unwrap_err();
        assert!(e.to_string().contains("ssh-ed25519"), "{e}");
        assert!(SshPublicKey::parse("").is_err());
        assert!(SshPublicKey::parse("ssh-ed25519 !!!").is_err());
    }

    #[test]
    fn parses_private_key_and_signs() {
        let k = SshPrivateKey::parse(PRIV).unwrap();
        assert!(!k.is_encrypted());
        assert_eq!(k.public().fingerprint(), FP);
        assert_eq!(k.public().comment.as_deref(), Some("test@agit"));
        let msg = auth_message("nonce-1");
        let sig = k.sign(&msg).unwrap();
        assert!(k.public().verify(&msg, &sig));
        assert!(!k.public().verify(&auth_message("nonce-2"), &sig));
        assert!(!k.public().verify(&msg, &sig[..63]));
        // re-encoding yields a parseable file with the same key
        let again = SshPrivateKey::parse(&k.to_openssh().unwrap()).unwrap();
        assert_eq!(again.public(), k.public());
        assert_eq!(again.sign(&msg).unwrap(), sig);
    }

    #[test]
    fn recognises_encrypted_private_keys() {
        let k = SshPrivateKey::parse(ENC_PRIV).unwrap();
        assert!(k.is_encrypted());
        assert_eq!(k.public().fingerprint(), ENC_FP);
        assert!(k.sign(b"x").is_none());
        assert!(k.to_openssh().is_err());
    }

    #[test]
    fn agit_hex_secret_is_accepted() {
        let kp = crate::sign::Keypair::generate("t");
        let text = kp.secret_hex();
        let k = SshPrivateKey::parse(&text).unwrap();
        assert_eq!(k.public().to_hex(), kp.public_key_hex());
    }

    #[test]
    fn wire_and_signature_blob() {
        let k = SshPublicKey::parse(PUB).unwrap();
        assert_eq!(
            SshPublicKey::from_wire(&k.wire()).unwrap().as_bytes(),
            k.as_bytes()
        );
        let mut blob = Vec::new();
        put_string(&mut blob, KEY_TYPE.as_bytes());
        put_string(&mut blob, &[7u8; 64]);
        assert_eq!(signature_from_wire(&blob).unwrap(), [7u8; 64]);
        assert!(signature_from_wire(&blob[..10]).is_err());
    }
}
