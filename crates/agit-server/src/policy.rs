//! Repository policies: enforced at ref updates and pull request merges.

use std::collections::HashSet;
use std::fs;

use agit_core::diff::diff_trees;
use agit_core::refs::{write_atomic, HEADS_PREFIX};
use agit_core::{Error, ObjectId, Result};
use agit_protocol::types::{CheckStatus, CommentKind, Policy, PullRequest, ReviewDecision};
use agit_protocol::walk::commit_closure;

use crate::state::RepoHandle;

pub struct PolicyStore<'a> {
    repo: &'a RepoHandle,
}

impl<'a> PolicyStore<'a> {
    pub fn new(repo: &'a RepoHandle) -> Self {
        PolicyStore { repo }
    }

    fn path(&self) -> std::path::PathBuf {
        self.repo.dir.join("policy.json")
    }

    pub fn get(&self) -> Result<Policy> {
        match fs::read(self.path()) {
            Ok(b) => Ok(serde_json::from_slice(&b)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Policy::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn set(&self, p: Policy, actor: &str) -> Result<Policy> {
        let _g = self.repo.write_lock.lock().unwrap();
        write_atomic(&self.path(), &serde_json::to_vec_pretty(&p)?)?;
        self.repo.emit("policy.update", Some(actor), serde_json::to_value(&p)?)?;
        Ok(p)
    }

    pub fn is_protected(policy: &Policy, full_ref: &str) -> bool {
        let short = full_ref.strip_prefix(HEADS_PREFIX).unwrap_or(full_ref);
        policy.protected_branches.iter().any(|p| {
            p == full_ref
                || p == short
                || p.strip_prefix(HEADS_PREFIX).is_some_and(|x| x == short)
                || globset::Glob::new(p.strip_prefix(HEADS_PREFIX).unwrap_or(p))
                    .ok()
                    .is_some_and(|g| g.compile_matcher().is_match(short))
        })
    }

    /// Validate the commits a ref update introduces (old..new) against the
    /// policy's commit rules.
    pub fn check_commits(&self, policy: &Policy, old: Option<ObjectId>, new: ObjectId) -> Result<()> {
        if !policy.require_intent && !policy.require_agent && policy.max_files_per_commit == 0 {
            return Ok(());
        }
        let store = &self.repo.store;
        let have = match old {
            Some(o) => commit_closure(store, &[o], Some(50_000))?,
            None => HashSet::new(),
        };
        let mut stack = vec![new];
        let mut seen = HashSet::new();
        while let Some(id) = stack.pop() {
            if have.contains(&id) || !seen.insert(id) {
                continue;
            }
            let c = store.read_commit(&id)?;
            if policy.require_intent && c.intent.as_deref().map(str::trim).is_none_or(str::is_empty) {
                return Err(Error::PolicyViolation(format!("commit {} has no intent (require_intent)", id.short())));
            }
            if policy.require_agent && c.agent.is_none() {
                return Err(Error::PolicyViolation(format!("commit {} has no agent identity (require_agent)", id.short())));
            }
            if policy.max_files_per_commit > 0 && c.parents.len() <= 1 {
                let parent_tree = match c.parents.first() {
                    Some(p) => Some(store.read_commit(p)?.tree),
                    None => None,
                };
                let n = diff_trees(store, parent_tree.as_ref(), Some(&c.tree))?.len();
                if n > policy.max_files_per_commit as usize {
                    return Err(Error::PolicyViolation(format!(
                        "commit {} touches {n} files, limit is {} (max_files_per_commit)",
                        id.short(),
                        policy.max_files_per_commit
                    )));
                }
            }
            stack.extend(c.parents);
        }
        Ok(())
    }

    /// Direct ref update (push) against a possibly protected branch.
    pub fn check_push(&self, full_ref: &str, old: Option<ObjectId>, new: Option<ObjectId>, actor: &str) -> Result<()> {
        let policy = self.get()?;
        if !Self::is_protected(&policy, full_ref) {
            return Ok(());
        }
        if policy.require_pull_request {
            return Err(Error::PolicyViolation(format!("{full_ref} is protected; changes must go through a pull request")));
        }
        if !policy.allowed_push_agents.is_empty() && !policy.allowed_push_agents.iter().any(|a| a == actor) {
            return Err(Error::PolicyViolation(format!("agent {actor} may not push to {full_ref}")));
        }
        if new.is_none() {
            return Err(Error::PolicyViolation(format!("{full_ref} is protected and cannot be deleted")));
        }
        self.check_commits(&policy, old, new.unwrap())
    }

    /// Pull request merge into a possibly protected target.
    pub fn check_merge(&self, pr: &PullRequest, actor: &str) -> Result<()> {
        let policy = self.get()?;
        if !Self::is_protected(&policy, &pr.target) {
            return Ok(());
        }
        if !policy.allowed_merge_agents.is_empty() && !policy.allowed_merge_agents.iter().any(|a| a == actor) {
            return Err(Error::PolicyViolation(format!("agent {actor} may not merge into {}", pr.target)));
        }
        if policy.require_approvals > 0 {
            // Latest decision per reviewer counts; the author cannot approve their own PR.
            let mut latest: std::collections::BTreeMap<&str, ReviewDecision> = Default::default();
            for r in &pr.reviews {
                latest.insert(r.reviewer.as_str(), r.decision);
            }
            if latest.values().any(|d| *d == ReviewDecision::RequestChanges) {
                return Err(Error::PolicyViolation("a reviewer requested changes".into()));
            }
            let approvals = latest.iter().filter(|(who, d)| **d == ReviewDecision::Approve && **who != pr.author.id).count() as u32;
            if approvals < policy.require_approvals {
                return Err(Error::PolicyViolation(format!("{approvals} approval(s), {} required", policy.require_approvals)));
            }
        }
        if policy.block_on_blockers && pr.comments.iter().any(|c| c.kind == CommentKind::Blocker && !c.resolved) {
            return Err(Error::PolicyViolation("unresolved blocker comments".into()));
        }
        if policy.require_checks {
            if pr.checks.is_empty() {
                return Err(Error::PolicyViolation("no checks reported (require_checks)".into()));
            }
            if let Some(c) = pr.checks.iter().find(|c| c.status != CheckStatus::Success) {
                return Err(Error::PolicyViolation(format!("check {} is {:?}", c.name, c.status)));
            }
        }
        self.check_commits(&policy, Some(pr.target_id), pr.source_id)
    }
}
