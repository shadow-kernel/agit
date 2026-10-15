//! Authentication and authorization.
//!
//! * The root token (`--token` / `AGIT_SERVER_TOKEN`) has every permission
//!   and bootstraps everything else.
//! * Scoped tokens (`POST /auth/tokens`) carry `{repo, access}` scopes and
//!   may be bound to an agent identity. A bound token always acts as that
//!   agent, so `X-Agit-Agent` can no longer be spoofed for it.
//! * Without a root token the server runs in open mode (every request is an
//!   anonymous root) for local development; scoped tokens still work and
//!   still bind identities.
//! * Every mutation and every denied request is appended to `audit.jsonl`.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use agit_core::events::now_ms;
use agit_core::refs::write_atomic;
use agit_core::{Error, Result};
use agit_protocol::types::{Access, Scope, TokenCreate, TokenCreated, TokenInfo, WhoAmI};
use serde::{Deserialize, Serialize};

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
}

impl Principal {
    pub fn root() -> Self {
        Principal {
            id: "root".into(),
            root: true,
            agent: None,
            scopes: vec![],
        }
    }
    pub fn anonymous_root() -> Self {
        Principal {
            id: "anonymous".into(),
            root: true,
            agent: None,
            scopes: vec![],
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

    pub fn whoami(&self) -> WhoAmI {
        WhoAmI {
            principal: self.id.clone(),
            agent: self.agent.clone(),
            root: self.root,
            scopes: self.scopes.clone(),
        }
    }
}

fn hash_token(t: &str) -> String {
    blake3::hash(t.as_bytes()).to_hex().to_string()
}

pub struct AuthStore {
    path: PathBuf,
    audit_path: PathBuf,
    lock: Mutex<()>,
}

impl AuthStore {
    pub fn open(data_dir: &std::path::Path) -> AuthStore {
        AuthStore {
            path: data_dir.join("auth").join("tokens.json"),
            audit_path: data_dir.join("audit.jsonl"),
            lock: Mutex::new(()),
        }
    }

    fn load(&self) -> Result<Vec<TokenRecord>> {
        match fs::read(&self.path) {
            Ok(b) => Ok(serde_json::from_slice(&b)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
            Err(e) => Err(e.into()),
        }
    }

    fn save(&self, v: &[TokenRecord]) -> Result<()> {
        if let Some(p) = self.path.parent() {
            fs::create_dir_all(p)?;
        }
        write_atomic(&self.path, &serde_json::to_vec_pretty(v)?)
    }

    pub fn list(&self) -> Result<Vec<TokenInfo>> {
        Ok(self.load()?.into_iter().map(|r| r.info).collect())
    }

    /// Create a token. The caller must already be allowed to grant every
    /// requested scope (checked by the handler).
    pub fn create(&self, req: TokenCreate, created_by: &str) -> Result<TokenCreated> {
        if req.name.trim().is_empty() {
            return Err(Error::Invalid("token name is required".into()));
        }
        if req.scopes.is_empty() {
            return Err(Error::Invalid("at least one scope is required".into()));
        }
        let _g = self.lock.lock().unwrap();
        let mut all = self.load()?;
        let id = format!("tok_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
        let secret = format!("agit_{}", uuid::Uuid::new_v4().simple());
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
        };
        all.push(TokenRecord {
            info: info.clone(),
            token_hash: hash_token(&secret),
        });
        self.save(&all)?;
        Ok(TokenCreated {
            token: secret,
            info,
        })
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        let _g = self.lock.lock().unwrap();
        let mut all = self.load()?;
        let before = all.len();
        all.retain(|r| r.info.id != id);
        if all.len() == before {
            return Err(Error::NotFound(format!("token {id}")));
        }
        self.save(&all)
    }

    /// Resolve a presented bearer token to a principal (None = unknown or expired).
    pub fn resolve(&self, presented: &str) -> Result<Option<Principal>> {
        let h = hash_token(presented);
        let all = self.load()?;
        let Some(rec) = all.iter().find(|r| r.token_hash == h) else {
            return Ok(None);
        };
        if rec.info.expires_ms.is_some_and(|e| e <= now_ms()) {
            return Ok(None);
        }
        Ok(Some(Principal {
            id: rec.info.id.clone(),
            root: false,
            agent: rec.info.agent.clone(),
            scopes: rec.info.scopes.clone(),
        }))
    }

    /// Record `last_used_ms` (best effort, throttled to once a minute).
    pub fn touch(&self, id: &str) {
        let _g = self.lock.lock().unwrap();
        if let Ok(mut all) = self.load() {
            if let Some(r) = all.iter_mut().find(|r| r.info.id == id) {
                let now = now_ms();
                if r.info.last_used_ms.is_none_or(|t| now - t > 60_000) {
                    r.info.last_used_ms = Some(now);
                    let _ = self.save(&all);
                }
            }
        }
    }

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
        if let Ok(mut f) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.audit_path)
        {
            let _ = writeln!(f, "{line}");
        }
    }
}

/// Which access a request needs, derived from method and path.
pub fn required_access(method: &str, path: &str) -> (Option<String>, Access) {
    let rest = path.strip_prefix("/api/v1").unwrap_or(path);
    if rest.starts_with("/auth/") {
        let needed = if rest == "/auth/whoami" {
            Access::Read
        } else {
            Access::Admin
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
