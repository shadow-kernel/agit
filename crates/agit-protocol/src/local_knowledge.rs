//! Local knowledge store: notes and tag rules inside `.agit/knowledge/`.
//!
//! Lets agents read and record knowledge without a server (`agit know
//! ... --local`, or automatically when no remote is reachable) and keeps a
//! cache of the server's notes so `agit know context` works offline.
//! `sync` pushes local-only notes to the server and refreshes the cache.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use agit_core::events::now_ms;
use agit_core::lease::pattern_covers;
use agit_core::ops::{log_store, LogOptions};
use agit_core::refs::write_atomic;
use agit_core::{Error, Repository, Result};
use serde::{Deserialize, Serialize};

use crate::client::Client;
use crate::types::*;

pub struct LocalNotes<'a> {
    repo: &'a Repository,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncReport {
    pub pushed: usize,
    pub pulled: usize,
    pub rules_pulled: usize,
}

fn validate_tag(t: &str) -> Result<()> {
    let ok = !t.is_empty()
        && t.len() <= 80
        && t.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, ':' | '-' | '_' | '.' | '/')
        })
        && t.contains(':')
        && !t.starts_with(':')
        && !t.ends_with(':');
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "invalid tag {t:?} (use namespace:value, lowercase)"
        )))
    }
}

impl<'a> LocalNotes<'a> {
    pub fn open(repo: &'a Repository) -> Self {
        LocalNotes { repo }
    }

    fn dir(&self) -> PathBuf {
        self.repo.common_dir().join("knowledge").join("notes")
    }

    fn rules_path(&self) -> PathBuf {
        self.repo.common_dir().join("knowledge").join("rules.json")
    }

    fn path(&self, id: &str) -> PathBuf {
        self.dir().join(format!("{id}.json"))
    }

    pub fn all(&self) -> Result<Vec<Note>> {
        let mut out = Vec::new();
        if let Ok(rd) = fs::read_dir(self.dir()) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "json") {
                    if let Ok(n) = serde_json::from_slice::<Note>(&fs::read(&p)?) {
                        out.push(n);
                    }
                }
            }
        }
        out.sort_by_key(|n| std::cmp::Reverse(n.updated_ms));
        Ok(out)
    }

    pub fn get(&self, id: &str) -> Result<Note> {
        if id.contains(['/', '\\']) {
            return Err(Error::Invalid("invalid note id".into()));
        }
        let bytes = fs::read(self.path(id)).map_err(|_| Error::NotFound(format!("note {id}")))?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    fn save(&self, n: &Note) -> Result<()> {
        fs::create_dir_all(self.dir())?;
        write_atomic(&self.path(&n.id), &serde_json::to_vec_pretty(n)?)
    }

    /// Local-only notes carry the `n_local_` prefix until synced.
    pub fn create(&self, req: NoteCreate, author: &str) -> Result<Note> {
        if req.title.trim().is_empty() {
            return Err(Error::Invalid("note needs a title".into()));
        }
        for t in &req.tags {
            validate_tag(t)?;
        }
        let now = now_ms();
        let n = Note {
            id: format!(
                "n_local_{}",
                &uuid::Uuid::new_v4().simple().to_string()[..10]
            ),
            title: req.title,
            body: req.body,
            tags: req.tags,
            paths: req.paths,
            commits: req.commits,
            pulls: req.pulls,
            author: author.to_string(),
            confidence: req.confidence.unwrap_or(1.0).clamp(0.0, 1.0),
            created_ms: now,
            updated_ms: now,
            superseded_by: None,
            valid_at: req.valid_at.or(self.repo.head_commit()?),
            draft: req.draft,
            stale: false,
        };
        self.save(&n)?;
        self.repo.events().append(
            "note.create",
            Some(self.repo.workspace()),
            Some(author),
            serde_json::json!({ "id": n.id, "title": n.title, "tags": n.tags, "local": true }),
        )?;
        Ok(n)
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        self.get(id)?;
        fs::remove_file(self.path(id))?;
        Ok(())
    }

    pub fn query(
        &self,
        tags: &[String],
        path: Option<&str>,
        q: Option<&str>,
        limit: usize,
    ) -> Result<Vec<NoteSummary>> {
        let needle = q.map(|s| s.to_lowercase());
        let mut out = Vec::new();
        for n in self.all()? {
            if n.superseded_by.is_some() || n.draft {
                continue;
            }
            if !tags.iter().all(|t| n.tags.iter().any(|x| x == t)) {
                continue;
            }
            if let Some(p) = path {
                if !n.paths.iter().any(|np| pattern_covers(np, p)) {
                    continue;
                }
            }
            if let Some(s) = &needle {
                let hay = format!("{}\n{}\n{}", n.title, n.body, n.tags.join(" ")).to_lowercase();
                if !hay.contains(s.as_str()) {
                    continue;
                }
            }
            out.push(NoteSummary::from(&n));
            if limit > 0 && out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    pub fn tags(&self) -> Result<Vec<TagCount>> {
        let mut m: BTreeMap<String, usize> = BTreeMap::new();
        for n in self.all()? {
            if n.draft {
                continue;
            }
            for t in n.tags {
                *m.entry(t).or_insert(0) += 1;
            }
        }
        let mut v: Vec<TagCount> = m
            .into_iter()
            .map(|(tag, count)| TagCount { tag, count })
            .collect();
        v.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.tag.cmp(&b.tag)));
        Ok(v)
    }

    pub fn rules(&self) -> Result<Vec<TagRule>> {
        match fs::read(self.rules_path()) {
            Ok(b) => Ok(serde_json::from_slice(&b)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
            Err(e) => Err(e.into()),
        }
    }

    pub fn set_rules(&self, rules: &[TagRule]) -> Result<()> {
        fs::create_dir_all(self.rules_path().parent().unwrap())?;
        write_atomic(&self.rules_path(), &serde_json::to_vec_pretty(rules)?)
    }

    /// Offline context: local notes, rule-suggested tags, recent commits on
    /// the path, active leases. Pull requests need the server.
    pub fn context(&self, path: &str, limit: usize) -> Result<ContextResponse> {
        let path = path.trim_matches('/').to_string();
        let notes = self.query(&[], Some(&path), None, limit)?;
        let mut suggested: Vec<String> = Vec::new();
        for r in self.rules()? {
            if pattern_covers(&r.pattern, &path) {
                suggested.extend(r.tags);
            }
        }
        suggested.sort();
        suggested.dedup();
        let recent_commits = match self.repo.head_commit()? {
            Some(h) => log_store(
                self.repo.store(),
                &[h],
                &LogOptions {
                    limit: Some(limit),
                    path: Some(path.clone()),
                    ..Default::default()
                },
            )?
            .into_iter()
            .map(|e| CommitEntry {
                id: e.id,
                commit: e.commit,
            })
            .collect(),
            None => vec![],
        };
        let leases: Vec<_> = self
            .repo
            .leases()
            .list()?
            .into_iter()
            .filter(|l| {
                l.paths
                    .iter()
                    .any(|p| pattern_covers(p, &path) || pattern_covers(&path, p))
            })
            .collect();
        let cutoff = now_ms() - 24 * 3600 * 1000;
        let mut agents_active: Vec<String> = leases.iter().map(|l| l.holder.clone()).collect();
        for c in &recent_commits {
            if c.commit.time_ms >= cutoff {
                if let Some(a) = &c.commit.agent {
                    agents_active.push(a.id.clone());
                }
            }
        }
        agents_active.sort();
        agents_active.dedup();
        Ok(ContextResponse {
            path,
            notes,
            suggested_tags: suggested,
            recent_commits,
            leases,
            open_pulls: vec![],
            agents_active,
            files: vec![],
        })
    }

    /// Push local-only notes to the server, then refresh the local cache with
    /// every server note (accepted, non-superseded) and the tag rules.
    pub fn sync(&self, client: &Client, repo_name: &str) -> Result<SyncReport> {
        let mut report = SyncReport::default();
        for n in self.all()? {
            if !n.id.starts_with("n_local_") {
                continue;
            }
            let created = client.create_note(
                repo_name,
                &NoteCreate {
                    title: n.title.clone(),
                    body: n.body.clone(),
                    tags: n.tags.clone(),
                    paths: n.paths.clone(),
                    commits: n.commits.clone(),
                    pulls: n.pulls.clone(),
                    confidence: Some(n.confidence),
                    // Only pin a validity commit the server actually has.
                    valid_at: match n.valid_at {
                        Some(v) => client
                            .missing(repo_name, &[v])
                            .map(|m| if m.is_empty() { Some(v) } else { None })
                            .unwrap_or(None),
                        None => None,
                    },
                    draft: n.draft,
                },
            )?;
            fs::remove_file(self.path(&n.id))?;
            self.save(&created)?;
            report.pushed += 1;
        }
        let summaries = client.notes(repo_name, "?limit=0&include_superseded=1&drafts=1")?;
        for s in summaries {
            let full = client.note(repo_name, &s.id)?;
            self.save(&full)?;
            report.pulled += 1;
        }
        if let Ok(rules) = client.get_json_public(&format!("/repos/{repo_name}/knowledge/rules")) {
            let rules: Vec<TagRule> = rules;
            report.rules_pulled = rules.len();
            self.set_rules(&rules)?;
        }
        self.repo.events().append(
            "knowledge.sync",
            Some(self.repo.workspace()),
            None,
            serde_json::to_value(&report)?,
        )?;
        Ok(report)
    }
}
