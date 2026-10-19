//! Blocking HTTP client for the agit server API (used by the CLI and tests).

// `ureq::Error` is large by design; the request closures hand it straight to
// `map_err`, so boxing it would only add noise.
#![allow(clippy::result_large_err)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agit_core::{Error, ObjectId, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::types::*;

#[derive(Debug, Clone)]
pub struct Client {
    /// e.g. `http://localhost:7878/api/v1`
    base: String,
    /// Shared between clones so an automatic sign-in benefits all of them.
    token: Arc<Mutex<Option<String>>>,
    agent: Option<String>,
    http: ureq::Agent,
    /// Sign in with an SSH key once when the server answers 401. On unless
    /// a token was given explicitly (`AGIT_TOKEN`, `with_token`) or
    /// `AGIT_AUTO_LOGIN=0`.
    auto_login: bool,
    login_tried: Arc<AtomicBool>,
    ssh_key: Option<PathBuf>,
}

/// Parsed form of a repository URL `http(s)://host[:port]/api/v1/repos/<name>`.
#[derive(Debug, Clone)]
pub struct RepoUrl {
    pub base: String,
    pub repo: String,
}

impl RepoUrl {
    pub fn parse(url: &str) -> Result<RepoUrl> {
        let url = url.trim_end_matches('/');
        let idx = url.rfind("/repos/").ok_or_else(|| {
            Error::Invalid(format!(
                "not an agit repository url (expected .../api/v1/repos/<name>): {url}"
            ))
        })?;
        let base = url[..idx].to_string();
        let repo = url[idx + 7..].to_string();
        if repo.is_empty() || repo.contains('/') {
            return Err(Error::Invalid(format!(
                "invalid repository name in url {url}"
            )));
        }
        Ok(RepoUrl { base, repo })
    }
}

fn strip_prefix(msg: &str, prefix: &str) -> String {
    msg.strip_prefix(prefix).unwrap_or(msg).to_string()
}

fn map_err(e: ureq::Error) -> Error {
    match e {
        ureq::Error::Status(status, resp) => {
            let body = resp.into_string().unwrap_or_default();
            match serde_json::from_str::<ApiErrorBody>(&body) {
                Ok(b) => match b.error.code.as_str() {
                    "ref_conflict" => Error::RefConflict {
                        name: b
                            .error
                            .details
                            .as_ref()
                            .and_then(|d| d.get("name"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("?")
                            .to_string(),
                        expected: b
                            .error
                            .details
                            .as_ref()
                            .and_then(|d| d.get("expected"))
                            .and_then(|v| v.as_str())
                            .and_then(|s| s.parse().ok()),
                        actual: b
                            .error
                            .details
                            .as_ref()
                            .and_then(|d| d.get("actual"))
                            .and_then(|v| v.as_str())
                            .and_then(|s| s.parse().ok()),
                    },
                    "lease_conflict" => Error::LeaseConflict {
                        path: b
                            .error
                            .details
                            .as_ref()
                            .and_then(|d| d.get("path"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("?")
                            .to_string(),
                        holder: b
                            .error
                            .details
                            .as_ref()
                            .and_then(|d| d.get("holder"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("?")
                            .to_string(),
                        lease_id: b
                            .error
                            .details
                            .as_ref()
                            .and_then(|d| d.get("lease_id"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("?")
                            .to_string(),
                    },
                    "merge_conflicts" => Error::MergeConflicts(
                        b.error
                            .details
                            .as_ref()
                            .and_then(|d| d.get("count"))
                            .and_then(|v| v.as_u64())
                            .unwrap_or(1) as usize,
                    ),
                    "already_exists" => Error::AlreadyExists(b.error.message.into()),
                    "policy_violation" => {
                        Error::PolicyViolation(strip_prefix(&b.error.message, "policy violation: "))
                    }
                    "forbidden" => Error::Forbidden(strip_prefix(&b.error.message, "forbidden: ")),
                    "unauthorized" => {
                        Error::Unauthorized(strip_prefix(&b.error.message, "unauthorized: "))
                    }
                    "not_found" => Error::NotFound(strip_prefix(&b.error.message, "not found: ")),
                    "lease_not_found" => {
                        Error::LeaseNotFound(strip_prefix(&b.error.message, "lease not found: "))
                    }
                    "non_fast_forward" => Error::Invalid(b.error.message),
                    _ => Error::Invalid(format!(
                        "server error {status} [{}]: {}",
                        b.error.code, b.error.message
                    )),
                },
                Err(_) => Error::Invalid(format!("server error {status}: {body}")),
            }
        }
        ureq::Error::Transport(t) => Error::Invalid(format!("transport error: {t}")),
    }
}

impl Client {
    /// A client for `base`. The bearer token comes from `AGIT_TOKEN`, else
    /// from the session cached by an earlier `agit auth login` (or automatic
    /// sign-in) for this server.
    pub fn new(base: impl Into<String>) -> Self {
        let base = base.into().trim_end_matches('/').to_string();
        let env_token = std::env::var("AGIT_TOKEN").ok().filter(|t| !t.is_empty());
        let explicit = env_token.is_some();
        let token = env_token.or_else(|| crate::sshauth::cached_session(&base).map(|s| s.token));
        let auto_login = !explicit
            && !matches!(
                std::env::var("AGIT_AUTO_LOGIN").as_deref(),
                Ok("0") | Ok("off") | Ok("false") | Ok("no")
            );
        Client {
            base,
            token: Arc::new(Mutex::new(token)),
            agent: std::env::var("AGIT_AGENT").ok(),
            http: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(10))
                .timeout(Duration::from_secs(600))
                .build(),
            auto_login,
            login_tried: Arc::new(AtomicBool::new(false)),
            ssh_key: std::env::var_os("AGIT_SSH_KEY").map(PathBuf::from),
        }
    }

    /// Use this token (and do not try an SSH sign-in when it is refused).
    pub fn with_token(self, token: Option<String>) -> Self {
        if let Some(t) = token {
            *self.token.lock().unwrap() = Some(t);
            return Client {
                auto_login: false,
                ..self
            };
        }
        self
    }

    /// Use shorter network timeouts (for calls on the hot path, such as the
    /// lease check before every commit).
    pub fn with_timeouts(mut self, connect: Duration, total: Duration) -> Self {
        self.http = ureq::AgentBuilder::new()
            .timeout_connect(connect)
            .timeout(total)
            .build();
        self
    }

    /// Prefer this SSH private key for automatic sign-in.
    pub fn with_ssh_key(mut self, path: Option<PathBuf>) -> Self {
        if path.is_some() {
            self.ssh_key = path;
        }
        self
    }

    /// Turn the automatic SSH sign-in on 401 on or off.
    pub fn with_auto_login(mut self, on: bool) -> Self {
        self.auto_login = on;
        self
    }

    pub fn token(&self) -> Option<String> {
        self.token.lock().unwrap().clone()
    }

    pub fn set_token(&self, token: Option<String>) {
        *self.token.lock().unwrap() = token;
    }

    /// Sign in with an SSH key (explicit path, else the usual candidates),
    /// keep the session token on this client and cache it for later
    /// processes. Returns the token and a description of the key used.
    pub fn login_with_ssh_key(
        &self,
        key: Option<&std::path::Path>,
        ttl_secs: Option<u64>,
    ) -> Result<(TokenCreated, crate::sshauth::Identity)> {
        let key = key.or(self.ssh_key.as_deref());
        let (created, id) = crate::sshauth::login_auto(self, key, ttl_secs)?;
        self.set_token(Some(created.token.clone()));
        crate::sshauth::remember_session(&self.base, &created, Some(id.public.fingerprint()));
        Ok((created, id))
    }

    /// One automatic sign-in attempt after a 401; `Ok(true)` when a token
    /// was obtained and the request should be retried.
    fn try_auto_login(&self) -> std::result::Result<bool, String> {
        if !self.auto_login || self.login_tried.swap(true, Ordering::SeqCst) {
            return Ok(false);
        }
        match self.login_with_ssh_key(None, None) {
            Ok((created, id)) => {
                eprintln!(
                    "agit: signed in to {} as {} with ssh key {} ({})",
                    self.base,
                    created
                        .info
                        .agent
                        .clone()
                        .unwrap_or_else(|| created.info.name.clone()),
                    id.public.fingerprint(),
                    id.source
                );
                Ok(true)
            }
            Err(e) => Err(e.to_string()),
        }
    }

    /// Run a request, signing in with an SSH key and retrying once when the
    /// server answers 401 without a usable token.
    fn exec<F>(&self, method: &str, path: &str, f: F) -> Result<ureq::Response>
    where
        F: Fn(ureq::Request) -> std::result::Result<ureq::Response, ureq::Error>,
    {
        match f(self.req(method, path)) {
            Ok(r) => Ok(r),
            Err(e) => {
                let err = map_err(e);
                let Error::Unauthorized(msg) = &err else {
                    return Err(err);
                };
                match self.try_auto_login() {
                    Ok(true) => f(self.req(method, path)).map_err(map_err),
                    Ok(false) => Err(err),
                    Err(reason) => Err(Error::Unauthorized(format!("{msg} ({reason})"))),
                }
            }
        }
    }

    pub fn with_agent(mut self, agent: Option<String>) -> Self {
        if agent.is_some() {
            self.agent = agent;
        }
        self
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    fn req(&self, method: &str, path: &str) -> ureq::Request {
        let mut r = self.http.request(method, &format!("{}{}", self.base, path));
        if let Some(t) = self.token.lock().unwrap().as_deref() {
            r = r.set("Authorization", &format!("Bearer {t}"));
        }
        if let Some(a) = &self.agent {
            r = r.set("X-Agit-Agent", a);
        }
        r
    }

    /// Generic GET for endpoints without a dedicated method.
    pub fn get_json_public<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.get_json(path)
    }

    fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let resp = self.exec("GET", path, |r| r.call())?;
        resp.into_json()
            .map_err(|e| Error::Invalid(format!("bad json from server: {e}")))
    }

    fn send_json<B: Serialize, T: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let resp = self.exec(method, path, |r| r.send_json(body))?;
        resp.into_json()
            .map_err(|e| Error::Invalid(format!("bad json from server: {e}")))
    }

    /// A bodiless non-GET request (DELETE, POST without payload) returning JSON.
    fn call_json<T: DeserializeOwned>(&self, method: &str, path: &str) -> Result<T> {
        let resp = self.exec(method, path, |r| r.call())?;
        resp.into_json()
            .map_err(|e| Error::Invalid(format!("bad json from server: {e}")))
    }

    // -- repos --
    pub fn health(&self) -> Result<Health> {
        self.get_json("/health")
    }
    pub fn list_repos(&self) -> Result<Vec<RepoSummary>> {
        self.get_json("/repos")
    }
    pub fn create_repo(&self, name: &str, default_branch: Option<&str>) -> Result<RepoSummary> {
        self.send_json(
            "POST",
            "/repos",
            &CreateRepo {
                name: name.into(),
                default_branch: default_branch.map(str::to_string),
            },
        )
    }
    pub fn repo(&self, repo: &str) -> Result<RepoSummary> {
        self.get_json(&format!("/repos/{repo}"))
    }

    // -- refs --
    pub fn refs(&self, repo: &str, prefix: &str) -> Result<Vec<RefEntry>> {
        self.get_json(&format!("/repos/{repo}/refs?prefix={prefix}"))
    }
    pub fn update_refs(&self, repo: &str, req: &RefUpdateRequest) -> Result<RefUpdateResponse> {
        self.send_json("POST", &format!("/repos/{repo}/refs/update"), req)
    }

    // -- objects --
    pub fn missing(&self, repo: &str, ids: &[ObjectId]) -> Result<Vec<ObjectId>> {
        let r: MissingResponse = self.send_json(
            "POST",
            &format!("/repos/{repo}/objects/missing"),
            &IdList { ids: ids.to_vec() },
        )?;
        Ok(r.missing)
    }
    pub fn upload(&self, repo: &str, stream: &[u8]) -> Result<StoreResponse> {
        let resp = self.exec("POST", &format!("/repos/{repo}/objects"), |r| {
            r.set("Content-Type", crate::stream::CONTENT_TYPE)
                .send_bytes(stream)
        })?;
        resp.into_json()
            .map_err(|e| Error::Invalid(format!("bad json from server: {e}")))
    }
    pub fn fetch(&self, repo: &str, wants: &[ObjectId], haves: &[ObjectId]) -> Result<Vec<u8>> {
        let body = FetchRequest {
            wants: wants.to_vec(),
            haves: haves.to_vec(),
        };
        let resp = self.exec("POST", &format!("/repos/{repo}/fetch"), |r| {
            r.send_json(&body)
        })?;
        let mut out = Vec::new();
        use std::io::Read;
        resp.into_reader().read_to_end(&mut out)?;
        Ok(out)
    }

    // -- leases --
    pub fn leases(&self, repo: &str) -> Result<Vec<LeaseJson>> {
        self.get_json(&format!("/repos/{repo}/leases"))
    }
    pub fn claim_lease(&self, repo: &str, req: &LeaseCreate) -> Result<LeaseJson> {
        self.send_json("POST", &format!("/repos/{repo}/leases"), req)
    }
    pub fn renew_lease(&self, repo: &str, id: &str, ttl_secs: u64) -> Result<LeaseJson> {
        self.send_json(
            "POST",
            &format!("/repos/{repo}/leases/{id}/renew"),
            &LeaseRenew { ttl_secs },
        )
    }
    /// `Ok(())` when `holder` may modify `paths` on the server.
    pub fn check_lease(&self, repo: &str, holder: &str, paths: &[String]) -> Result<()> {
        let _: serde_json::Value = self.send_json(
            "POST",
            &format!("/repos/{repo}/leases/check"),
            &LeaseCheck {
                holder: holder.to_string(),
                paths: paths.to_vec(),
            },
        )?;
        Ok(())
    }
    pub fn release_lease(&self, repo: &str, id: &str) -> Result<serde_json::Value> {
        self.call_json("DELETE", &format!("/repos/{repo}/leases/{id}"))
    }

    // -- pulls --
    pub fn pulls(&self, repo: &str, state: Option<&str>) -> Result<Vec<PullRequest>> {
        let q = state.map(|s| format!("?state={s}")).unwrap_or_default();
        self.get_json(&format!("/repos/{repo}/pulls{q}"))
    }
    pub fn create_pull(&self, repo: &str, req: &PullCreate) -> Result<PullRequest> {
        self.send_json("POST", &format!("/repos/{repo}/pulls"), req)
    }
    pub fn pull(&self, repo: &str, n: u64) -> Result<PullDetail> {
        self.get_json(&format!("/repos/{repo}/pulls/{n}"))
    }
    pub fn review_pull(&self, repo: &str, n: u64, req: &ReviewCreate) -> Result<PullRequest> {
        self.send_json("POST", &format!("/repos/{repo}/pulls/{n}/reviews"), req)
    }
    pub fn comment_pull(&self, repo: &str, n: u64, req: &CommentCreate) -> Result<Comment> {
        self.send_json("POST", &format!("/repos/{repo}/pulls/{n}/comments"), req)
    }
    pub fn merge_pull(&self, repo: &str, n: u64, req: &MergeRequest) -> Result<MergeResponse> {
        self.send_json("POST", &format!("/repos/{repo}/pulls/{n}/merge"), req)
    }
    pub fn close_pull(&self, repo: &str, n: u64) -> Result<PullRequest> {
        self.send_json(
            "POST",
            &format!("/repos/{repo}/pulls/{n}/close"),
            &serde_json::json!({}),
        )
    }

    // -- knowledge --
    pub fn notes(&self, repo: &str, query: &str) -> Result<Vec<NoteSummary>> {
        self.get_json(&format!("/repos/{repo}/knowledge/notes{query}"))
    }
    pub fn note(&self, repo: &str, id: &str) -> Result<Note> {
        self.get_json(&format!("/repos/{repo}/knowledge/notes/{id}"))
    }
    pub fn create_note(&self, repo: &str, req: &NoteCreate) -> Result<Note> {
        self.send_json("POST", &format!("/repos/{repo}/knowledge/notes"), req)
    }
    pub fn patch_note(&self, repo: &str, id: &str, patch: &NotePatch) -> Result<Note> {
        self.send_json(
            "PATCH",
            &format!("/repos/{repo}/knowledge/notes/{id}"),
            patch,
        )
    }
    pub fn delete_note(&self, repo: &str, id: &str) -> Result<serde_json::Value> {
        self.call_json("DELETE", &format!("/repos/{repo}/knowledge/notes/{id}"))
    }
    /// Notes whose covered paths changed since their `valid_at` (against `rev`, default HEAD).
    pub fn stale_notes(&self, repo: &str, rev: Option<&str>) -> Result<Vec<StaleNote>> {
        let q = rev
            .map(|r| format!("?rev={}", urlencode(r)))
            .unwrap_or_default();
        self.get_json(&format!("/repos/{repo}/knowledge/stale{q}"))
    }
    /// Mark a note as valid at `rev` (default HEAD).
    pub fn validate_note(&self, repo: &str, id: &str, rev: Option<&str>) -> Result<Note> {
        self.send_json(
            "POST",
            &format!("/repos/{repo}/knowledge/notes/{id}/validate"),
            &ValidateRequest {
                rev: rev.map(str::to_string),
            },
        )
    }
    /// Accept a draft note, optionally editing it with `patch` on the way.
    pub fn accept_note(&self, repo: &str, id: &str, patch: Option<&NotePatch>) -> Result<Note> {
        let body = match patch {
            Some(p) => serde_json::to_value(p)?,
            None => serde_json::json!({}),
        };
        self.send_json(
            "POST",
            &format!("/repos/{repo}/knowledge/notes/{id}/accept"),
            &body,
        )
    }
    pub fn webhooks(&self, repo: &str) -> Result<Vec<Webhook>> {
        self.get_json(&format!("/repos/{repo}/webhooks"))
    }
    pub fn create_webhook(&self, repo: &str, req: &WebhookCreate) -> Result<Webhook> {
        self.send_json("POST", &format!("/repos/{repo}/webhooks"), req)
    }
    pub fn delete_webhook(&self, repo: &str, id: &str) -> Result<serde_json::Value> {
        self.call_json("DELETE", &format!("/repos/{repo}/webhooks/{id}"))
    }
    pub fn whoami(&self) -> Result<WhoAmI> {
        self.get_json("/auth/whoami")
    }
    /// A single-use nonce to sign with an SSH key (no token needed).
    pub fn auth_challenge(&self) -> Result<Challenge> {
        self.call_json("POST", "/auth/challenge")
    }
    /// Redeem a signed challenge for a session token (no token needed).
    pub fn auth_session(&self, req: &SessionCreate) -> Result<TokenCreated> {
        self.send_json("POST", "/auth/session", req)
    }
    /// Revoke the token this client is using and drop it.
    pub fn logout(&self) -> Result<serde_json::Value> {
        let r: serde_json::Value = self.call_json("POST", "/auth/logout")?;
        self.set_token(None);
        crate::sshauth::forget_session(&self.base);
        Ok(r)
    }
    pub fn authorized_keys(&self) -> Result<Vec<AuthorizedKey>> {
        self.get_json("/auth/keys")
    }
    pub fn add_authorized_key(&self, req: &AuthorizedKeyCreate) -> Result<AuthorizedKey> {
        self.send_json("POST", "/auth/keys", req)
    }
    pub fn delete_authorized_key(&self, id: &str) -> Result<serde_json::Value> {
        self.call_json("DELETE", &format!("/auth/keys/{id}"))
    }
    pub fn create_token(&self, req: &TokenCreate) -> Result<TokenCreated> {
        self.send_json("POST", "/auth/tokens", req)
    }
    pub fn tokens(&self) -> Result<Vec<TokenInfo>> {
        self.get_json("/auth/tokens")
    }
    pub fn delete_token(&self, id: &str) -> Result<serde_json::Value> {
        self.call_json("DELETE", &format!("/auth/tokens/{id}"))
    }
    pub fn keys(&self, repo: &str) -> Result<Vec<KeyEntry>> {
        self.get_json(&format!("/repos/{repo}/keys"))
    }
    pub fn register_key(&self, repo: &str, agent: &str, public_key: &str) -> Result<KeyEntry> {
        self.send_json(
            "POST",
            &format!("/repos/{repo}/keys"),
            &KeyCreate {
                agent: agent.into(),
                public_key: public_key.into(),
            },
        )
    }
    pub fn delete_key(&self, repo: &str, key_id: &str) -> Result<serde_json::Value> {
        self.call_json("DELETE", &format!("/repos/{repo}/keys/{key_id}"))
    }
    pub fn commit_detail(&self, repo: &str, rev: &str) -> Result<CommitDetail> {
        self.get_json(&format!("/repos/{repo}/commits/{rev}"))
    }
    pub fn policy(&self, repo: &str) -> Result<Policy> {
        self.get_json(&format!("/repos/{repo}/policy"))
    }
    pub fn set_policy(&self, repo: &str, policy: &Policy) -> Result<Policy> {
        self.send_json("PUT", &format!("/repos/{repo}/policy"), policy)
    }
    pub fn tags(&self, repo: &str) -> Result<Vec<TagCount>> {
        self.get_json(&format!("/repos/{repo}/knowledge/tags"))
    }
    pub fn context(&self, repo: &str, path: &str) -> Result<ContextResponse> {
        self.get_json(&format!("/repos/{repo}/context?path={}", urlencode(path)))
    }
    /// Context plus file contents, trimmed to `budget` approximate tokens (default 6000).
    pub fn context_pack(
        &self,
        repo: &str,
        path: &str,
        budget: Option<usize>,
    ) -> Result<ContextResponse> {
        let b = budget.map(|b| format!("&budget={b}")).unwrap_or_default();
        self.get_json(&format!(
            "/repos/{repo}/context?path={}&include=files{b}",
            urlencode(path)
        ))
    }

    // -- misc --
    pub fn events(&self, repo: &str, since: u64) -> Result<Vec<agit_core::events::Event>> {
        self.get_json(&format!("/repos/{repo}/events?since={since}"))
    }
}

pub fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
