//! Knowledge base: tagged notes linked to paths, commits and pull requests.
//!
//! Notes are JSON files under `<repo>/knowledge/notes`. On top of the files
//! this module keeps, per repository and in memory, a BM25 full-text index
//! (built lazily at the first `q=` query and maintained on every mutation)
//! and a small cache of "which paths changed between commit A and B" used
//! to decide whether a note is stale.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::sync::{Arc, Mutex, RwLock};

use agit_core::diff::{diff_trees, Change};
use agit_core::events::now_ms;
use agit_core::lease::pattern_covers;
use agit_core::refs::write_atomic;
use agit_core::{Error, ObjectId, Result};
use agit_protocol::types::*;

use crate::state::RepoHandle;

// ---------------------------------------------------------------------------
// Tokenizer and BM25 index
// ---------------------------------------------------------------------------

const BM25_K1: f32 = 1.2;
const BM25_B: f32 = 0.75;
const CHANGED_CACHE_MAX: usize = 256;

/// `(from, to)` -> paths that differ between the two commits' trees.
type ChangedCache = HashMap<(ObjectId, ObjectId), Arc<Vec<String>>>;

/// Per-repository in-memory caches, hung off `RepoHandle`.
#[derive(Default)]
pub struct KnowledgeCache {
    index: RwLock<Option<SearchIndex>>,
    changed: Mutex<ChangedCache>,
}

/// Inverted index over note title, body, tags and paths.
#[derive(Default)]
pub struct SearchIndex {
    /// term -> note id -> weighted term frequency
    postings: HashMap<String, HashMap<String, f32>>,
    /// note id -> weighted document length
    lengths: HashMap<String, f32>,
}

impl SearchIndex {
    fn build(notes: &[Note]) -> SearchIndex {
        let mut ix = SearchIndex::default();
        for n in notes {
            ix.upsert(n);
        }
        ix
    }

    fn upsert(&mut self, n: &Note) {
        self.remove(&n.id);
        let tf = doc_terms(n);
        let len: f32 = tf.values().sum();
        for (term, w) in tf {
            self.postings
                .entry(term)
                .or_default()
                .insert(n.id.clone(), w);
        }
        self.lengths.insert(n.id.clone(), len.max(1.0));
    }

    fn remove(&mut self, id: &str) {
        if self.lengths.remove(id).is_none() {
            return;
        }
        for m in self.postings.values_mut() {
            m.remove(id);
        }
        self.postings.retain(|_, m| !m.is_empty());
    }

    /// Ranked `(note id, score)` pairs, best first. Terms match exactly; terms
    /// of three or more characters also match as prefixes at half weight.
    pub fn search(&self, query: &str) -> Vec<(String, f32)> {
        let n = self.lengths.len() as f32;
        if n == 0.0 {
            return vec![];
        }
        let avg = self.lengths.values().sum::<f32>() / n;
        let terms: BTreeSet<String> = tokenize(query).into_iter().collect();
        let mut scores: HashMap<&str, f32> = HashMap::new();
        for t in &terms {
            let mut matched: Vec<(&HashMap<String, f32>, f32)> = Vec::new();
            if let Some(p) = self.postings.get(t) {
                matched.push((p, 1.0));
            }
            if t.chars().count() >= 3 {
                for (term, p) in &self.postings {
                    if term != t && term.starts_with(t.as_str()) {
                        matched.push((p, 0.5));
                    }
                }
            }
            for (post, weight) in matched {
                let df = post.len() as f32;
                let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
                for (doc, tf) in post {
                    let dl = self.lengths.get(doc).copied().unwrap_or(1.0);
                    let s = idf * tf * (BM25_K1 + 1.0)
                        / (tf + BM25_K1 * (1.0 - BM25_B + BM25_B * dl / avg));
                    *scores.entry(doc.as_str()).or_insert(0.0) += s * weight;
                }
            }
        }
        let mut v: Vec<(String, f32)> = scores
            .into_iter()
            .map(|(d, s)| (d.to_string(), s))
            .collect();
        v.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        v
    }
}

fn doc_terms(n: &Note) -> HashMap<String, f32> {
    let mut tf: HashMap<String, f32> = HashMap::new();
    let mut add = |text: &str, w: f32| {
        for t in tokenize(text) {
            *tf.entry(t).or_insert(0.0) += w;
        }
    };
    add(&n.title, 3.0);
    add(&n.body, 1.0);
    for t in &n.tags {
        add(t, 2.0);
    }
    for p in &n.paths {
        add(p, 1.0);
    }
    tf
}

/// Lowercase tokens: unicode alphanumeric runs, with camelCase and
/// snake_case split into their parts (the unsplit word is kept as well), and
/// `namespace:value` tags kept as one token in addition to their parts.
pub fn tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for word in text.split(char::is_whitespace) {
        let word = word.trim_matches(|c: char| !c.is_alphanumeric());
        if word.is_empty() {
            continue;
        }
        let lower = word.to_lowercase();
        if lower.contains(':') && validate_tag(&lower).is_ok() {
            out.push(lower.clone());
        }
        let mut run = String::new();
        for c in word.chars() {
            if c.is_alphanumeric() {
                run.push(c);
            } else {
                push_run(&run, &mut out);
                run.clear();
            }
        }
        push_run(&run, &mut out);
    }
    out
}

fn push_run(run: &str, out: &mut Vec<String>) {
    if run.is_empty() {
        return;
    }
    let chars: Vec<char> = run.chars().collect();
    let mut parts: Vec<String> = Vec::new();
    let mut start = 0;
    for i in 1..chars.len() {
        let prev = chars[i - 1];
        let cur = chars[i];
        let boundary = cur.is_uppercase()
            && (prev.is_lowercase()
                || prev.is_numeric()
                || (prev.is_uppercase() && chars.get(i + 1).is_some_and(|c| c.is_lowercase())));
        if boundary {
            parts.push(chars[start..i].iter().collect());
            start = i;
        }
    }
    parts.push(chars[start..].iter().collect());
    if parts.len() > 1 {
        out.push(run.to_lowercase());
    }
    for p in parts {
        if p.chars().count() >= 2 {
            out.push(p.to_lowercase());
        }
    }
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

pub struct NoteStore<'a> {
    repo: &'a RepoHandle,
}

#[derive(Default)]
pub struct NoteQuery {
    pub tags: Vec<String>,
    pub path: Option<String>,
    pub q: Option<String>,
    pub commit: Option<ObjectId>,
    pub pull: Option<u64>,
    pub limit: usize,
    pub include_superseded: bool,
    pub include_drafts: bool,
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

    /// Every note as stored (no staleness computed), newest update first.
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
        out.sort_by_key(|n| std::cmp::Reverse(n.updated_ms));
        Ok(out)
    }

    /// Raw note as stored.
    pub fn load(&self, id: &str) -> Result<Note> {
        if id.contains(['/', '\\']) {
            return Err(Error::Invalid("invalid note id".into()));
        }
        let bytes = fs::read(self.path(id)).map_err(|_| Error::NotFound(format!("note {id}")))?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// Note with `stale` computed against HEAD.
    pub fn get(&self, id: &str) -> Result<Note> {
        let mut n = self.load(id)?;
        let head = self.repo.head()?;
        self.mark_stale(&mut n, head)?;
        Ok(n)
    }

    fn save(&self, n: &Note) -> Result<()> {
        let mut on_disk = n.clone();
        on_disk.stale = false;
        write_atomic(&self.path(&n.id), &serde_json::to_vec_pretty(&on_disk)?)?;
        if let Some(ix) = self.repo.knowledge.index.write().unwrap().as_mut() {
            ix.upsert(n);
        }
        Ok(())
    }

    fn with_index<T>(&self, f: impl FnOnce(&SearchIndex) -> T) -> Result<T> {
        if let Some(ix) = self.repo.knowledge.index.read().unwrap().as_ref() {
            return Ok(f(ix));
        }
        let built = SearchIndex::build(&self.all()?);
        let mut slot = self.repo.knowledge.index.write().unwrap();
        let ix = slot.get_or_insert(built);
        Ok(f(ix))
    }

    // -- staleness ----------------------------------------------------------

    fn changed_between(&self, from: ObjectId, to: ObjectId) -> Result<Arc<Vec<String>>> {
        if from == to {
            return Ok(Arc::new(vec![]));
        }
        if let Some(v) = self.repo.knowledge.changed.lock().unwrap().get(&(from, to)) {
            return Ok(v.clone());
        }
        let store = &self.repo.store;
        // A `valid_at` the server does not know (e.g. a note imported from
        // elsewhere) cannot be compared; treat the note as not stale.
        let Ok(a) = store.read_commit(&from) else {
            return Ok(Arc::new(vec![]));
        };
        let b = store.read_commit(&to)?;
        let paths: Vec<String> = diff_trees(store, Some(&a.tree), Some(&b.tree))?
            .into_iter()
            .map(|c| c.path)
            .collect();
        let paths = Arc::new(paths);
        let mut cache = self.repo.knowledge.changed.lock().unwrap();
        if cache.len() >= CHANGED_CACHE_MAX {
            cache.clear();
        }
        cache.insert((from, to), paths.clone());
        Ok(paths)
    }

    /// Paths covered by the note's `paths` that changed between `valid_at` and `head`.
    pub fn changed_paths(&self, n: &Note, head: Option<ObjectId>) -> Result<Vec<String>> {
        let (Some(v), Some(h)) = (n.valid_at, head) else {
            return Ok(vec![]);
        };
        if n.paths.is_empty() || v == h {
            return Ok(vec![]);
        }
        let changed = self.changed_between(v, h)?;
        Ok(changed
            .iter()
            .filter(|p| n.paths.iter().any(|pat| pattern_covers(pat, p)))
            .cloned()
            .collect())
    }

    fn mark_stale(&self, n: &mut Note, head: Option<ObjectId>) -> Result<()> {
        n.stale = !self.changed_paths(n, head)?.is_empty();
        Ok(())
    }

    fn summary(
        &self,
        mut n: Note,
        head: Option<ObjectId>,
        score: Option<f32>,
    ) -> Result<NoteSummary> {
        self.mark_stale(&mut n, head)?;
        let mut s = NoteSummary::from(&n);
        s.score = score;
        Ok(s)
    }

    /// Stale notes (non-superseded; drafts only when `include_drafts`) against `rev`.
    pub fn stale(&self, rev: Option<&str>, include_drafts: bool) -> Result<Vec<StaleNote>> {
        let head = match rev {
            Some(r) => Some(self.repo.resolve(r)?),
            None => self.repo.head()?,
        };
        let mut out = Vec::new();
        for mut n in self.all()? {
            if n.superseded_by.is_some() || (n.draft && !include_drafts) {
                continue;
            }
            let changed = self.changed_paths(&n, head)?;
            if changed.is_empty() {
                continue;
            }
            n.stale = true;
            out.push(StaleNote {
                note: NoteSummary::from(&n),
                changed_paths: changed,
            });
        }
        Ok(out)
    }

    // -- queries ------------------------------------------------------------

    fn matches(n: &Note, q: &NoteQuery) -> bool {
        if !q.include_superseded && n.superseded_by.is_some() {
            return false;
        }
        if !q.include_drafts && n.draft {
            return false;
        }
        if !q.tags.iter().all(|t| n.tags.iter().any(|x| x == t)) {
            return false;
        }
        if let Some(p) = &q.path {
            if !n.paths.iter().any(|np| pattern_covers(np, p)) {
                return false;
            }
        }
        if let Some(c) = &q.commit {
            if !n.commits.contains(c) {
                return false;
            }
        }
        if let Some(p) = q.pull {
            if !n.pulls.contains(&p) {
                return false;
            }
        }
        true
    }

    pub fn query(&self, q: &NoteQuery) -> Result<Vec<NoteSummary>> {
        let head = self.repo.head()?;
        let notes = self.all()?;
        let candidates: Vec<(Note, Option<f32>)> = match &q.q {
            Some(text) => {
                let ranked = self.with_index(|ix| ix.search(text))?;
                let mut by_id: HashMap<String, Note> =
                    notes.into_iter().map(|n| (n.id.clone(), n)).collect();
                ranked
                    .into_iter()
                    .filter_map(|(id, score)| by_id.remove(&id).map(|n| (n, Some(score))))
                    .collect()
            }
            None => notes.into_iter().map(|n| (n, None)).collect(),
        };
        let mut out = Vec::new();
        for (n, score) in candidates {
            if !Self::matches(&n, q) {
                continue;
            }
            out.push(self.summary(n, head, score)?);
            if q.limit > 0 && out.len() >= q.limit {
                break;
            }
        }
        Ok(out)
    }

    /// Notes for a commit detail: notes linking `id` directly (drafts
    /// included), then accepted notes covering a changed path. At most `cap`.
    pub fn for_commit(
        &self,
        id: ObjectId,
        changes: &[Change],
        cap: usize,
    ) -> Result<Vec<NoteSummary>> {
        let head = self.repo.head()?;
        let mut linked = Vec::new();
        let mut covering = Vec::new();
        for n in self.all()? {
            if n.superseded_by.is_some() {
                continue;
            }
            if n.commits.contains(&id) {
                linked.push(n);
            } else if !n.draft
                && changes
                    .iter()
                    .any(|c| n.paths.iter().any(|p| pattern_covers(p, &c.path)))
            {
                covering.push(n);
            }
        }
        let mut out = Vec::new();
        for n in linked.into_iter().chain(covering) {
            if out.len() >= cap {
                break;
            }
            out.push(self.summary(n, head, None)?);
        }
        Ok(out)
    }

    // -- mutations ----------------------------------------------------------

    pub fn create(&self, req: NoteCreate, author: &str) -> Result<Note> {
        if req.title.trim().is_empty() {
            return Err(Error::Invalid("note needs a title".into()));
        }
        for t in &req.tags {
            validate_tag(t)?;
        }
        let valid_at = match req.valid_at {
            Some(v) => {
                if !self.repo.store.contains(&v) {
                    return Err(Error::ObjectNotFound(v));
                }
                Some(v)
            }
            None => self.repo.head()?,
        };
        let now = now_ms();
        let n = Note {
            id: new_id(),
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
            valid_at,
            draft: req.draft,
            stale: false,
        };
        let _g = self.repo.write_lock.lock().unwrap();
        self.save(&n)?;
        self.repo.emit(
            "note.create",
            Some(author),
            serde_json::json!({ "id": n.id, "title": n.title, "tags": n.tags, "paths": n.paths, "draft": n.draft }),
        )?;
        Ok(n)
    }

    /// Draft note summarizing a merged pull request. Called by the merge
    /// hook after the merge is committed; must not hold the repo write lock.
    pub fn draft_from_merge(
        &self,
        pr: &PullRequest,
        merged: ObjectId,
        changes: &[Change],
        author: &str,
    ) -> Result<Note> {
        let mut body = String::new();
        if let Some(i) = &pr.intent {
            body.push_str(&format!("Intent: {i}\n"));
        }
        if let Some(t) = &pr.task {
            body.push_str(&format!("Task: {t}\n"));
        }
        if !pr.body.trim().is_empty() {
            body.push_str(&format!("\n{}\n", pr.body.trim()));
        }
        body.push_str("\nChanged files:\n");
        for c in changes {
            body.push_str(&format!("- {} ({})\n", c.path, c.kind.letter()));
        }
        let mut latest: BTreeMap<&str, &Review> = BTreeMap::new();
        for r in &pr.reviews {
            latest.insert(&r.reviewer, r);
        }
        if !latest.is_empty() {
            body.push_str("\nReviews:\n");
            for (who, r) in latest {
                let decision = match r.decision {
                    ReviewDecision::Approve => "approve",
                    ReviewDecision::RequestChanges => "request_changes",
                    ReviewDecision::Comment => "comment",
                };
                if r.body.trim().is_empty() {
                    body.push_str(&format!("- {who}: {decision}\n"));
                } else {
                    body.push_str(&format!("- {who}: {decision} - {}\n", r.body.trim()));
                }
            }
        }
        let mut tags = vec!["kind:change".to_string()];
        let mut paths = BTreeSet::new();
        for c in changes {
            tags.extend(self.suggested_tags(&c.path)?);
            match c.path.rsplit_once('/') {
                Some((dir, _)) => paths.insert(format!("{dir}/**")),
                None => paths.insert(c.path.clone()),
            };
        }
        let now = now_ms();
        let n = Note {
            id: new_id(),
            title: pr.title.clone(),
            body,
            tags: dedup(tags),
            paths: paths.into_iter().collect(),
            commits: vec![merged],
            pulls: vec![pr.number],
            author: author.to_string(),
            confidence: 0.4,
            created_ms: now,
            updated_ms: now,
            superseded_by: None,
            valid_at: Some(merged),
            draft: true,
            stale: false,
        };
        let _g = self.repo.write_lock.lock().unwrap();
        self.save(&n)?;
        self.repo.emit(
            "note.draft",
            Some(author),
            serde_json::json!({ "id": n.id, "title": n.title, "pull": pr.number, "commit": merged, "tags": n.tags, "paths": n.paths }),
        )?;
        Ok(n)
    }

    fn apply_patch(n: &mut Note, p: NotePatch) -> Result<()> {
        if let Some(t) = p.title {
            if t.trim().is_empty() {
                return Err(Error::Invalid("note needs a title".into()));
            }
            n.title = t;
        }
        if let Some(b) = p.body {
            n.body = b;
        }
        if let Some(tags) = p.tags {
            for t in &tags {
                validate_tag(t)?;
            }
            n.tags = dedup(tags);
        }
        if let Some(x) = p.paths {
            n.paths = x;
        }
        if let Some(x) = p.commits {
            n.commits = x;
        }
        if let Some(x) = p.pulls {
            n.pulls = x;
        }
        if let Some(c) = p.confidence {
            n.confidence = c.clamp(0.0, 1.0);
        }
        if let Some(s) = p.superseded_by {
            n.superseded_by = s;
        }
        n.updated_ms = now_ms();
        Ok(())
    }

    pub fn patch(&self, id: &str, p: NotePatch, author: &str) -> Result<Note> {
        let _g = self.repo.write_lock.lock().unwrap();
        let mut n = self.load(id)?;
        Self::apply_patch(&mut n, p)?;
        self.save(&n)?;
        self.repo.emit(
            "note.update",
            Some(author),
            serde_json::json!({ "id": n.id }),
        )?;
        drop(_g);
        self.mark_stale(&mut n, self.repo.head()?)?;
        Ok(n)
    }

    /// Set `valid_at` to `rev` (default HEAD): the author confirms the note
    /// still holds at that commit.
    pub fn validate(&self, id: &str, rev: Option<&str>, author: &str) -> Result<Note> {
        let at = match rev {
            Some(r) => self.repo.resolve(r)?,
            None => self
                .repo
                .head()?
                .ok_or_else(|| Error::UnknownRevision("HEAD".into()))?,
        };
        let _g = self.repo.write_lock.lock().unwrap();
        let mut n = self.load(id)?;
        n.valid_at = Some(at);
        n.updated_ms = now_ms();
        self.save(&n)?;
        self.repo.emit(
            "note.validate",
            Some(author),
            serde_json::json!({ "id": n.id, "valid_at": at }),
        )?;
        drop(_g);
        self.mark_stale(&mut n, self.repo.head()?)?;
        Ok(n)
    }

    /// Turn a draft into a regular note, optionally editing it on the way.
    pub fn accept(&self, id: &str, patch: Option<NotePatch>, author: &str) -> Result<Note> {
        let _g = self.repo.write_lock.lock().unwrap();
        let mut n = self.load(id)?;
        if let Some(p) = patch {
            Self::apply_patch(&mut n, p)?;
        }
        n.draft = false;
        n.updated_ms = now_ms();
        self.save(&n)?;
        self.repo.emit(
            "note.accept",
            Some(author),
            serde_json::json!({ "id": n.id, "title": n.title }),
        )?;
        drop(_g);
        self.mark_stale(&mut n, self.repo.head()?)?;
        Ok(n)
    }

    pub fn delete(&self, id: &str, author: &str) -> Result<()> {
        let _g = self.repo.write_lock.lock().unwrap();
        self.load(id)?;
        fs::remove_file(self.path(id))?;
        if let Some(ix) = self.repo.knowledge.index.write().unwrap().as_mut() {
            ix.remove(id);
        }
        self.repo
            .emit("note.delete", Some(author), serde_json::json!({ "id": id }))?;
        Ok(())
    }

    // -- tags and rules -----------------------------------------------------

    /// Tag counts over accepted (non-draft) notes.
    pub fn tags(&self) -> Result<Vec<TagCount>> {
        let mut m: BTreeMap<String, usize> = BTreeMap::new();
        for n in self.all()?.into_iter().filter(|n| !n.draft) {
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

    pub fn set_rules(&self, rules: Vec<TagRule>, author: &str) -> Result<Vec<TagRule>> {
        for r in &rules {
            for t in &r.tags {
                validate_tag(t)?;
            }
        }
        let _g = self.repo.write_lock.lock().unwrap();
        write_atomic(&self.rules_path(), &serde_json::to_vec_pretty(&rules)?)?;
        self.repo.emit(
            "knowledge.rules",
            Some(author),
            serde_json::json!({ "count": rules.len() }),
        )?;
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

fn new_id() -> String {
    format!("n_{}", &uuid::Uuid::new_v4().simple().to_string()[..12])
}

fn dedup(v: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    v.into_iter().filter(|t| seen.insert(t.clone())).collect()
}

/// Tags are `namespace:value` with lowercase ascii, digits, `-`, `_`, `.`, `/`.
pub fn validate_tag(t: &str) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizer_splits_cases_and_keeps_tags() {
        let t = tokenize("SessionStore rotate_token area:auth HTTPServer x");
        for w in [
            "sessionstore",
            "session",
            "store",
            "rotate",
            "token",
            "area:auth",
            "area",
            "auth",
            "httpserver",
            "http",
            "server",
        ] {
            assert!(t.contains(&w.to_string()), "missing {w} in {t:?}");
        }
        assert!(!t.contains(&"x".to_string()));
    }

    fn note(id: &str, title: &str, body: &str) -> Note {
        Note {
            id: id.into(),
            title: title.into(),
            body: body.into(),
            tags: vec![],
            paths: vec![],
            commits: vec![],
            pulls: vec![],
            author: "t".into(),
            confidence: 1.0,
            created_ms: 0,
            updated_ms: 0,
            superseded_by: None,
            valid_at: None,
            draft: false,
            stale: false,
        }
    }

    #[test]
    fn bm25_ranks_title_hits_first_and_tracks_updates() {
        let mut ix = SearchIndex::build(&[
            note("a", "Token rotation", "sessions expire"),
            note("b", "Sessions", "we rotate the token nightly, token store"),
            note("c", "Unrelated", "nothing here"),
        ]);
        let r = ix.search("token rotation");
        assert_eq!(r[0].0, "a");
        assert_eq!(r.len(), 2);
        ix.remove("a");
        assert_eq!(ix.search("rotation").len(), 0);
        ix.upsert(&note("c", "Rotation", ""));
        assert_eq!(ix.search("rotation")[0].0, "c");
    }
}
