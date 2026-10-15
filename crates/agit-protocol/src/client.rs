//! Blocking HTTP client for the agit server API (used by the CLI and tests).

use std::time::Duration;

use agit_core::{Error, ObjectId, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::types::*;

#[derive(Debug, Clone)]
pub struct Client {
    /// e.g. `http://localhost:7878/api/v1`
    base: String,
    token: Option<String>,
    agent: Option<String>,
    http: ureq::Agent,
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
        let idx = url
            .rfind("/repos/")
            .ok_or_else(|| Error::Invalid(format!("not an agit repository url (expected .../api/v1/repos/<name>): {url}")))?;
        let base = url[..idx].to_string();
        let repo = url[idx + 7..].to_string();
        if repo.is_empty() || repo.contains('/') {
            return Err(Error::Invalid(format!("invalid repository name in url {url}")));
        }
        Ok(RepoUrl { base, repo })
    }
}

fn map_err(e: ureq::Error) -> Error {
    match e {
        ureq::Error::Status(status, resp) => {
            let body = resp.into_string().unwrap_or_default();
            match serde_json::from_str::<ApiErrorBody>(&body) {
                Ok(b) => match b.error.code.as_str() {
                    "ref_conflict" => Error::RefConflict {
                        name: b.error.details.as_ref().and_then(|d| d.get("name")).and_then(|v| v.as_str()).unwrap_or("?").to_string(),
                        expected: b.error.details.as_ref().and_then(|d| d.get("expected")).and_then(|v| v.as_str()).and_then(|s| s.parse().ok()),
                        actual: b.error.details.as_ref().and_then(|d| d.get("actual")).and_then(|v| v.as_str()).and_then(|s| s.parse().ok()),
                    },
                    "lease_conflict" => Error::LeaseConflict {
                        path: b.error.details.as_ref().and_then(|d| d.get("path")).and_then(|v| v.as_str()).unwrap_or("?").to_string(),
                        holder: b.error.details.as_ref().and_then(|d| d.get("holder")).and_then(|v| v.as_str()).unwrap_or("?").to_string(),
                        lease_id: b.error.details.as_ref().and_then(|d| d.get("lease_id")).and_then(|v| v.as_str()).unwrap_or("?").to_string(),
                    },
                    "merge_conflicts" => Error::MergeConflicts(
                        b.error.details.as_ref().and_then(|d| d.get("count")).and_then(|v| v.as_u64()).unwrap_or(1) as usize,
                    ),
                    "already_exists" => Error::AlreadyExists(b.error.message.into()),
                    _ => Error::Invalid(format!("server error {status} [{}]: {}", b.error.code, b.error.message)),
                },
                Err(_) => Error::Invalid(format!("server error {status}: {body}")),
            }
        }
        ureq::Error::Transport(t) => Error::Invalid(format!("transport error: {t}")),
    }
}

impl Client {
    pub fn new(base: impl Into<String>) -> Self {
        Client {
            base: base.into().trim_end_matches('/').to_string(),
            token: std::env::var("AGIT_TOKEN").ok(),
            agent: std::env::var("AGIT_AGENT").ok(),
            http: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(10))
                .timeout(Duration::from_secs(600))
                .build(),
        }
    }

    pub fn with_token(mut self, token: Option<String>) -> Self {
        if token.is_some() {
            self.token = token;
        }
        self
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
        if let Some(t) = &self.token {
            r = r.set("Authorization", &format!("Bearer {t}"));
        }
        if let Some(a) = &self.agent {
            r = r.set("X-Agit-Agent", a);
        }
        r
    }

    fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let resp = self.req("GET", path).call().map_err(map_err)?;
        resp.into_json().map_err(|e| Error::Invalid(format!("bad json from server: {e}")))
    }

    fn send_json<B: Serialize, T: DeserializeOwned>(&self, method: &str, path: &str, body: &B) -> Result<T> {
        let resp = self.req(method, path).send_json(body).map_err(map_err)?;
        resp.into_json().map_err(|e| Error::Invalid(format!("bad json from server: {e}")))
    }

    // -- repos --
    pub fn health(&self) -> Result<Health> {
        self.get_json("/health")
    }
    pub fn list_repos(&self) -> Result<Vec<RepoSummary>> {
        self.get_json("/repos")
    }
    pub fn create_repo(&self, name: &str, default_branch: Option<&str>) -> Result<RepoSummary> {
        self.send_json("POST", "/repos", &CreateRepo { name: name.into(), default_branch: default_branch.map(str::to_string) })
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
        let r: MissingResponse = self.send_json("POST", &format!("/repos/{repo}/objects/missing"), &IdList { ids: ids.to_vec() })?;
        Ok(r.missing)
    }
    pub fn upload(&self, repo: &str, stream: &[u8]) -> Result<StoreResponse> {
        let resp = self
            .req("POST", &format!("/repos/{repo}/objects"))
            .set("Content-Type", crate::stream::CONTENT_TYPE)
            .send_bytes(stream)
            .map_err(map_err)?;
        resp.into_json().map_err(|e| Error::Invalid(format!("bad json from server: {e}")))
    }
    pub fn fetch(&self, repo: &str, wants: &[ObjectId], haves: &[ObjectId]) -> Result<Vec<u8>> {
        let resp = self
            .req("POST", &format!("/repos/{repo}/fetch"))
            .send_json(&FetchRequest { wants: wants.to_vec(), haves: haves.to_vec() })
            .map_err(map_err)?;
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
    pub fn release_lease(&self, repo: &str, id: &str) -> Result<serde_json::Value> {
        let resp = self.req("DELETE", &format!("/repos/{repo}/leases/{id}")).call().map_err(map_err)?;
        resp.into_json().map_err(|e| Error::Invalid(format!("bad json from server: {e}")))
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
        self.send_json("POST", &format!("/repos/{repo}/pulls/{n}/close"), &serde_json::json!({}))
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
    pub fn tags(&self, repo: &str) -> Result<Vec<TagCount>> {
        self.get_json(&format!("/repos/{repo}/knowledge/tags"))
    }
    pub fn context(&self, repo: &str, path: &str) -> Result<ContextResponse> {
        self.get_json(&format!("/repos/{repo}/context?path={}", urlencode(path)))
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
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
