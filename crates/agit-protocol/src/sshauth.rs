//! Signing in to an agit server with an SSH key.
//!
//! The flow is `POST /auth/challenge` (a single-use nonce), a signature over
//! `agit-auth-v1\n<nonce>` with the Ed25519 key, then `POST /auth/session`,
//! which returns a short-lived scoped token. This module finds a usable key
//! (an explicit file, `AGIT_SSH_KEY`, `~/.ssh/id_ed25519`, the identities
//! held by ssh-agent, or an agit signing key), signs through the file or
//! through ssh-agent, and caches the resulting session per server in
//! `~/.agit/sessions.json` so later processes reuse it until it expires.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use agit_core::events::now_ms;
use agit_core::refs::write_atomic;
use agit_core::sshkey::{auth_message, SshPrivateKey, SshPublicKey};
use agit_core::{Error, Result};
use serde::{Deserialize, Serialize};

use crate::client::Client;
use crate::types::{SessionCreate, TokenCreated};

/// Something that can sign a challenge: a key we hold or one ssh-agent holds.
pub struct Identity {
    pub public: SshPublicKey,
    /// Where the key came from, for messages (`~/.ssh/id_ed25519`, `ssh-agent`).
    pub source: String,
    signer: Signer,
}

enum Signer {
    Local(Box<SshPrivateKey>),
    Agent { blob: Vec<u8> },
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Identity({}, {})",
            self.public.fingerprint(),
            self.source
        )
    }
}

impl Identity {
    pub fn from_private_key(key: SshPrivateKey, source: impl Into<String>) -> Result<Identity> {
        if key.is_encrypted() {
            return Err(Error::Invalid(
                "private key is passphrase-protected; add it to ssh-agent (ssh-add) or \
                 point AGIT_SSH_KEY at an unencrypted key"
                    .into(),
            ));
        }
        Ok(Identity {
            public: key.public().clone(),
            source: source.into(),
            signer: Signer::Local(Box::new(key)),
        })
    }

    pub fn sign(&self, message: &[u8]) -> Result<[u8; 64]> {
        match &self.signer {
            Signer::Local(k) => k
                .sign(message)
                .ok_or_else(|| Error::Invalid("key cannot sign".into())),
            Signer::Agent { blob } => agent::sign(blob, message),
        }
    }
}

fn home() -> PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn load_key_file(path: &Path) -> Result<SshPrivateKey> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| Error::Invalid(format!("cannot read ssh key {}: {e}", path.display())))?;
    SshPrivateKey::parse(&text).map_err(|e| Error::Invalid(format!("{}: {e}", path.display())))
}

/// Candidate identities in the order they are tried. `explicit` (or
/// `AGIT_SSH_KEY`) must be usable or an error is returned; the default
/// candidates are collected best-effort. Errors for keys that were found but
/// cannot be used (encrypted without an agent) are returned alongside so the
/// caller can show them when nothing works.
pub fn identities(explicit: Option<&Path>) -> Result<(Vec<Identity>, Vec<String>)> {
    let mut out: Vec<Identity> = Vec::new();
    let mut problems = Vec::new();
    let explicit = explicit
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("AGIT_SSH_KEY").map(PathBuf::from));
    // ssh-agent is only asked when no usable key file was named explicitly.
    let agent_ids = match &explicit {
        Some(p) => match load_key_file(p) {
            Ok(k) if k.is_encrypted() => agent::identities().unwrap_or_default(),
            _ => vec![],
        },
        None => agent::identities().unwrap_or_default(),
    };
    let push = |id: Identity, out: &mut Vec<Identity>| {
        if !out.iter().any(|o| o.public == id.public) {
            out.push(id);
        }
    };
    let from_file = |path: &Path| -> Result<Identity> {
        let key = load_key_file(path)?;
        if key.is_encrypted() {
            // The public half is readable: look for the key in ssh-agent.
            if let Some((blob, _)) = agent_ids
                .iter()
                .find(|(_, pk)| pk.as_bytes() == key.public().as_bytes())
            {
                return Ok(Identity {
                    public: key.public().clone(),
                    source: format!("{} via ssh-agent", path.display()),
                    signer: Signer::Agent { blob: blob.clone() },
                });
            }
            return Err(Error::Invalid(format!(
                "{} is passphrase-protected and not loaded in ssh-agent; run `ssh-add {}`",
                path.display(),
                path.display()
            )));
        }
        Identity::from_private_key(key, path.display().to_string())
    };

    if let Some(p) = explicit {
        push(from_file(&p)?, &mut out);
        return Ok((out, problems));
    }

    let default = home().join(".ssh").join("id_ed25519");
    if default.is_file() {
        match from_file(&default) {
            Ok(id) => push(id, &mut out),
            Err(e) => problems.push(e.to_string()),
        }
    }
    for (blob, pk) in &agent_ids {
        push(
            Identity {
                source: format!(
                    "ssh-agent{}",
                    pk.comment
                        .as_deref()
                        .map(|c| format!(" ({c})"))
                        .unwrap_or_default()
                ),
                public: pk.clone(),
                signer: Signer::Agent { blob: blob.clone() },
            },
            &mut out,
        );
    }
    // agit's own signing keys double as login keys.
    if let Ok(list) = agit_core::sign::Keypair::list() {
        for info in list {
            let path = agit_core::sign::Keypair::path_for(&info.name);
            if let Ok(key) = load_key_file(&path) {
                if let Ok(id) = Identity::from_private_key(key, format!("agit key {}", info.name)) {
                    push(id, &mut out);
                }
            }
        }
    }
    Ok((out, problems))
}

/// Sign in with one identity.
pub fn login(client: &Client, id: &Identity, ttl_secs: Option<u64>) -> Result<TokenCreated> {
    let ch = client.auth_challenge()?;
    let sig = id.sign(&auth_message(&ch.nonce))?;
    client.auth_session(&SessionCreate {
        public_key: id.public.to_openssh(),
        nonce: ch.nonce,
        signature: hex::encode(sig),
        ttl_secs,
    })
}

/// Try every candidate identity until the server accepts one. Keys the
/// server does not know are skipped; any other failure is returned at once.
pub fn login_auto(
    client: &Client,
    explicit: Option<&Path>,
    ttl_secs: Option<u64>,
) -> Result<(TokenCreated, Identity)> {
    let (ids, mut problems) = identities(explicit)?;
    if ids.is_empty() && problems.is_empty() {
        return Err(Error::Unauthorized(
            "no ssh key found (looked for AGIT_SSH_KEY, ~/.ssh/id_ed25519, ssh-agent and \
             ~/.agit/keys); generate one with `ssh-keygen -t ed25519` and ask an admin to \
             add it with `agit auth key add`"
                .into(),
        ));
    }
    for id in ids {
        match login(client, &id, ttl_secs) {
            Ok(t) => return Ok((t, id)),
            Err(Error::Forbidden(m)) if m.contains("not authorized") => {
                problems.push(format!(
                    "{} ({}) is not authorized on {}",
                    id.public.fingerprint(),
                    id.source,
                    client.base()
                ));
            }
            Err(e) => return Err(e),
        }
    }
    Err(Error::Unauthorized(format!(
        "no ssh key was accepted: {}",
        problems.join("; ")
    )))
}

// ---------------------------------------------------------------------------
// session cache
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedSession {
    pub token: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Fingerprint of the key that signed in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
}

static SESSIONS_OVERRIDE: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// Use `path` instead of `~/.agit/sessions.json` for the rest of this
/// process (tests and embedders; `AGIT_SESSIONS_FILE` does the same from the
/// environment).
pub fn set_sessions_file(path: Option<PathBuf>) {
    *SESSIONS_OVERRIDE.lock().unwrap() = path;
}

pub fn sessions_file() -> PathBuf {
    if let Some(p) = SESSIONS_OVERRIDE.lock().unwrap().clone() {
        return p;
    }
    if let Some(p) = std::env::var_os("AGIT_SESSIONS_FILE") {
        return PathBuf::from(p);
    }
    home().join(".agit").join("sessions.json")
}

fn load_sessions() -> BTreeMap<String, CachedSession> {
    std::fs::read(sessions_file())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_sessions(all: &BTreeMap<String, CachedSession>) -> Result<()> {
    let path = sessions_file();
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    write_atomic(&path, &serde_json::to_vec_pretty(all)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn key_for(base: &str) -> String {
    base.trim_end_matches('/').to_string()
}

/// The cached, unexpired session for a server base URL.
pub fn cached_session(base: &str) -> Option<CachedSession> {
    let s = load_sessions().remove(&key_for(base))?;
    if s.expires_ms.is_some_and(|e| e <= now_ms() + 30_000) {
        return None;
    }
    Some(s)
}

pub fn remember_session(base: &str, created: &TokenCreated, fingerprint: Option<String>) {
    let mut all = load_sessions();
    all.retain(|_, s| !s.expires_ms.is_some_and(|e| e <= now_ms()));
    all.insert(
        key_for(base),
        CachedSession {
            token: created.token.clone(),
            id: created.info.id.clone(),
            expires_ms: created.info.expires_ms,
            agent: created.info.agent.clone(),
            key: created.info.key.clone(),
            fingerprint,
        },
    );
    let _ = save_sessions(&all);
}

pub fn forget_session(base: &str) -> Option<CachedSession> {
    let mut all = load_sessions();
    let removed = all.remove(&key_for(base));
    if removed.is_some() {
        let _ = save_sessions(&all);
    }
    removed
}

// ---------------------------------------------------------------------------
// ssh-agent client (unix domain socket at $SSH_AUTH_SOCK)
// ---------------------------------------------------------------------------

pub mod agent {
    use super::*;

    const REQUEST_IDENTITIES: u8 = 11;
    const IDENTITIES_ANSWER: u8 = 12;
    const SIGN_REQUEST: u8 = 13;
    const SIGN_RESPONSE: u8 = 14;

    /// Ed25519 identities the agent holds, as (wire blob, public key).
    pub fn identities() -> Result<Vec<(Vec<u8>, SshPublicKey)>> {
        let reply = roundtrip(&[REQUEST_IDENTITIES])?;
        if reply.first() != Some(&IDENTITIES_ANSWER) {
            return Err(Error::Invalid(
                "ssh-agent refused to list identities".into(),
            ));
        }
        let mut cur = &reply[1..];
        let n = agit_core::sshkey::read_u32(&mut cur)?;
        let mut out = Vec::new();
        for _ in 0..n {
            let blob = agit_core::sshkey::read_string(&mut cur)?.to_vec();
            let comment = agit_core::sshkey::read_string(&mut cur)?;
            if let Ok(mut pk) = SshPublicKey::from_wire(&blob) {
                let c = String::from_utf8_lossy(comment).to_string();
                pk.comment = if c.is_empty() { None } else { Some(c) };
                out.push((blob, pk));
            }
        }
        Ok(out)
    }

    pub fn sign(blob: &[u8], message: &[u8]) -> Result<[u8; 64]> {
        let mut req = vec![SIGN_REQUEST];
        agit_core::sshkey::put_string(&mut req, blob);
        agit_core::sshkey::put_string(&mut req, message);
        req.extend_from_slice(&0u32.to_be_bytes());
        let reply = roundtrip(&req)?;
        if reply.first() != Some(&SIGN_RESPONSE) {
            return Err(Error::Invalid(
                "ssh-agent refused to sign (is the key loaded? `ssh-add -l`)".into(),
            ));
        }
        let mut cur = &reply[1..];
        let sig_blob = agit_core::sshkey::read_string(&mut cur)?;
        agit_core::sshkey::signature_from_wire(sig_blob)
    }

    #[cfg(unix)]
    fn roundtrip(msg: &[u8]) -> Result<Vec<u8>> {
        use std::io::{Read, Write};
        use std::os::unix::net::UnixStream;
        let sock = std::env::var_os("SSH_AUTH_SOCK")
            .ok_or_else(|| Error::Invalid("SSH_AUTH_SOCK is not set".into()))?;
        let mut s = UnixStream::connect(&sock)
            .map_err(|e| Error::Invalid(format!("cannot connect to ssh-agent: {e}")))?;
        s.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
        s.set_write_timeout(Some(std::time::Duration::from_secs(10)))?;
        let mut framed = (msg.len() as u32).to_be_bytes().to_vec();
        framed.extend_from_slice(msg);
        s.write_all(&framed)?;
        let mut len = [0u8; 4];
        s.read_exact(&mut len)?;
        let len = u32::from_be_bytes(len) as usize;
        if len > 1 << 20 {
            return Err(Error::Invalid("ssh-agent reply too large".into()));
        }
        let mut body = vec![0u8; len];
        s.read_exact(&mut body)?;
        Ok(body)
    }

    #[cfg(not(unix))]
    fn roundtrip(_msg: &[u8]) -> Result<Vec<u8>> {
        Err(Error::Invalid(
            "ssh-agent is only supported on unix; use an unencrypted key file".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_cache_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        set_sessions_file(Some(dir.path().join("s.json")));
        let created = TokenCreated {
            token: "agit_x".into(),
            info: crate::types::TokenInfo {
                id: "tok_1".into(),
                name: "session".into(),
                agent: Some("a".into()),
                scopes: vec![],
                created_ms: 0,
                expires_ms: Some(now_ms() + 3_600_000),
                created_by: "key_1".into(),
                last_used_ms: None,
                key: Some("key_1".into()),
            },
        };
        remember_session("http://h/api/v1/", &created, Some("SHA256:x".into()));
        let s = cached_session("http://h/api/v1").unwrap();
        assert_eq!(s.token, "agit_x");
        assert_eq!(s.fingerprint.as_deref(), Some("SHA256:x"));
        // expired sessions are not returned
        let mut expired = created.clone();
        expired.info.expires_ms = Some(now_ms() - 1);
        remember_session("http://old/api/v1", &expired, None);
        assert!(cached_session("http://old/api/v1").is_none());
        assert!(forget_session("http://h/api/v1").is_some());
        assert!(cached_session("http://h/api/v1").is_none());
        set_sessions_file(None);
    }

    #[test]
    fn identities_from_explicit_file() {
        let dir = tempfile::tempdir().unwrap();
        let kp = agit_core::sign::Keypair::generate("t");
        let path = dir.path().join("k");
        std::fs::write(&path, kp.secret_hex()).unwrap();
        let (ids, problems) = identities(Some(&path)).unwrap();
        assert_eq!(ids.len(), 1);
        assert!(problems.is_empty());
        let msg = auth_message("n");
        let sig = ids[0].sign(&msg).unwrap();
        assert!(ids[0].public.verify(&msg, &sig));
        assert!(identities(Some(&dir.path().join("missing"))).is_err());
    }
}
