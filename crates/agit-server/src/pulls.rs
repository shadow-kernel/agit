//! Pull requests: file-backed store plus server-side merge.

use std::collections::BTreeMap;
use std::fs;

use agit_core::diff::{diff_trees, Change};
use agit_core::events::now_ms;
use agit_core::merge::{is_ancestor, merge_base, merge_trees, Conflict, MergeLabels};
use agit_core::ops::{log_store, LogOptions};
use agit_core::refs::{write_atomic, Expect};
use agit_core::{AgentInfo, Commit, Error, Result, Signature};
use agit_protocol::types::*;

use crate::state::RepoHandle;

pub struct PullStore<'a> {
    repo: &'a RepoHandle,
}

impl<'a> PullStore<'a> {
    pub fn new(repo: &'a RepoHandle) -> Self {
        PullStore { repo }
    }

    fn dir(&self) -> std::path::PathBuf {
        self.repo.dir.join("pulls")
    }

    fn path(&self, n: u64) -> std::path::PathBuf {
        self.dir().join(format!("{n}.json"))
    }

    pub fn load(&self, n: u64) -> Result<PullRequest> {
        let bytes =
            fs::read(self.path(n)).map_err(|_| Error::NotFound(format!("pull request #{n}")))?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn save(&self, pr: &PullRequest) -> Result<()> {
        write_atomic(&self.path(pr.number), &serde_json::to_vec_pretty(pr)?)
    }

    pub fn list(&self, state: Option<PullState>) -> Result<Vec<PullRequest>> {
        let mut out = Vec::new();
        for e in fs::read_dir(self.dir())?.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "json") {
                if let Ok(pr) = serde_json::from_slice::<PullRequest>(&fs::read(&p)?) {
                    if state.is_none_or(|s| pr.state == s) {
                        out.push(pr);
                    }
                }
            }
        }
        out.sort_by_key(|pr| std::cmp::Reverse(pr.number));
        Ok(out)
    }

    fn next_number(&self) -> Result<u64> {
        let counter = self.dir().join("counter");
        let n: u64 = fs::read_to_string(&counter)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0)
            + 1;
        write_atomic(&counter, n.to_string().as_bytes())?;
        Ok(n)
    }

    pub fn create(&self, req: PullCreate, actor: Actor) -> Result<PullRequest> {
        let _g = self.repo.write_lock.lock().unwrap();
        let source = self.repo.full_ref(&req.source);
        let target = self.repo.full_ref(
            req.target
                .as_deref()
                .unwrap_or(&self.repo.meta.default_branch),
        );
        if source == target {
            return Err(Error::Invalid("source and target are the same ref".into()));
        }
        let source_id = self
            .repo
            .refs
            .read(&source)?
            .ok_or_else(|| Error::RefNotFound(source.clone()))?;
        let target_id = self
            .repo
            .refs
            .read(&target)?
            .ok_or_else(|| Error::RefNotFound(target.clone()))?;
        let base_id = merge_base(&self.repo.store, &source_id, &target_id)?;
        let now = now_ms();
        let pr = PullRequest {
            number: self.next_number()?,
            title: req.title,
            body: req.body.unwrap_or_default(),
            intent: req.intent,
            task: req.task,
            source,
            target,
            source_id,
            target_id,
            base_id,
            author: actor.clone(),
            state: PullState::Open,
            created_ms: now,
            updated_ms: now,
            merged_commit: None,
            reviews: vec![],
            comments: vec![],
            checks: vec![],
        };
        self.save(&pr)?;
        self.repo.emit("pull.open", Some(&actor.id), serde_json::json!({ "number": pr.number, "title": pr.title, "source": pr.source, "target": pr.target }))?;
        Ok(pr)
    }

    /// Refresh source/target ids from the current refs (for open PRs).
    fn refresh(&self, pr: &mut PullRequest) -> Result<()> {
        if pr.state != PullState::Open {
            return Ok(());
        }
        if let Some(s) = self.repo.refs.read(&pr.source)? {
            pr.source_id = s;
        }
        if let Some(t) = self.repo.refs.read(&pr.target)? {
            pr.target_id = t;
        }
        pr.base_id = merge_base(&self.repo.store, &pr.source_id, &pr.target_id)?;
        Ok(())
    }

    pub fn commits(&self, pr: &PullRequest) -> Result<Vec<CommitEntry>> {
        // Commits reachable from the source but not from the merge base. Using
        // the base (not the target) keeps the list stable after the PR merged.
        let stop = pr.base_id.unwrap_or(pr.target_id);
        let have = agit_protocol::walk::commit_closure(&self.repo.store, &[stop], Some(20_000))?;
        let entries = log_store(
            &self.repo.store,
            &[pr.source_id],
            &LogOptions {
                limit: Some(500),
                ..Default::default()
            },
        )?;
        Ok(entries
            .into_iter()
            .filter(|e| !have.contains(&e.id))
            .map(|e| CommitEntry {
                id: e.id,
                commit: e.commit,
            })
            .collect())
    }

    pub fn changes(&self, pr: &PullRequest) -> Result<Vec<Change>> {
        let store = &self.repo.store;
        let base_tree = match pr.base_id {
            Some(b) => Some(store.read_commit(&b)?.tree),
            None => Some(store.read_commit(&pr.target_id)?.tree),
        };
        let src_tree = store.read_commit(&pr.source_id)?.tree;
        diff_trees(store, base_tree.as_ref(), Some(&src_tree))
    }

    fn try_merge_trees(&self, pr: &PullRequest) -> Result<agit_core::merge::MergeOutcome> {
        let store = &self.repo.store;
        let base_tree = match pr.base_id {
            Some(b) => Some(store.read_commit(&b)?.tree),
            None => None,
        };
        let ours = store.read_commit(&pr.target_id)?.tree;
        let theirs = store.read_commit(&pr.source_id)?.tree;
        merge_trees(
            store,
            base_tree.as_ref(),
            &ours,
            &theirs,
            &MergeLabels {
                ours: pr.target.clone(),
                theirs: pr.source.clone(),
            },
        )
    }

    pub fn detail(&self, n: u64) -> Result<PullDetail> {
        let mut pr = self.load(n)?;
        self.refresh(&mut pr)?;
        let commits = self.commits(&pr)?;
        let changes = self.changes(&pr)?;
        let (mergeable, conflicts): (bool, Vec<Conflict>) = if pr.state == PullState::Open {
            if is_ancestor(&self.repo.store, &pr.source_id, &pr.target_id)? {
                (false, vec![])
            } else {
                let o = self.try_merge_trees(&pr)?;
                (o.is_clean(), o.conflicts)
            }
        } else {
            (false, vec![])
        };
        Ok(PullDetail {
            pr,
            commits,
            changes,
            mergeable,
            conflicts,
        })
    }

    pub fn update(&self, n: u64, patch: PullUpdate, actor: &str) -> Result<PullRequest> {
        let _g = self.repo.write_lock.lock().unwrap();
        let mut pr = self.load(n)?;
        if let Some(t) = patch.title {
            pr.title = t;
        }
        if let Some(b) = patch.body {
            pr.body = b;
        }
        if patch.intent.is_some() {
            pr.intent = patch.intent;
        }
        if patch.task.is_some() {
            pr.task = patch.task;
        }
        pr.updated_ms = now_ms();
        self.save(&pr)?;
        self.repo.emit(
            "pull.update",
            Some(actor),
            serde_json::json!({ "number": n }),
        )?;
        Ok(pr)
    }

    pub fn comment(&self, n: u64, req: CommentCreate, actor: &str) -> Result<Comment> {
        let _g = self.repo.write_lock.lock().unwrap();
        let mut pr = self.load(n)?;
        let id = pr.comments.iter().map(|c| c.id).max().unwrap_or(0) + 1;
        let c = Comment {
            id,
            author: actor.to_string(),
            path: req.path,
            line: req.line,
            kind: req.kind,
            body: req.body,
            resolved: false,
            time_ms: now_ms(),
        };
        pr.comments.push(c.clone());
        pr.updated_ms = now_ms();
        self.save(&pr)?;
        self.repo.emit(
            "pull.comment",
            Some(actor),
            serde_json::json!({ "number": n, "comment": id, "path": c.path, "kind": c.kind }),
        )?;
        Ok(c)
    }

    pub fn resolve_comment(&self, n: u64, id: u64, actor: &str) -> Result<PullRequest> {
        let _g = self.repo.write_lock.lock().unwrap();
        let mut pr = self.load(n)?;
        let c = pr
            .comments
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or_else(|| Error::NotFound(format!("comment {id}")))?;
        c.resolved = true;
        pr.updated_ms = now_ms();
        self.save(&pr)?;
        self.repo.emit(
            "pull.comment.resolve",
            Some(actor),
            serde_json::json!({ "number": n, "comment": id }),
        )?;
        Ok(pr)
    }

    pub fn review(&self, n: u64, req: ReviewCreate, actor: &str) -> Result<PullRequest> {
        let _g = self.repo.write_lock.lock().unwrap();
        let mut pr = self.load(n)?;
        if pr.state != PullState::Open {
            return Err(Error::Invalid("pull request is not open".into()));
        }
        pr.reviews.push(Review {
            reviewer: actor.to_string(),
            decision: req.decision,
            body: req.body,
            time_ms: now_ms(),
        });
        pr.updated_ms = now_ms();
        self.save(&pr)?;
        self.repo.emit(
            "pull.review",
            Some(actor),
            serde_json::json!({ "number": n, "decision": req.decision }),
        )?;
        Ok(pr)
    }

    pub fn check(&self, n: u64, req: CheckCreate, actor: &str) -> Result<PullRequest> {
        let _g = self.repo.write_lock.lock().unwrap();
        let mut pr = self.load(n)?;
        pr.checks.retain(|c| c.name != req.name);
        pr.checks.push(Check {
            name: req.name.clone(),
            status: req.status,
            url: req.url,
            time_ms: now_ms(),
        });
        pr.updated_ms = now_ms();
        self.save(&pr)?;
        self.repo.emit(
            "pull.check",
            Some(actor),
            serde_json::json!({ "number": n, "name": req.name, "status": req.status }),
        )?;
        Ok(pr)
    }

    pub fn close(&self, n: u64, actor: &str) -> Result<PullRequest> {
        let _g = self.repo.write_lock.lock().unwrap();
        let mut pr = self.load(n)?;
        if pr.state != PullState::Open {
            return Err(Error::Invalid("pull request is not open".into()));
        }
        pr.state = PullState::Closed;
        pr.updated_ms = now_ms();
        self.save(&pr)?;
        self.repo.emit(
            "pull.close",
            Some(actor),
            serde_json::json!({ "number": n }),
        )?;
        Ok(pr)
    }

    pub fn merge(&self, n: u64, req: MergeRequest, actor: &Actor) -> Result<MergeResponse> {
        let _g = self.repo.write_lock.lock().unwrap();
        let mut pr = self.load(n)?;
        if pr.state != PullState::Open {
            return Err(Error::Invalid("pull request is not open".into()));
        }
        self.refresh(&mut pr)?;
        crate::policy::PolicyStore::new(self.repo).check_merge(&pr, &actor.id)?;
        let store = &self.repo.store;
        if is_ancestor(store, &pr.source_id, &pr.target_id)? {
            return Err(Error::Invalid(
                "source is already contained in target; nothing to merge".into(),
            ));
        }
        let ff_possible = pr.base_id == Some(pr.target_id);
        let merged_commit = match req.strategy {
            MergeStrategy::Ff => {
                if !ff_possible {
                    return Err(Error::Invalid(
                        "not fast-forwardable; use strategy=merge".into(),
                    ));
                }
                pr.source_id
            }
            MergeStrategy::Merge => {
                let outcome = self.try_merge_trees(&pr)?;
                if !outcome.is_clean() {
                    return Err(Error::MergeConflicts(outcome.conflicts.len()));
                }
                let mut meta = BTreeMap::new();
                meta.insert("agit.pull".to_string(), serde_json::json!(pr.number));
                let commit = Commit {
                    tree: outcome.tree,
                    parents: vec![pr.target_id, pr.source_id],
                    author: Signature {
                        name: actor.id.clone(),
                        email: format!("{}@agents", actor.id),
                    },
                    time_ms: now_ms(),
                    message: req.message.clone().unwrap_or_else(|| {
                        format!(
                            "Merge pull request #{} from {}\n\n{}",
                            pr.number, pr.source, pr.title
                        )
                    }),
                    intent: pr.intent.clone(),
                    task: pr.task.clone(),
                    agent: Some(AgentInfo {
                        id: actor.id.clone(),
                        model: actor.model.clone(),
                        run_id: None,
                        on_behalf_of: None,
                    }),
                    checkpoint: false,
                    meta,
                    signature: None,
                };
                store.write_commit(&commit)?
            }
        };
        self.repo
            .refs
            .update(&pr.target, Expect::Is(pr.target_id), Some(merged_commit))?;
        pr.state = PullState::Merged;
        pr.merged_commit = Some(merged_commit);
        pr.target_id = merged_commit;
        pr.updated_ms = now_ms();
        self.save(&pr)?;
        self.repo.emit("ref.update", Some(&actor.id), serde_json::json!({ "ref": pr.target, "new": merged_commit, "op": "merge", "pull": pr.number }))?;
        self.repo.emit("pull.merge", Some(&actor.id), serde_json::json!({ "number": pr.number, "commit": merged_commit, "strategy": req.strategy }))?;
        Ok(MergeResponse { pr, merged_commit })
    }

    /// Open PRs whose change set touches `path`.
    pub fn open_touching(&self, path: &str) -> Result<Vec<PullRequest>> {
        let mut out = Vec::new();
        for mut pr in self.list(Some(PullState::Open))? {
            self.refresh(&mut pr)?;
            let touches = self
                .changes(&pr)?
                .iter()
                .any(|c| c.path == path || c.path.starts_with(&format!("{path}/")));
            if touches {
                out.push(pr);
            }
        }
        Ok(out)
    }

    pub fn open_by_author(&self) -> Result<BTreeMap<String, usize>> {
        let mut m = BTreeMap::new();
        for pr in self.list(Some(PullState::Open))? {
            *m.entry(pr.author.id).or_insert(0) += 1;
        }
        Ok(m)
    }
}
