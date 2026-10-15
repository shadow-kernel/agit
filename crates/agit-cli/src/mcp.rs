//! Minimal MCP (Model Context Protocol) server over stdio.
//!
//! JSON-RPC 2.0, one message per line. Exposes repository operations as
//! tools so any MCP-capable agent can use agit without shelling out.
//! Register it as `{"command": "agit", "args": ["mcp"]}` in the agent's
//! MCP configuration, started inside a repository (or with `-C <dir>`).

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;

use agit_core::diff;
use agit_core::lease::{LeaseMode, LeaseRequest};
use agit_core::ops::{self, CommitOptions, LogOptions, MergeOptions};
use agit_core::worktree;
use agit_core::{AgentInfo, Repository};
use agit_protocol::sync;
use agit_protocol::types::*;
use anyhow::{anyhow, Result};
use serde_json::{json, Value};

const PROTOCOL_VERSION: &str = "2025-06-18";

struct Tool {
    name: &'static str,
    description: &'static str,
    schema: Value,
}

fn obj(props: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required, "additionalProperties": false })
}

fn tools() -> Vec<Tool> {
    vec![
        Tool { name: "agit_status", description: "Working tree status: staged, unstaged, untracked and conflicted paths, current branch and HEAD.", schema: obj(json!({}), &[]) },
        Tool { name: "agit_log", description: "Commit history. Filter by agent id, task id or path. Returns id, message, intent, task, agent, parents, time.", schema: obj(json!({ "rev": {"type":"string","default":"HEAD"}, "limit": {"type":"integer","default":20}, "agent": {"type":"string"}, "task": {"type":"string"}, "path": {"type":"string"} }), &[]) },
        Tool { name: "agit_show", description: "Show one commit with its metadata, changed files and unified patch.", schema: obj(json!({ "rev": {"type":"string"}, "patch": {"type":"boolean","default":true} }), &["rev"]) },
        Tool { name: "agit_diff", description: "Diff. No arguments: working tree vs index. staged=true: index vs HEAD. from/to: between revisions. Returns changes, stats and patch.", schema: obj(json!({ "staged": {"type":"boolean"}, "from": {"type":"string"}, "to": {"type":"string"}, "context": {"type":"integer","default":3} }), &[]) },
        Tool { name: "agit_read_file", description: "Read a file from the working tree or, with rev, from a commit.", schema: obj(json!({ "path": {"type":"string"}, "rev": {"type":"string"} }), &["path"]) },
        Tool { name: "agit_ls", description: "List a directory of the working tree's HEAD tree (or any rev).", schema: obj(json!({ "path": {"type":"string","default":""}, "rev": {"type":"string","default":"HEAD"} }), &[]) },
        Tool { name: "agit_add", description: "Stage paths (files or directories). all=true stages everything.", schema: obj(json!({ "paths": {"type":"array","items":{"type":"string"}}, "all": {"type":"boolean"} }), &[]) },
        Tool { name: "agit_commit", description: "Commit staged changes with intent, task and agent identity. Refuses to touch paths under another agent's exclusive lease unless force=true.", schema: obj(json!({ "message": {"type":"string"}, "intent": {"type":"string"}, "task": {"type":"string"}, "agent": {"type":"string"}, "model": {"type":"string"}, "run_id": {"type":"string"}, "all": {"type":"boolean","description":"stage tracked modifications first"}, "force": {"type":"boolean"}, "meta": {"type":"object"} }), &["message"]) },
        Tool { name: "agit_checkpoint", description: "Snapshot the whole working tree (tracked and untracked) without moving HEAD. Cheap; use before risky edits.", schema: obj(json!({ "message": {"type":"string"} }), &[]) },
        Tool { name: "agit_checkpoint_list", description: "List checkpoints of this workspace, newest first.", schema: obj(json!({ "limit": {"type":"integer","default":20} }), &[]) },
        Tool { name: "agit_restore", description: "Restore the working tree to a checkpoint or commit (overwrites local changes).", schema: obj(json!({ "rev": {"type":"string"} }), &["rev"]) },
        Tool { name: "agit_branches", description: "List branches with ids; marks the current one.", schema: obj(json!({}), &[]) },
        Tool { name: "agit_switch", description: "Switch to a branch (create=true creates it from HEAD) or detach at a revision.", schema: obj(json!({ "target": {"type":"string"}, "create": {"type":"boolean"}, "force": {"type":"boolean"} }), &["target"]) },
        Tool { name: "agit_merge", description: "Merge a revision into the current branch. Returns fast_forward, merged, already_up_to_date or conflicts with the conflicted paths.", schema: obj(json!({ "rev": {"type":"string"}, "message": {"type":"string"}, "no_ff": {"type":"boolean"}, "agent": {"type":"string"} }), &["rev"]) },
        Tool { name: "agit_lease_claim", description: "Claim exclusive (or shared) paths for an agent for ttl seconds so other agents avoid them. Fails with lease_conflict if someone else holds them.", schema: obj(json!({ "holder": {"type":"string"}, "paths": {"type":"array","items":{"type":"string"}}, "ttl_secs": {"type":"integer","default":3600}, "note": {"type":"string"}, "shared": {"type":"boolean"} }), &["holder","paths"]) },
        Tool { name: "agit_lease_release", description: "Release a lease by id, or all leases of a holder.", schema: obj(json!({ "id": {"type":"string"}, "holder": {"type":"string"} }), &[]) },
        Tool { name: "agit_lease_list", description: "Active leases: who is working where.", schema: obj(json!({}), &[]) },
        Tool { name: "agit_lease_check", description: "Check whether holder may modify the given paths right now.", schema: obj(json!({ "holder": {"type":"string"}, "paths": {"type":"array","items":{"type":"string"}} }), &["holder","paths"]) },
        Tool { name: "agit_events", description: "Repository events (commits, merges, leases, workspaces) after a sequence number.", schema: obj(json!({ "since": {"type":"integer","default":0}, "limit": {"type":"integer","default":100}, "kind": {"type":"string"} }), &[]) },
        Tool { name: "agit_workspaces", description: "List workspaces (separate working directories sharing this repository).", schema: obj(json!({}), &[]) },
        Tool { name: "agit_push", description: "Push a branch to the remote (compare-and-swap; rejects non-fast-forward unless force).", schema: obj(json!({ "remote": {"type":"string"}, "branch": {"type":"string"}, "force": {"type":"boolean"}, "create": {"type":"boolean"} }), &[]) },
        Tool { name: "agit_pull", description: "Fetch the current branch from the remote and merge it.", schema: obj(json!({ "remote": {"type":"string"} }), &[]) },
        Tool { name: "agit_pr_create", description: "Open a pull request on the remote from a branch (default current) into the default branch. push=true pushes first.", schema: obj(json!({ "title": {"type":"string"}, "body": {"type":"string"}, "intent": {"type":"string"}, "task": {"type":"string"}, "source": {"type":"string"}, "target": {"type":"string"}, "push": {"type":"boolean","default":true} }), &["title"]) },
        Tool { name: "agit_pr_list", description: "List pull requests on the remote (state: open, merged, closed).", schema: obj(json!({ "state": {"type":"string","default":"open"} }), &[]) },
        Tool { name: "agit_pr_view", description: "Pull request detail: commits, changed files, reviews, comments, mergeability and conflicts.", schema: obj(json!({ "number": {"type":"integer"} }), &["number"]) },
        Tool { name: "agit_pr_review", description: "Submit a review: approve, request_changes or comment.", schema: obj(json!({ "number": {"type":"integer"}, "decision": {"type":"string","enum":["approve","request_changes","comment"]}, "body": {"type":"string"} }), &["number","decision"]) },
        Tool { name: "agit_pr_comment", description: "Comment on a pull request, optionally anchored to a path and line. kind: question, suggestion, blocker, note.", schema: obj(json!({ "number": {"type":"integer"}, "body": {"type":"string"}, "path": {"type":"string"}, "line": {"type":"integer"}, "kind": {"type":"string"} }), &["number","body"]) },
        Tool { name: "agit_pr_merge", description: "Merge a pull request on the server (strategy merge or ff).", schema: obj(json!({ "number": {"type":"integer"}, "strategy": {"type":"string","enum":["merge","ff"]}, "message": {"type":"string"} }), &["number"]) },
        Tool { name: "agit_context", description: "Everything to read before touching a path: knowledge notes, suggested tags, recent commits, active leases, open pull requests touching it.", schema: obj(json!({ "path": {"type":"string"} }), &["path"]) },
        Tool { name: "agit_know_search", description: "Search knowledge notes by tags (all must match), path coverage or text.", schema: obj(json!({ "tags": {"type":"array","items":{"type":"string"}}, "path": {"type":"string"}, "query": {"type":"string"}, "limit": {"type":"integer","default":20} }), &[]) },
        Tool { name: "agit_know_get", description: "Read one knowledge note in full.", schema: obj(json!({ "id": {"type":"string"} }), &["id"]) },
        Tool { name: "agit_know_add", description: "Record knowledge: a titled markdown note with typed tags (namespace:value), the paths it applies to, related commits and confidence.", schema: obj(json!({ "title": {"type":"string"}, "body": {"type":"string"}, "tags": {"type":"array","items":{"type":"string"}}, "paths": {"type":"array","items":{"type":"string"}}, "commits": {"type":"array","items":{"type":"string"}}, "confidence": {"type":"number"} }), &["title","body"]) },
    ]
}

struct Server {
    cwd: PathBuf,
    remote: String,
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(|x| x.as_str()).map(str::to_string)
}
fn b(v: &Value, k: &str) -> bool {
    v.get(k).and_then(|x| x.as_bool()).unwrap_or(false)
}
fn n(v: &Value, k: &str, d: u64) -> u64 {
    v.get(k).and_then(|x| x.as_u64()).unwrap_or(d)
}
fn list(v: &Value, k: &str) -> Vec<String> {
    v.get(k)
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

impl Server {
    fn repo(&self) -> Result<Repository> {
        Ok(Repository::discover(&self.cwd)?)
    }

    fn agent(&self, args: &Value) -> Option<AgentInfo> {
        let id = s(args, "agent").or_else(|| std::env::var("AGIT_AGENT").ok())?;
        Some(AgentInfo {
            id,
            model: s(args, "model").or_else(|| std::env::var("AGIT_MODEL").ok()),
            run_id: s(args, "run_id").or_else(|| std::env::var("AGIT_RUN_ID").ok()),
            on_behalf_of: None,
        })
    }

    fn rel(&self, repo: &Repository, p: &str) -> Result<String> {
        Ok(repo.rel_path(&self.cwd, std::path::Path::new(p))?)
    }

    fn call(&self, name: &str, args: &Value) -> Result<Value> {
        let repo = self.repo()?;
        let v = match name {
            "agit_status" => serde_json::to_value(worktree::status(&repo, true)?)?,
            "agit_log" => {
                let rev = s(args, "rev").unwrap_or_else(|| "HEAD".into());
                let start = match repo.resolve(&rev) {
                    Ok(id) => vec![id],
                    Err(_) if rev == "HEAD" => vec![],
                    Err(e) => return Err(e.into()),
                };
                let path = match s(args, "path") {
                    Some(p) => Some(self.rel(&repo, &p)?),
                    None => None,
                };
                serde_json::to_value(ops::log(
                    &repo,
                    &start,
                    &LogOptions {
                        limit: Some(n(args, "limit", 20) as usize),
                        agent: s(args, "agent"),
                        task: s(args, "task"),
                        path,
                        first_parent: false,
                    },
                )?)?
            }
            "agit_show" => {
                let id = repo.resolve(&s(args, "rev").unwrap_or_else(|| "HEAD".into()))?;
                let c = repo.store().read_commit(&id)?;
                let parent_tree = match c.parents.first() {
                    Some(p) => Some(repo.store().read_commit(p)?.tree),
                    None => None,
                };
                let changes = diff::diff_trees(repo.store(), parent_tree.as_ref(), Some(&c.tree))?;
                let patch = if args.get("patch").and_then(|x| x.as_bool()).unwrap_or(true) {
                    Some(diff::render_patch(repo.store(), &changes, 3)?)
                } else {
                    None
                };
                json!({ "id": id, "commit": c, "changes": changes, "patch": patch })
            }
            "agit_diff" => {
                let ctx = n(args, "context", 3) as usize;
                let (old, new) = match (s(args, "from"), s(args, "to")) {
                    (Some(f), Some(t)) => (
                        diff::tree_to_sides(repo.store(), Some(&repo.resolve_commit(&f)?.1.tree))?,
                        diff::tree_to_sides(repo.store(), Some(&repo.resolve_commit(&t)?.1.tree))?,
                    ),
                    (Some(f), None) => (
                        diff::tree_to_sides(repo.store(), Some(&repo.resolve_commit(&f)?.1.tree))?,
                        worktree_sides(&repo)?,
                    ),
                    _ if b(args, "staged") => (
                        diff::tree_to_sides(repo.store(), repo.head_tree()?.as_ref())?,
                        index_sides(&repo)?,
                    ),
                    _ => (index_sides(&repo)?, worktree_sides(&repo)?),
                };
                let changes = diff::diff_maps(&old, &new);
                let from_worktree = s(args, "to").is_none() && !b(args, "staged");
                let patch = render(&repo, &changes, ctx, from_worktree)?;
                let stats: Vec<Value> = changes
                    .iter()
                    .map(|c| {
                        let (a, d) = stats(&repo, c, from_worktree).unwrap_or((0, 0));
                        json!({ "path": c.path, "added": a, "removed": d })
                    })
                    .collect();
                json!({ "changes": changes, "stats": stats, "patch": patch })
            }
            "agit_read_file" => {
                let path = s(args, "path").ok_or_else(|| anyhow!("path required"))?;
                let rel = self.rel(&repo, &path)?;
                match s(args, "rev") {
                    None => {
                        let bytes = std::fs::read(repo.abs_path(&rel))?;
                        json!({ "path": rel, "size": bytes.len(), "binary": diff::is_binary(&bytes), "content": if diff::is_binary(&bytes) { None } else { Some(String::from_utf8_lossy(&bytes).to_string()) } })
                    }
                    Some(rev) => {
                        let tree = repo.resolve_commit(&rev)?.1.tree;
                        let flat = agit_core::index::flatten_tree(repo.store(), &tree)?;
                        let e = flat
                            .get(&rel)
                            .ok_or_else(|| anyhow!("{rel} not found in {rev}"))?;
                        let bytes = repo.store().read_blob(&e.id)?;
                        json!({ "path": rel, "rev": rev, "id": e.id, "size": bytes.len(), "binary": diff::is_binary(&bytes), "content": if diff::is_binary(&bytes) { None } else { Some(String::from_utf8_lossy(&bytes).to_string()) } })
                    }
                }
            }
            "agit_ls" => {
                let rev = s(args, "rev").unwrap_or_else(|| "HEAD".into());
                let path = s(args, "path").unwrap_or_default();
                let path = path.trim_matches('/').to_string();
                let mut cur = repo.resolve_commit(&rev)?.1.tree;
                if !path.is_empty() {
                    for part in path.split('/') {
                        let t = repo.store().read_tree(&cur)?;
                        let e = t.get(part).ok_or_else(|| anyhow!("{path} not found"))?;
                        if !e.mode.is_dir() {
                            return Err(anyhow!("{path} is a file"));
                        }
                        cur = e.id;
                    }
                }
                let t = repo.store().read_tree(&cur)?;
                json!({ "path": path, "entries": t.entries() })
            }
            "agit_add" => {
                let paths = if b(args, "all") {
                    vec![]
                } else {
                    list(args, "paths")
                        .iter()
                        .map(|p| self.rel(&repo, p))
                        .collect::<Result<Vec<_>>>()?
                };
                if paths.is_empty() && !b(args, "all") {
                    return Err(anyhow!("give paths or all=true"));
                }
                serde_json::to_value(worktree::add(&repo, &paths, false)?)?
            }
            "agit_commit" => {
                let meta: BTreeMap<String, Value> = args
                    .get("meta")
                    .and_then(|m| m.as_object())
                    .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                    .unwrap_or_default();
                serde_json::to_value(ops::commit(
                    &repo,
                    CommitOptions {
                        message: s(args, "message").unwrap_or_default(),
                        intent: s(args, "intent"),
                        task: s(args, "task"),
                        agent: self.agent(args),
                        meta,
                        author: None,
                        allow_empty: false,
                        force: b(args, "force"),
                        all: b(args, "all"),
                    },
                )?)?
            }
            "agit_checkpoint" => serde_json::to_value(ops::checkpoint(
                &repo,
                s(args, "message"),
                self.agent(args),
            )?)?,
            "agit_checkpoint_list" => serde_json::to_value(
                ops::list_checkpoints(&repo, n(args, "limit", 20) as usize)?
                    .into_iter()
                    .map(|(id, commit)| ops::LogEntry { id, commit })
                    .collect::<Vec<_>>(),
            )?,
            "agit_restore" => serde_json::to_value(ops::restore(
                &repo,
                &s(args, "rev").ok_or_else(|| anyhow!("rev required"))?,
            )?)?,
            "agit_branches" => serde_json::to_value(ops::list_branches(&repo)?)?,
            "agit_switch" => serde_json::to_value(ops::switch(
                &repo,
                &s(args, "target").ok_or_else(|| anyhow!("target required"))?,
                b(args, "create"),
                b(args, "force"),
            )?)?,
            "agit_merge" => serde_json::to_value(ops::merge(
                &repo,
                &s(args, "rev").ok_or_else(|| anyhow!("rev required"))?,
                MergeOptions {
                    message: s(args, "message"),
                    intent: None,
                    agent: self.agent(args),
                    no_ff: b(args, "no_ff"),
                    force: false,
                },
            )?)?,
            "agit_lease_claim" => {
                let paths: Vec<String> = list(args, "paths")
                    .iter()
                    .map(|p| {
                        if p.contains(['*', '?', '[']) {
                            Ok(p.clone())
                        } else {
                            self.rel(&repo, p)
                        }
                    })
                    .collect::<Result<_>>()?;
                let holder = s(args, "holder").ok_or_else(|| anyhow!("holder required"))?;
                let lease = repo.leases().claim(LeaseRequest {
                    holder: holder.clone(),
                    paths,
                    mode: if b(args, "shared") {
                        LeaseMode::Shared
                    } else {
                        LeaseMode::Exclusive
                    },
                    ttl: Some(std::time::Duration::from_secs(n(args, "ttl_secs", 3600))),
                    note: s(args, "note"),
                    workspace: Some(repo.workspace().to_string()),
                })?;
                repo.events().append(
                    "lease.claim",
                    Some(repo.workspace()),
                    Some(&holder),
                    serde_json::to_value(&lease)?,
                )?;
                serde_json::to_value(lease)?
            }
            "agit_lease_release" => {
                let released = match (s(args, "id"), s(args, "holder")) {
                    (Some(id), _) => vec![repo.leases().release(&id)?],
                    (None, Some(h)) => repo.leases().release_all(&h)?,
                    _ => return Err(anyhow!("id or holder required")),
                };
                for l in &released {
                    repo.events().append(
                        "lease.release",
                        Some(repo.workspace()),
                        Some(&l.holder),
                        json!({ "id": l.id, "paths": l.paths }),
                    )?;
                }
                serde_json::to_value(released)?
            }
            "agit_lease_list" => serde_json::to_value(repo.leases().list()?)?,
            "agit_lease_check" => {
                let paths: Vec<String> = list(args, "paths")
                    .iter()
                    .map(|p| self.rel(&repo, p))
                    .collect::<Result<_>>()?;
                ops::enforce_leases(
                    repo.leases(),
                    &s(args, "holder").unwrap_or_default(),
                    &paths,
                )?;
                json!({ "allowed": true })
            }
            "agit_events" => {
                let kind = s(args, "kind");
                let mut ev = repo.events().read_after(n(args, "since", 0))?;
                if let Some(k) = kind {
                    ev.retain(|e| e.kind.starts_with(&k));
                }
                let limit = n(args, "limit", 100) as usize;
                if ev.len() > limit {
                    ev = ev.split_off(ev.len() - limit);
                }
                serde_json::to_value(ev)?
            }
            "agit_workspaces" => serde_json::to_value(repo.list_workspaces()?)?,
            "agit_push" => {
                let remote = s(args, "remote").unwrap_or_else(|| self.remote.clone());
                let branch = match s(args, "branch") {
                    Some(b) => b,
                    None => repo
                        .current_branch()?
                        .ok_or_else(|| anyhow!("not on a branch"))?,
                };
                serde_json::to_value(sync::push(
                    &repo,
                    &remote,
                    &branch,
                    b(args, "force"),
                    b(args, "create"),
                )?)?
            }
            "agit_pull" => {
                let remote = s(args, "remote").unwrap_or_else(|| self.remote.clone());
                let (f, m) = sync::pull(
                    &repo,
                    &remote,
                    MergeOptions {
                        agent: self.agent(args),
                        ..Default::default()
                    },
                )?;
                json!({ "fetch": f, "merge": m })
            }
            "agit_pr_create" => {
                let (c, name) = self.client(&repo, args)?;
                let source = match s(args, "source") {
                    Some(x) => x,
                    None => repo
                        .current_branch()?
                        .ok_or_else(|| anyhow!("not on a branch"))?,
                };
                if args.get("push").and_then(|x| x.as_bool()).unwrap_or(true) {
                    sync::push(&repo, &self.remote, &source, false, false)?;
                }
                serde_json::to_value(c.create_pull(
                    &name,
                    &PullCreate {
                        title: s(args, "title").unwrap_or_default(),
                        body: s(args, "body"),
                        intent: s(args, "intent"),
                        task: s(args, "task"),
                        source,
                        target: s(args, "target"),
                    },
                )?)?
            }
            "agit_pr_list" => {
                let (c, name) = self.client(&repo, args)?;
                serde_json::to_value(c.pulls(
                    &name,
                    Some(&s(args, "state").unwrap_or_else(|| "open".into())),
                )?)?
            }
            "agit_pr_view" => {
                let (c, name) = self.client(&repo, args)?;
                serde_json::to_value(c.pull(&name, n(args, "number", 0))?)?
            }
            "agit_pr_review" => {
                let (c, name) = self.client(&repo, args)?;
                let decision = match s(args, "decision").as_deref() {
                    Some("approve") => ReviewDecision::Approve,
                    Some("request_changes") => ReviewDecision::RequestChanges,
                    _ => ReviewDecision::Comment,
                };
                serde_json::to_value(c.review_pull(
                    &name,
                    n(args, "number", 0),
                    &ReviewCreate {
                        decision,
                        body: s(args, "body").unwrap_or_default(),
                    },
                )?)?
            }
            "agit_pr_comment" => {
                let (c, name) = self.client(&repo, args)?;
                let kind = match s(args, "kind").as_deref() {
                    Some("question") => CommentKind::Question,
                    Some("suggestion") => CommentKind::Suggestion,
                    Some("blocker") => CommentKind::Blocker,
                    _ => CommentKind::Note,
                };
                serde_json::to_value(c.comment_pull(
                    &name,
                    n(args, "number", 0),
                    &CommentCreate {
                        path: s(args, "path"),
                        line: args.get("line").and_then(|x| x.as_u64()).map(|x| x as u32),
                        kind,
                        body: s(args, "body").unwrap_or_default(),
                    },
                )?)?
            }
            "agit_pr_merge" => {
                let (c, name) = self.client(&repo, args)?;
                let strategy = if s(args, "strategy").as_deref() == Some("ff") {
                    MergeStrategy::Ff
                } else {
                    MergeStrategy::Merge
                };
                serde_json::to_value(c.merge_pull(
                    &name,
                    n(args, "number", 0),
                    &MergeRequest {
                        strategy,
                        message: s(args, "message"),
                    },
                )?)?
            }
            "agit_context" => {
                let (c, name) = self.client(&repo, args)?;
                let p = self.rel(&repo, &s(args, "path").unwrap_or_default())?;
                serde_json::to_value(c.context(&name, &p)?)?
            }
            "agit_know_search" => {
                let (c, name) = self.client(&repo, args)?;
                let mut q = format!("?limit={}", n(args, "limit", 20));
                let tags = list(args, "tags");
                if !tags.is_empty() {
                    q.push_str(&format!("&tags={}", tags.join(",")));
                }
                if let Some(p) = s(args, "path") {
                    q.push_str(&format!("&path={}", agit_protocol::client::urlencode(&p)));
                }
                if let Some(t) = s(args, "query") {
                    q.push_str(&format!("&q={}", agit_protocol::client::urlencode(&t)));
                }
                serde_json::to_value(c.notes(&name, &q)?)?
            }
            "agit_know_get" => {
                let (c, name) = self.client(&repo, args)?;
                serde_json::to_value(c.note(&name, &s(args, "id").unwrap_or_default())?)?
            }
            "agit_know_add" => {
                let (c, name) = self.client(&repo, args)?;
                let commits = list(args, "commits")
                    .iter()
                    .map(|r| Ok(repo.resolve(r)?))
                    .collect::<Result<Vec<_>>>()?;
                serde_json::to_value(
                    c.create_note(
                        &name,
                        &NoteCreate {
                            title: s(args, "title").unwrap_or_default(),
                            body: s(args, "body").unwrap_or_default(),
                            tags: list(args, "tags"),
                            paths: list(args, "paths"),
                            commits,
                            pulls: vec![],
                            confidence: args
                                .get("confidence")
                                .and_then(|x| x.as_f64())
                                .map(|x| x as f32),
                        },
                    )?,
                )?
            }
            other => return Err(anyhow!("unknown tool {other}")),
        };
        Ok(v)
    }

    fn client(&self, repo: &Repository, args: &Value) -> Result<(agit_protocol::Client, String)> {
        let (c, name) = sync::client_for(repo, &self.remote)?;
        Ok((c.with_agent(s(args, "agent")), name))
    }
}

fn index_sides(repo: &Repository) -> Result<BTreeMap<String, diff::Side>> {
    Ok(repo
        .load_index()?
        .iter()
        .map(|e| {
            (
                e.path.clone(),
                diff::Side {
                    id: e.id,
                    mode: e.mode,
                    size: e.size,
                },
            )
        })
        .collect())
}

fn worktree_sides(repo: &Repository) -> Result<BTreeMap<String, diff::Side>> {
    let mut sides = index_sides(repo)?;
    for c in worktree::status(repo, true)?.unstaged {
        match c.kind {
            diff::ChangeKind::Deleted => {
                sides.remove(&c.path);
            }
            _ => {
                sides.insert(c.path.clone(), c.new.unwrap());
            }
        }
    }
    Ok(sides)
}

fn read_side(
    repo: &Repository,
    path: &str,
    side: &diff::Side,
    from_worktree: bool,
) -> Result<Vec<u8>> {
    if from_worktree && !repo.store().contains(&side.id) {
        return Ok(std::fs::read(repo.abs_path(path))?);
    }
    Ok(repo.store().read_blob(&side.id)?)
}

fn render(
    repo: &Repository,
    changes: &[diff::Change],
    ctx: usize,
    from_worktree: bool,
) -> Result<String> {
    let mut out = String::new();
    for c in changes {
        let old = c
            .old
            .as_ref()
            .and_then(|s| repo.store().read_blob(&s.id).ok())
            .unwrap_or_default();
        let new = match &c.new {
            Some(s) => read_side(repo, &c.path, s, from_worktree)?,
            None => vec![],
        };
        match diff::unified(&c.path, &old, &new, ctx) {
            Some(p) => out.push_str(&p),
            None => out.push_str(&format!("Binary files a/{0} and b/{0} differ\n", c.path)),
        }
    }
    Ok(out)
}

fn stats(repo: &Repository, c: &diff::Change, from_worktree: bool) -> Result<(usize, usize)> {
    let old = c
        .old
        .as_ref()
        .and_then(|s| repo.store().read_blob(&s.id).ok())
        .unwrap_or_default();
    let new = match &c.new {
        Some(s) => read_side(repo, &c.path, s, from_worktree)?,
        None => vec![],
    };
    Ok(diff::line_stats(&old, &new))
}

fn error_payload(e: &anyhow::Error) -> Value {
    match e.downcast_ref::<agit_core::Error>() {
        Some(core) => {
            json!({ "ok": false, "error": { "code": core.code(), "message": core.to_string(), "retryable": core.is_retryable() } })
        }
        None => {
            json!({ "ok": false, "error": { "code": "error", "message": format!("{e:#}"), "retryable": false } })
        }
    }
}

/// Handle one JSON-RPC request; returns None for notifications.
fn handle(server: &Server, msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let result: std::result::Result<Value, (i64, String)> = match method {
        "initialize" => Ok(json!({
            "protocolVersion": params.get("protocolVersion").and_then(|v| v.as_str()).unwrap_or(PROTOCOL_VERSION),
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": "agit", "version": env!("CARGO_PKG_VERSION") },
            "instructions": "agit version control for agents. Typical flow: agit_context(path) before editing, agit_lease_claim for the paths you will change, agit_checkpoint before risky edits, agit_add + agit_commit with intent/task/agent, agit_pr_create when done, agit_know_add to record what you learned."
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(
            json!({ "tools": tools().iter().map(|t| json!({ "name": t.name, "description": t.description, "inputSchema": t.schema })).collect::<Vec<_>>() }),
        ),
        "tools/call" => {
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            match server.call(name, &args) {
                Ok(v) => Ok(
                    json!({ "content": [{ "type": "text", "text": serde_json::to_string(&v).unwrap_or_default() }], "structuredContent": v, "isError": false }),
                ),
                Err(e) => {
                    let p = error_payload(&e);
                    Ok(
                        json!({ "content": [{ "type": "text", "text": serde_json::to_string(&p).unwrap_or_default() }], "structuredContent": p, "isError": true }),
                    )
                }
            }
        }
        m if m.starts_with("notifications/") => return None,
        "resources/list" => Ok(json!({ "resources": [] })),
        "prompts/list" => Ok(json!({ "prompts": [] })),
        _ => Err((-32601, format!("method not found: {method}"))),
    };
    id.as_ref()?;
    Some(match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err((code, message)) => {
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
        }
    })
}

pub fn serve(cwd: PathBuf, remote: String) -> Result<()> {
    let server = Server { cwd, remote };
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let err = json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": format!("parse error: {e}") } });
                let mut out = stdout.lock();
                writeln!(out, "{err}")?;
                out.flush()?;
                continue;
            }
        };
        if let Some(resp) = handle(&server, &msg) {
            let mut out = stdout.lock();
            writeln!(out, "{resp}")?;
            out.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_list_and_call() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(&dir.path().join("r")).unwrap();
        std::fs::write(repo.work_dir().join("a.txt"), "hi\n").unwrap();
        let server = Server {
            cwd: repo.work_dir().to_path_buf(),
            remote: "origin".into(),
        };
        let init = handle(&server, &json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18" } })).unwrap();
        assert_eq!(init["result"]["serverInfo"]["name"], "agit");
        assert!(handle(
            &server,
            &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
        )
        .is_none());
        let list = handle(
            &server,
            &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        )
        .unwrap();
        assert!(list["result"]["tools"].as_array().unwrap().len() > 25);
        let st = handle(&server, &json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "agit_status", "arguments": {} } })).unwrap();
        assert_eq!(st["result"]["isError"], false);
        assert_eq!(st["result"]["structuredContent"]["untracked"][0], "a.txt");
        handle(&server, &json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": { "name": "agit_add", "arguments": { "all": true } } })).unwrap();
        let c = handle(&server, &json!({ "jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": { "name": "agit_commit", "arguments": { "message": "via mcp", "intent": "test", "agent": "mcp-agent" } } })).unwrap();
        assert_eq!(c["result"]["isError"], false);
        let log = handle(&server, &json!({ "jsonrpc": "2.0", "id": 6, "method": "tools/call", "params": { "name": "agit_log", "arguments": { "agent": "mcp-agent" } } })).unwrap();
        assert_eq!(log["result"]["structuredContent"][0]["message"], "via mcp");
        let err = handle(&server, &json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": { "name": "agit_show", "arguments": { "rev": "nope" } } })).unwrap();
        assert_eq!(err["result"]["isError"], true);
        assert_eq!(
            err["result"]["structuredContent"]["error"]["code"],
            "unknown_revision"
        );
        let unknown = handle(
            &server,
            &json!({ "jsonrpc": "2.0", "id": 8, "method": "nope" }),
        )
        .unwrap();
        assert_eq!(unknown["error"]["code"], -32601);
    }
}
