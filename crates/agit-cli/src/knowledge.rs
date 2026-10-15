//! Knowledge access with server-first, local-fallback semantics, shared by
//! `agit know` and the MCP tools.

use agit_core::{Error, Repository};
use agit_protocol::local_knowledge::LocalNotes;
use agit_protocol::sync;
use agit_protocol::types::*;
use agit_protocol::Client;
use anyhow::Result;
use serde::Serialize;

/// Where an answer came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Server,
    Local,
}

pub struct Knowledge<'a> {
    repo: &'a Repository,
    remote: Option<(Client, String)>,
    pub author: String,
}

fn is_transport(e: &Error) -> bool {
    matches!(e, Error::Invalid(m) if m.starts_with("transport error"))
}

impl<'a> Knowledge<'a> {
    /// `local = true` never touches the server. Otherwise the configured
    /// remote is used when it exists; connection failures fall back to the
    /// local store.
    pub fn new(repo: &'a Repository, remote: &str, agent: Option<&str>, local: bool) -> Self {
        let remote = if local {
            None
        } else {
            sync::client_for(repo, remote)
                .ok()
                .map(|(c, n)| (c.with_agent(agent.map(str::to_string)), n))
        };
        let author = agent
            .map(str::to_string)
            .or_else(|| std::env::var("AGIT_AGENT").ok())
            .unwrap_or_else(|| {
                repo.author()
                    .map(|a| a.name)
                    .unwrap_or_else(|_| "unknown".into())
            });
        Knowledge {
            repo,
            remote,
            author,
        }
    }

    fn local(&self) -> LocalNotes<'a> {
        LocalNotes::open(self.repo)
    }

    /// Run `remote_call` when a server is configured; on transport failure
    /// (or no server) run `local_call`.
    fn either<T>(
        &self,
        remote_call: impl FnOnce(&Client, &str) -> agit_core::Result<T>,
        local_call: impl FnOnce(&LocalNotes<'a>) -> agit_core::Result<T>,
    ) -> Result<(T, Source)> {
        if let Some((c, name)) = &self.remote {
            match remote_call(c, name) {
                Ok(v) => return Ok((v, Source::Server)),
                Err(e) if is_transport(&e) => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok((local_call(&self.local())?, Source::Local))
    }

    pub fn add(&self, req: NoteCreate) -> Result<(Note, Source)> {
        let author = self.author.clone();
        self.either(
            |c, name| c.create_note(name, &req),
            |l| l.create(req.clone(), &author),
        )
    }

    pub fn search(
        &self,
        tags: &[String],
        path: Option<&str>,
        query: Option<&str>,
        limit: usize,
    ) -> Result<(Vec<NoteSummary>, Source)> {
        let mut q = format!("?limit={limit}");
        if !tags.is_empty() {
            q.push_str(&format!("&tags={}", tags.join(",")));
        }
        if let Some(p) = path {
            q.push_str(&format!("&path={}", agit_protocol::client::urlencode(p)));
        }
        if let Some(s) = query {
            q.push_str(&format!("&q={}", agit_protocol::client::urlencode(s)));
        }
        self.either(
            |c, name| c.notes(name, &q),
            |l| l.query(tags, path, query, limit),
        )
    }

    pub fn get(&self, id: &str) -> Result<(Note, Source)> {
        self.either(|c, name| c.note(name, id), |l| l.get(id))
    }

    pub fn tags(&self) -> Result<(Vec<TagCount>, Source)> {
        self.either(|c, name| c.tags(name), |l| l.tags())
    }

    pub fn context(&self, path: &str, limit: usize) -> Result<(ContextResponse, Source)> {
        self.either(|c, name| c.context(name, path), |l| l.context(path, limit))
    }

    pub fn sync(&self) -> Result<agit_protocol::local_knowledge::SyncReport> {
        let (c, name) = self.remote.as_ref().ok_or_else(|| {
            anyhow::anyhow!("no remote configured; `agit remote add origin <url>` first")
        })?;
        Ok(self.local().sync(c, name)?)
    }
}
