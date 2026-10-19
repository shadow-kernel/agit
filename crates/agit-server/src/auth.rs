//! Authentication and authorization.
//!
//! Three ways in, all ending in the same [`Principal`]:
//!
//! * **SSH keys** (`POST /auth/challenge` + `POST /auth/session`): the server
//!   keeps a registry of authorized Ed25519 public keys with scopes
//!   (`auth/authorized_keys.json`) plus the admin keys given at startup
//!   (`--admin-key`). A client proves possession of the private key by
//!   signing a single-use nonce and receives a short-lived session token.
//!   Deleting a key revokes its sessions.
//! * **Scoped tokens** (`POST /auth/tokens`) carry `{repo, access}` scopes and
//!   may be bound to an agent identity. A bound token always acts as that
//!   agent, so `X-Agit-Agent` cannot be spoofed for it.
//! * The **root token** (`--token` / `AGIT_SERVER_TOKEN`) has every permission.
//!
//! The server is *closed* as soon as a root token or an admin key is
//! configured; without either it runs in open mode (every request is an
//! anonymous root) for local development. Every mutation and every denied
//! request is appended to `audit.jsonl`.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use agit_core::events::now_ms;
use agit_core::refs::write_atomic;
use agit_core::sshkey::{auth_message, SshPublicKey};
use agit_core::{Error, Result};
use agit_protocol::types::{
    Access, AuthorizedKey, AuthorizedKeyCreate, Challenge, Scope, SessionCreate, TokenCreate,
    TokenCreated, TokenInfo, WhoAmI,
};
use serde::{Deserialize, Serialize};

/// `TokenInfo.key` of sessions opened with a startup admin key.
pub const ADMIN_KEY_ID: &str = "admin";
const CHALLENGE_TTL_MS: i64 = 2 * 60 * 1000;
const MAX_CHALLENGES: usize = 10_000;
pub const SESSION_DEFAULT_TTL_SECS: u64 = 8 * 3600;
pub const SESSION_MAX_TTL_SECS: u64 = 30 * 86_400;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TokenRecord {
    #[serde(flatten)]
    info: TokenInfo,
    token_hash: String,
}

#[derive(Debug, Clone)]
pub struct Principal {
    /// `root`, `anonymous` or the token id.
    pub id: String,
    pub root: bool,
    pub agent: Option<String>,
    pub scopes: Vec<Scope>,
    /// Authorized key behind a session token.
    pub key: Option<String>,
}

impl Principal {
    pub fn root() -> Self {
        Principal {
            id: "root".into(),
            root: true,
            agent: None,
            scopes: vec![],
            key: None,
        }
    }
    pub fn anonymous_root() -> Self {
        Principal {
            id: "anonymous".into(),
            root: true,
            agent: None,
            scopes: vec![],
            key: None,
        }
    }
    /// An unauthenticated caller on a public route.
    pub fn anonymous() -> Self {
        Principal {
            id: "anonymous".into(),
            root: false,
            agent: None,
            scopes: vec![],
            key: None,
        }
    }

    pub fn allows(&self, repo: Option<&str>, needed: Access) -> bool {
        if self.root {
            return true;
        }
        // Repository-independent reads (whoami, repository listing, which is
        // filtered by scope) are open to every authenticated principal.
        if repo.is_none() && needed == Access::Read {
            return true;
        }
        self.scopes.iter().any(|s| {
            let repo_ok = s.repo == "*" || repo.is_some_and(|r| r == s.repo);
            repo_ok && s.access >= needed
        })
    }

    /// May this principal hand out `scope` to a token or key? Server-wide
    /// scopes need root or a server-wide admin scope; repository scopes need
    /// admin on that repository.
    pub fn can_grant(&self, scope: &Scope) -> bool {
        if self.root {
            return true;
        }
        if scope.repo == "*" {
            return self
                .scopes
                .iter()
                .any(|s| s.repo == "*" && s.access == Access::Admin);
        }
        self.allows(Some(&scope.repo), Access::Admin)
    }

    pub fn whoami(&self) -> WhoAmI {
        WhoAmI {
            principal: self.id.clone(),
            agent: self.agent.clone(),
            root: self.root,
            scopes: self.scopes.clone(),
            key: self.key.clone(),
        }
    }
}

fn hash_token(t: &str) -> String {
    blake3::hash(t.as_bytes()).to_hex().to_string()
}

fn random_hex(bytes: usize) -> String {
    let mut s = String::new();
    while s.len() < bytes * 2 {
        s.push_str(&uuid::Uuid::new_v4().simple().to_string());
    }
    s.truncate(bytes * 2);
    s
}

pub struct AuthStore {
    tokens_path: PathBuf,
    keys_path: PathBuf,
    audit_path: PathBuf,
    admin_keys: Vec<SshPublicKey>,
    /// Parsed files, loaded once; this process is their only writer.
    tokens: Mutex<Option<Vec<TokenRecord>>>,
    keys: Mutex<Option<Vec<AuthorizedKey>>>,
    /// Outstanding challenge nonces and their expiry.
    challenges: Mutex<HashMap<String, i64>>,
}

fn load_json<T: serde::de::DeserializeOwned>(path: &std::path::Path) -> Result<Vec<T>> {
    match fs::read(path) {
        Ok(b) => Ok(serde_json::from_slice(&b)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) => Err(e.into()),
    }
}

fn save_json<T: Serialize>(path: &std::path::Path, v: &[T]) -> Result<()> {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p)?;
    }
    write_atomic(path, &serde_json::to_vec_pretty(v)?)
}

impl AuthStore {
    pub fn open(data_dir: &std::path::Path) -> AuthStore {
        AuthStore {
            tokens_path: data_dir.join("auth").join("tokens.json"),
            keys_path: data_dir.join("auth").join("authorized_keys.json"),
            audit_path: data_dir.join("audit.jsonl"),
            admin_keys: vec![],
            tokens: Mutex::new(None),
            keys: Mutex::new(None),
            challenges: Mutex::new(HashMap::new()),
        }
    }

    /// Keys with full admin rights, given at startup (not persisted).
    pub fn set_admin_keys(&mut self, keys: Vec<SshPublicKey>) {
        self.admin_keys = keys;
    }

    pub fn has_admin_keys(&self) -> bool {
        !self.admin_keys.is_empty()
    }

    // -- tokens ---------------------------------------------------------------

    fn with_tokens<R>(&self, f: impl FnOnce(&mut Vec<TokenRecord>) -> Result<R>) -> Result<R> {
        let mut guard = self.tokens.lock().unwrap();
        if guard.is_none() {
            *guard = Some(load_json(&self.tokens_path)?);
        }
        f(guard.as_mut().unwrap())
    }

    pub fn list(&self) -> Result<Vec<TokenInfo>> {
        self.with_tokens(|all| Ok(all.iter().map(|r| r.info.clone()).collect()))
    }

    /// Create a token. The caller must already be allowed to grant every
    /// requested scope (checked by the handler).
    pub fn create(&self, req: TokenCreate, created_by: &str) -> Result<TokenCreated> {
        self.create_with_key(req, created_by, None)
    }

    fn create_with_key(
        &self,
        req: TokenCreate,
        created_by: &str,
        key: Option<String>,
    ) -> Result<TokenCreated> {
        if req.name.trim().is_empty() {
            return Err(Error::Invalid("token name is required".into()));
        }
        if req.scopes.is_empty() {
            return Err(Error::Invalid("at least one scope is required".into()));
        }
        let id = format!("tok_{}", random_hex(5));
        let secret = format!("agit_{}", random_hex(16));
        let now = now_ms();
        let info = TokenInfo {
            id: id.clone(),
            name: req.name,
            agent: req.agent.filter(|a| !a.is_empty()),
            scopes: req.scopes,
            created_ms: now,
            expires_ms: req.ttl_secs.map(|s| now + (s as i64) * 1000),
            created_by: created_by.to_string(),
            last_used_ms: None,
            key,
        };
        let rec = TokenRecord {
            info: info.clone(),
            token_hash: hash_token(&secret),
        };
        self.with_tokens(|all| {
            all.push(rec);
            save_json(&self.tokens_path, all)
        })?;
        Ok(TokenCreated {
            token: secret,
            info,
        })
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        self.with_tokens(|all| {
            let before = all.len();
            all.retain(|r| r.info.id != id);
            if all.len() == before {
                return Err(Error::NotFound(format!("token {id}")));
            }
            save_json(&self.tokens_path, all)
        })
    }

    /// Resolve a presented bearer token to a principal (None = unknown,
    /// expired, or a session whose key was removed).
    pub fn resolve(&self, presented: &str) -> Result<Option<Principal>> {
        let h = hash_token(presented);
        let rec = self.with_tokens(|all| Ok(all.iter().find(|r| r.token_hash == h).cloned()))?;
        let Some(rec) = rec else {
            return Ok(None);
        };
        if rec.info.expires_ms.is_some_and(|e| e <= now_ms()) {
            return Ok(None);
        }
        if let Some(k) = &rec.info.key {
            if k != ADMIN_KEY_ID && self.get_key(k)?.is_none() {
                return Ok(None);
            }
        }
        Ok(Some(Principal {
            id: rec.info.id.clone(),
            root: false,
            agent: rec.info.agent.clone(),
            scopes: rec.info.scopes.clone(),
            key: rec.info.key.clone(),
        }))
    }

    /// Record `last_used_ms` (best effort, throttled to once a minute).
    pub fn touch(&self, id: &str) {
        let _ = self.with_tokens(|all| {
            if let Some(r) = all.iter_mut().find(|r| r.info.id == id) {
                let now = now_ms();
                if r.info.last_used_ms.is_none_or(|t| now - t > 60_000) {
                    r.info.last_used_ms = Some(now);
                    let _ = save_json(&self.tokens_path, all);
                }
            }
            Ok(())
        });
    }

    // -- authorized keys ------------------------------------------------------

    fn with_keys<R>(&self, f: impl FnOnce(&mut Vec<AuthorizedKey>) -> Result<R>) -> Result<R> {
        let mut guard = self.keys.lock().unwrap();
        if guard.is_none() {
            *guard = Some(load_json(&self.keys_path)?);
        }
        f(guard.as_mut().unwrap())
    }

    pub fn list_keys(&self) -> Result<Vec<AuthorizedKey>> {
        self.with_keys(|all| Ok(all.clone()))
    }

    pub fn get_key(&self, id: &str) -> Result<Option<AuthorizedKey>> {
        self.with_keys(|all| Ok(all.iter().find(|k| k.id == id).cloned()))
    }

    fn find_key(&self, public: &SshPublicKey) -> Result<Option<AuthorizedKey>> {
        let fp = public.fingerprint();
        self.with_keys(|all| Ok(all.iter().find(|k| k.fingerprint == fp).cloned()))
    }

    /// Register a public key. Scopes must already be checked by the handler.
    pub fn add_key(&self, req: AuthorizedKeyCreate, created_by: &str) -> Result<AuthorizedKey> {
        let public = SshPublicKey::parse(&req.public_key)?;
        if req.scopes.is_empty() {
            return Err(Error::Invalid("at least one scope is required".into()));
        }
        let fingerprint = public.fingerprint();
        let name = req
            .name
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .or_else(|| public.comment.clone())
            .unwrap_or_else(|| fingerprint.clone());
        let entry = AuthorizedKey {
            id: format!("key_{}", random_hex(5)),
            name,
            fingerprint: fingerprint.clone(),
            key_id: public.key_id(),
            public_key: public.to_openssh(),
            agent: req.agent.filter(|a| !a.is_empty()),
            scopes: req.scopes,
            created_ms: now_ms(),
            created_by: created_by.to_string(),
            last_used_ms: None,
        };
        self.with_keys(|all| {
            if let Some(existing) = all.iter().find(|k| k.fingerprint == fingerprint) {
                return Err(Error::AlreadyExists(
                    format!("key {fingerprint} is already authorized as {}", existing.id).into(),
                ));
            }
            all.push(entry.clone());
            save_json(&self.keys_path, all)
        })?;
        Ok(entry)
    }

    /// Remove a key and every session it opened.
    pub fn delete_key(&self, id: &str) -> Result<usize> {
        self.with_keys(|all| {
            let before = all.len();
            all.retain(|k| k.id != id);
            if all.len() == before {
                return Err(Error::NotFound(format!("authorized key {id}")));
            }
            save_json(&self.keys_path, all)
        })?;
        self.with_tokens(|all| {
            let before = all.len();
            all.retain(|t| t.info.key.as_deref() != Some(id));
            if all.len() != before {
                save_json(&self.tokens_path, all)?;
            }
            Ok(before - all.len())
        })
    }

    fn touch_key(&self, id: &str) {
        let _ = self.with_keys(|all| {
            if let Some(k) = all.iter_mut().find(|k| k.id == id) {
                k.last_used_ms = Some(now_ms());
                let _ = save_json(&self.keys_path, all);
            }
            Ok(())
        });
    }

    // -- challenge / session --------------------------------------------------

    pub fn issue_challenge(&self) -> Result<Challenge> {
        let now = now_ms();
        let mut ch = self.challenges.lock().unwrap();
        ch.retain(|_, exp| *exp > now);
        if ch.len() >= MAX_CHALLENGES {
            return Err(Error::Invalid(
                "too many outstanding challenges; try again shortly".into(),
            ));
        }
        let nonce = random_hex(32);
        let expires_ms = now + CHALLENGE_TTL_MS;
        ch.insert(nonce.clone(), expires_ms);
        Ok(Challenge { nonce, expires_ms })
    }

    /// Verify a signed challenge and open a session for the key's principal.
    pub fn redeem(&self, req: &SessionCreate) -> Result<TokenCreated> {
        let valid = {
            let mut ch = self.challenges.lock().unwrap();
            ch.remove(&req.nonce).is_some_and(|exp| exp > now_ms())
        };
        if !valid {
            return Err(Error::Forbidden(
                "challenge is unknown, already used or expired; request a new one".into(),
            ));
        }
        let public = SshPublicKey::parse(&req.public_key)?;
        let sig = hex::decode(req.signature.trim())
            .map_err(|_| Error::Invalid("signature must be hex".into()))?;
        if !public.verify(&auth_message(&req.nonce), &sig) {
            return Err(Error::Forbidden(
                "signature does not match the challenge".into(),
            ));
        }
        let fp = public.fingerprint();
        let (key_id, name, agent, scopes) = if let Some(admin) = self
            .admin_keys
            .iter()
            .find(|k| k.as_bytes() == public.as_bytes())
        {
            (
                ADMIN_KEY_ID.to_string(),
                admin
                    .comment
                    .clone()
                    .unwrap_or_else(|| format!("admin key {fp}")),
                None,
                vec![Scope {
                    repo: "*".into(),
                    access: Access::Admin,
                }],
            )
        } else if let Some(k) = self.find_key(&public)? {
            (k.id, k.name, k.agent, k.scopes)
        } else {
            return Err(Error::Forbidden(format!(
                "ssh key {fp} is not authorized on this server"
            )));
        };
        let ttl = req
            .ttl_secs
            .unwrap_or(SESSION_DEFAULT_TTL_SECS)
            .clamp(1, SESSION_MAX_TTL_SECS);
        let created = self.create_with_key(
            TokenCreate {
                name: format!("session: {name}"),
                agent,
                scopes,
                ttl_secs: Some(ttl),
            },
            &key_id,
            Some(key_id.clone()),
        )?;
        if key_id != ADMIN_KEY_ID {
            self.touch_key(&key_id);
        }
        Ok(created)
    }

    // -- audit ----------------------------------------------------------------

    pub fn audit(
        &self,
        principal: &Principal,
        agent: Option<&str>,
        method: &str,
        path: &str,
        status: u16,
    ) {
        let line = serde_json::json!({
            "time_ms": now_ms(),
            "principal": principal.id,
            "agent": agent,
            "method": method,
            "path": path,
            "status": status,
        });
        // One `write(2)` per line: `O_APPEND` makes a single write atomic
        // with respect to other writers, whereas formatting straight into
        // the file issues many small writes that interleave under load.
        let mut buf = match serde_json::to_vec(&line) {
            Ok(b) => b,
            Err(_) => return,
        };
        buf.push(b'\n');
        if let Ok(mut f) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.audit_path)
        {
            let _ = f.write_all(&buf);
        }
    }
}

#[cfg(test)]
mod audit_tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn concurrent_audit_lines_stay_whole() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(AuthStore::open(dir.path()));
        let threads: Vec<_> = (0..16)
            .map(|t| {
                let store = store.clone();
                std::thread::spawn(move || {
                    let p = Principal {
                        id: format!("tok_{t:02}"),
                        root: false,
                        agent: Some(format!("agent-{t}")),
                        scopes: vec![],
                        key: None,
                    };
                    for i in 0..200 {
                        store.audit(
                            &p,
                            p.agent.as_deref(),
                            "POST",
                            &format!("/repos/r{t}/refs/update/{i}"),
                            200,
                        );
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let text = fs::read_to_string(dir.path().join("audit.jsonl")).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 16 * 200);
        for l in lines {
            let v: serde_json::Value = serde_json::from_str(l).unwrap_or_else(|e| {
                panic!("torn audit line {l:?}: {e}");
            });
            assert_eq!(v["status"], 200);
        }
    }
}

/// Routes that need no credentials at all.
pub fn is_public(method: &str, path: &str) -> bool {
    let rest = path.strip_prefix("/api/v1").unwrap_or(path);
    method == "POST" && matches!(rest, "/auth/challenge" | "/auth/session")
}

/// Which access a request needs, derived from method and path.
pub fn required_access(method: &str, path: &str) -> (Option<String>, Access) {
    let rest = path.strip_prefix("/api/v1").unwrap_or(path);
    if rest.starts_with("/auth/") {
        let needed = match rest {
            "/auth/whoami" | "/auth/logout" => Access::Read,
            _ => Access::Admin,
        };
        return (None, needed);
    }
    let mut repo: Option<String> = None;
    let mut tail = "";
    if let Some(after) = rest.strip_prefix("/repos/") {
        let (name, t) = after.split_once('/').unwrap_or((after, ""));
        repo = Some(name.to_string());
        tail = t;
    }
    let mutation = !matches!(method, "GET" | "HEAD" | "OPTIONS");
    let admin_paths = tail.starts_with("policy")
        || tail.starts_with("keys")
        || tail.starts_with("webhooks")
        || tail.starts_with("knowledge/rules");
    let needed = if !mutation {
        Access::Read
    } else if rest == "/repos" || (tail.is_empty() && method == "DELETE") || admin_paths {
        Access::Admin
    } else {
        Access::Write
    };
    (repo, needed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agit_core::sshkey::SshPrivateKey;

    fn store() -> (tempfile::TempDir, AuthStore) {
        let dir = tempfile::tempdir().unwrap();
        let st = AuthStore::open(dir.path());
        (dir, st)
    }

    fn keypair() -> SshPrivateKey {
        SshPrivateKey::parse(&agit_core::sign::Keypair::generate("t").secret_hex()).unwrap()
    }

    fn sign_in(st: &AuthStore, key: &SshPrivateKey, ttl: Option<u64>) -> Result<TokenCreated> {
        let ch = st.issue_challenge().unwrap();
        let sig = key.sign(&auth_message(&ch.nonce)).unwrap();
        st.redeem(&SessionCreate {
            public_key: key.public().to_openssh(),
            nonce: ch.nonce,
            signature: hex::encode(sig),
            ttl_secs: ttl,
        })
    }

    #[test]
    fn key_session_lifecycle() {
        let (_d, st) = store();
        let key = keypair();
        // Unknown key: refused with the phrase the client looks for.
        let err = sign_in(&st, &key, None).unwrap_err();
        assert!(err.to_string().contains("not authorized"), "{err}");
        let added = st
            .add_key(
                AuthorizedKeyCreate {
                    public_key: key.public().to_openssh(),
                    name: Some("laptop".into()),
                    agent: Some("coder-1".into()),
                    scopes: vec![Scope {
                        repo: "demo".into(),
                        access: Access::Write,
                    }],
                },
                "root",
            )
            .unwrap();
        assert_eq!(added.fingerprint, key.public().fingerprint());
        // Same key twice is a conflict.
        assert!(matches!(
            st.add_key(
                AuthorizedKeyCreate {
                    public_key: key.public().to_hex(),
                    name: None,
                    agent: None,
                    scopes: added.scopes.clone(),
                },
                "root"
            ),
            Err(Error::AlreadyExists(_))
        ));
        let created = sign_in(&st, &key, Some(60)).unwrap();
        assert_eq!(created.info.key.as_deref(), Some(added.id.as_str()));
        assert_eq!(created.info.agent.as_deref(), Some("coder-1"));
        let p = st.resolve(&created.token).unwrap().unwrap();
        assert!(p.allows(Some("demo"), Access::Write));
        assert!(!p.allows(Some("other"), Access::Read));
        assert_eq!(p.key.as_deref(), Some(added.id.as_str()));
        // Deleting the key revokes the session.
        assert_eq!(st.delete_key(&added.id).unwrap(), 1);
        assert!(st.resolve(&created.token).unwrap().is_none());
        assert!(st.list().unwrap().is_empty());
    }

    #[test]
    fn challenges_are_single_use_and_signatures_checked() {
        let (_d, mut st) = store();
        let key = keypair();
        st.set_admin_keys(vec![key.public().clone()]);
        let ch = st.issue_challenge().unwrap();
        let sig = hex::encode(key.sign(&auth_message(&ch.nonce)).unwrap());
        let req = SessionCreate {
            public_key: key.public().to_openssh(),
            nonce: ch.nonce.clone(),
            signature: sig.clone(),
            ttl_secs: Some(SESSION_MAX_TTL_SECS * 10),
        };
        let created = st.redeem(&req).unwrap();
        assert_eq!(created.info.key.as_deref(), Some(ADMIN_KEY_ID));
        assert!(created.info.scopes.iter().any(|s| s.repo == "*"));
        // TTL is clamped to the maximum.
        let ttl = created.info.expires_ms.unwrap() - created.info.created_ms;
        assert!(ttl <= SESSION_MAX_TTL_SECS as i64 * 1000);
        // Replay is refused.
        assert!(st
            .redeem(&req)
            .unwrap_err()
            .to_string()
            .contains("challenge"));
        // A wrong signature is refused.
        let ch2 = st.issue_challenge().unwrap();
        let bad = SessionCreate {
            public_key: key.public().to_openssh(),
            nonce: ch2.nonce,
            signature: sig,
            ttl_secs: None,
        };
        assert!(st
            .redeem(&bad)
            .unwrap_err()
            .to_string()
            .contains("signature"));
        // Admin sessions may grant server-wide scopes.
        let p = st.resolve(&created.token).unwrap().unwrap();
        assert!(p.can_grant(&Scope {
            repo: "*".into(),
            access: Access::Admin
        }));
    }

    #[test]
    fn access_rules() {
        assert!(is_public("POST", "/api/v1/auth/challenge"));
        assert!(is_public("POST", "/api/v1/auth/session"));
        assert!(!is_public("GET", "/api/v1/auth/session"));
        assert!(!is_public("POST", "/api/v1/auth/tokens"));
        assert_eq!(
            required_access("POST", "/api/v1/auth/logout").1,
            Access::Read
        );
        assert_eq!(required_access("GET", "/api/v1/auth/keys").1, Access::Admin);
        assert_eq!(
            required_access("POST", "/api/v1/repos/x/refs/update"),
            (Some("x".into()), Access::Write)
        );
        let repo_admin = Principal {
            id: "t".into(),
            root: false,
            agent: None,
            scopes: vec![Scope {
                repo: "x".into(),
                access: Access::Admin,
            }],
            key: None,
        };
        assert!(repo_admin.can_grant(&Scope {
            repo: "x".into(),
            access: Access::Write
        }));
        assert!(!repo_admin.can_grant(&Scope {
            repo: "y".into(),
            access: Access::Read
        }));
        assert!(!repo_admin.can_grant(&Scope {
            repo: "*".into(),
            access: Access::Read
        }));
    }
}
