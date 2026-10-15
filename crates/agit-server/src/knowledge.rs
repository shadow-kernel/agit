//! Knowledge base: tagged notes linked to paths, commits and pull requests.

use std::collections::BTreeMap;
use std::fs;

use agit_core::events::now_ms;
use agit_core::lease::pattern_covers;
use agit_core::refs::write_atomic;
use agit_core::{Error, Result};
use agit_protocol::types::*;

use crate::state::RepoHandle;

pub struct NoteStore<'a> {
    repo: &'a RepoHandle,
}

#[derive(Default)]
pub struct NoteQuery {
    pub tags: Vec<String>,
    pub path: Option<String>,
    pub q: Option<String>,
    pub limit: usize,
    pub include_superseded: bool,
}

impl<'a> NoteStore<'a> {
    pub fn new(repo: &'a RepoHandle) -> Self {
        NoteStore { repo }
    }

    fn dir(&self) -> std::path::PathBuf {
        self.repo.dir.join("knowledge").join("notes")
    }

    fn path(&self, id: &str) -> std::path::PathBuf {
        self.dir().join(format!("{id}.json"))
    }

    fn rules_path(&self) -> std::path::PathBuf {
        self.repo.dir.join("knowledge").join("rules.json")
    }

    pub fn all(&self) -> Result<Vec<Note>> {
        let mut out = Vec::new();
        for e in fs::read_dir(self.dir())?.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "json") {
                if let Ok(n) = serde_json::from_slice::<Note>(&fs::read(&p)?) {
                    out.push(n);
                }
            }
        }
        out.sort_by(|a, b| b.updated_ms.cmp(&a.updated_ms));
        Ok(out)
    }

    pub fn get(&self, id: &str) -> Result<Note> {
        if id.contains(['/', '\\']) {
            return Err(Error::Invalid("invalid note id".into()));
        }
        let bytes = fs::read(self.path(id)).map_err(|_| Error::Invalid(format!("note {id} not found")))?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    fn save(&self, n: &Note) -> Result<()> {
        write_atomic(&self.path(&n.id), &serde_json::to_vec_pretty(n)?)
    }

    pub fn query(&self, q: &NoteQuery) -> Result<Vec<NoteSummary>> {
        let needle = q.q.as_ref().map(|s| s.to_lowercase());
        let mut out = Vec::new();
        for n in self.all()? {
            if !q.include_superseded && n.superseded_by.is_some() {
                continue;
            }
            if !q.tags.iter().all(|t| n.tags.iter().any(|x| x == t)) {
                continue;
            }
            if let Some(p) = &q.path {
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
            if q.limit > 0 && out.len() >= q.limit {
                break;
            }
        }
        Ok(out)
    }

    pub fn create(&self, req: NoteCreate, author: &str) -> Result<Note> {
        if req.title.trim().is_empty() {
            return Err(Error::Invalid("note needs a title".into()));
        }
        for t in &req.tags {
            validate_tag(t)?;
        }
        let _g = self.repo.write_lock.lock().unwrap();
        let now = now_ms();
        let n = Note {
            id: format!("n_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]),
            title: req.title,
            body: req.body,
            tags: dedup(req.tags),
            paths: req.paths,
            commits: req.commits,
            pulls: req.pulls,
            author: author.to_string(),
            confidence: req.confidence.unwrap_or(1.0).clamp(0.0, 1.0),
            created_ms: now,
            updated_ms: now,
            superseded_by: None,
        };
        self.save(&n)?;
        self.repo.emit("note.create", Some(author), serde_json::json!({ "id": n.id, "title": n.title, "tags": n.tags, "paths": n.paths }))?;
        Ok(n)
    }

    pub fn patch(&self, id: &str, p: NotePatch, author: &str) -> Result<Note> {
        let _g = self.repo.write_lock.lock().unwrap();
        let mut n = self.get(id)?;
        if let Some(t) = p.title { n.title = t; }
        if let Some(b) = p.body { n.body = b; }
        if let Some(tags) = p.tags {
            for t in &tags { validate_tag(t)?; }
            n.tags = dedup(tags);
        }
        if let Some(x) = p.paths { n.paths = x; }
        if let Some(x) = p.commits { n.commits = x; }
        if let Some(x) = p.pulls { n.pulls = x; }
        if let Some(c) = p.confidence { n.confidence = c.clamp(0.0, 1.0); }
        if let Some(s) = p.superseded_by { n.superseded_by = s; }
        n.updated_ms = now_ms();
        self.save(&n)?;
        self.repo.emit("note.update", Some(author), serde_json::json!({ "id": n.id }))?;
        Ok(n)
    }

    pub fn delete(&self, id: &str, author: &str) -> Result<()> {
        let _g = self.repo.write_lock.lock().unwrap();
        self.get(id)?;
        fs::remove_file(self.path(id))?;
        self.repo.emit("note.delete", Some(author), serde_json::json!({ "id": id }))?;
        Ok(())
    }

    pub fn tags(&self) -> Result<Vec<TagCount>> {
        let mut m: BTreeMap<String, usize> = BTreeMap::new();
        for n in self.all()? {
            for t in n.tags {
                *m.entry(t).or_insert(0) += 1;
            }
        }
        let mut v: Vec<TagCount> = m.into_iter().map(|(tag, count)| TagCount { tag, count }).collect();
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

    pub fn set_rules(&self, rules: Vec<TagRule>, author: &str) -> Result<Vec<TagRule>> {
        for r in &rules {
            for t in &r.tags { validate_tag(t)?; }
        }
        let _g = self.repo.write_lock.lock().unwrap();
        write_atomic(&self.rules_path(), &serde_json::to_vec_pretty(&rules)?)?;
        self.repo.emit("knowledge.rules", Some(author), serde_json::json!({ "count": rules.len() }))?;
        Ok(rules)
    }

    /// Tags that rules suggest for `path`.
    pub fn suggested_tags(&self, path: &str) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for r in self.rules()? {
            if pattern_covers(&r.pattern, path) {
                out.extend(r.tags);
            }
        }
        Ok(dedup(out))
    }
}

fn dedup(v: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    v.into_iter().filter(|t| seen.insert(t.clone())).collect()
}

/// Tags are `namespace:value` with lowercase ascii, digits, `-`, `_`, `.`, `/`.
pub fn validate_tag(t: &str) -> Result<()> {
    let ok = !t.is_empty()
        && t.len() <= 80
        && t.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, ':' | '-' | '_' | '.' | '/'))
        && !t.starts_with(':')
        && !t.ends_with(':');
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(format!("invalid tag {t:?} (use namespace:value, lowercase)")))
    }
}
