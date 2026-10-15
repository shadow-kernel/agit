//! Remote-facing commands: remote, push, fetch, pull, clone, pr, know.

use std::path::{Path, PathBuf};

use agit_core::ops::{MergeOptions, MergeResult};
use agit_core::{AgentInfo, Repository};
use agit_protocol::sync;
use agit_protocol::types::*;
use anyhow::{bail, Result};
use clap::Subcommand;

use crate::output::{emit, fmt_time};

#[derive(Subcommand)]
pub enum RemoteCmd {
    /// Register a remote: `agit remote add origin http://host:7878/api/v1/repos/<name>`.
    Add {
        name: String,
        url: String,
    },
    List,
    Remove {
        name: String,
    },
}

#[derive(Subcommand)]
pub enum PrCmd {
    /// Open a pull request from a branch (default: current) into the default branch.
    Create {
        #[arg(short, long)]
        title: String,
        #[arg(short, long)]
        body: Option<String>,
        #[arg(long)]
        intent: Option<String>,
        #[arg(long)]
        task: Option<String>,
        #[arg(long)]
        source: Option<String>,
        #[arg(long)]
        target: Option<String>,
        /// Push the source branch first.
        #[arg(long)]
        push: bool,
    },
    /// List pull requests (open by default).
    List {
        #[arg(long, default_value = "open")]
        state: String,
    },
    /// Show a pull request with commits, changes and mergeability.
    View {
        number: u64,
    },
    /// Submit a review: approve, request_changes or comment.
    Review {
        number: u64,
        #[arg(long, default_value = "approve")]
        decision: String,
        #[arg(short, long, default_value = "")]
        body: String,
    },
    /// Add a comment, optionally anchored to a path and line.
    Comment {
        number: u64,
        #[arg(short, long)]
        body: String,
        #[arg(long)]
        path: Option<String>,
        #[arg(long)]
        line: Option<u32>,
        #[arg(long, default_value = "note")]
        kind: String,
    },
    /// Merge a pull request on the server.
    Merge {
        number: u64,
        #[arg(long, default_value = "merge")]
        strategy: String,
        #[arg(short, long)]
        message: Option<String>,
    },
    Close {
        number: u64,
    },
}

#[derive(Subcommand)]
pub enum KnowCmd {
    /// Create a note. Tags are `namespace:value`, paths may be globs.
    Add {
        #[arg(short, long)]
        title: String,
        #[arg(short, long, default_value = "")]
        body: String,
        /// Read the body from a file (`-` for stdin).
        #[arg(long)]
        body_file: Option<PathBuf>,
        #[arg(long = "tag")]
        tags: Vec<String>,
        #[arg(long = "path")]
        paths: Vec<String>,
        #[arg(long = "commit")]
        commits: Vec<String>,
        #[arg(long)]
        confidence: Option<f32>,
    },
    /// Search notes by tags, path or text.
    Search {
        #[arg(long = "tag")]
        tags: Vec<String>,
        #[arg(long)]
        path: Option<String>,
        #[arg(short, long)]
        query: Option<String>,
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
    },
    /// Show one note.
    Get { id: String },
    /// Everything an agent should read before touching a path.
    Context { path: String },
    /// List tags with counts.
    Tags,
}

fn client_as(
    repo: &Repository,
    remote: &str,
    agent: Option<&str>,
) -> Result<(agit_protocol::Client, String)> {
    let (c, name) = sync::client_for(repo, remote)?;
    Ok((c.with_agent(agent.map(str::to_string)), name))
}

pub fn run_remote(repo: &Repository, json: bool, cmd: RemoteCmd) -> Result<()> {
    match cmd {
        RemoteCmd::Add { name, url } => {
            sync::add_remote(repo, &name, &url)?;
            emit(
                json,
                &serde_json::json!({ "name": name, "url": url }),
                || println!("added remote {name}"),
            );
        }
        RemoteCmd::List => {
            let cfg = repo.config()?;
            let list: Vec<serde_json::Value> = cfg
                .remotes
                .iter()
                .map(|(n, r)| serde_json::json!({ "name": n, "url": r.url }))
                .collect();
            emit(json, &list, || {
                for (n, r) in &cfg.remotes {
                    println!("{n}\t{}", r.url);
                }
            });
        }
        RemoteCmd::Remove { name } => {
            sync::remove_remote(repo, &name)?;
            emit(json, &serde_json::json!({ "removed": name }), || {
                println!("removed remote {name}")
            });
        }
    }
    Ok(())
}

pub fn run_push(
    repo: &Repository,
    json: bool,
    remote: &str,
    branch: Option<String>,
    force: bool,
    create: bool,
) -> Result<()> {
    let branch = match branch {
        Some(b) => b,
        None => repo
            .current_branch()?
            .ok_or_else(|| anyhow::anyhow!("not on a branch; specify one to push"))?,
    };
    let r = sync::push(repo, remote, &branch, force, create)?;
    emit(json, &r, || {
        if r.up_to_date {
            println!("Everything up to date");
        } else {
            println!(
                "To {remote}: {} -> {} ({} object(s) sent)",
                r.old
                    .map(|o| o.short())
                    .unwrap_or_else(|| "[new branch]".into()),
                r.new.short(),
                r.objects_sent
            );
        }
    });
    Ok(())
}

pub fn run_fetch(repo: &Repository, json: bool, remote: &str, branches: &[String]) -> Result<()> {
    let r = sync::fetch(
        repo,
        remote,
        if branches.is_empty() {
            None
        } else {
            Some(branches)
        },
    )?;
    emit(json, &r, || {
        for f in &r.refs {
            println!(
                "{} {} -> {}",
                f.name,
                f.old.map(|o| o.short()).unwrap_or_else(|| "[new]".into()),
                f.new.short()
            );
        }
        println!("{} object(s) received", r.objects_received);
    });
    Ok(())
}

pub fn run_pull(
    repo: &Repository,
    json: bool,
    remote: &str,
    no_ff: bool,
    agent: Option<AgentInfo>,
) -> Result<()> {
    let (f, m) = sync::pull(
        repo,
        remote,
        MergeOptions {
            no_ff,
            agent,
            ..Default::default()
        },
    )?;
    let conflicted = matches!(m, MergeResult::Conflicts { .. });
    emit(
        json,
        &serde_json::json!({ "fetch": f, "merge": m }),
        || match &m {
            MergeResult::AlreadyUpToDate { .. } => println!("Already up to date."),
            MergeResult::FastForward {
                to, files_changed, ..
            } => println!("Fast-forward to {} ({files_changed} file(s))", to.short()),
            MergeResult::Merged { commit, .. } => println!("Merge made commit {}", commit.short()),
            MergeResult::Conflicts { conflicts, .. } => {
                for c in conflicts {
                    println!("CONFLICT ({:?}): {}", c.kind, c.path);
                }
            }
        },
    );
    if conflicted {
        if let MergeResult::Conflicts { conflicts, .. } = m {
            return Err(agit_core::Error::MergeConflicts(conflicts.len()).into());
        }
    }
    Ok(())
}

pub fn run_clone(json: bool, cwd: &Path, url: &str, dir: Option<PathBuf>) -> Result<()> {
    let name = agit_protocol::client::RepoUrl::parse(url)?.repo;
    let dir = dir.unwrap_or_else(|| PathBuf::from(&name));
    let dir = if dir.is_absolute() {
        dir
    } else {
        cwd.join(dir)
    };
    if dir.exists() && std::fs::read_dir(&dir)?.next().is_some() {
        bail!("destination {} exists and is not empty", dir.display());
    }
    let (repo, f) = sync::clone(url, &dir)?;
    emit(
        json,
        &serde_json::json!({ "path": repo.work_dir(), "fetch": f }),
        || {
            println!(
                "Cloned {name} into {} ({} object(s))",
                repo.work_dir().display(),
                f.objects_received
            );
        },
    );
    Ok(())
}

pub fn run_pr(
    repo: &Repository,
    json: bool,
    remote: &str,
    cmd: PrCmd,
    agent: Option<&str>,
) -> Result<()> {
    let (c, name) = client_as(repo, remote, agent)?;
    match cmd {
        PrCmd::Create {
            title,
            body,
            intent,
            task,
            source,
            target,
            push,
        } => {
            let source = match source {
                Some(s) => s,
                None => repo
                    .current_branch()?
                    .ok_or_else(|| anyhow::anyhow!("not on a branch; use --source"))?,
            };
            if push {
                sync::push(repo, remote, &source, false, false)?;
            }
            let pr = c.create_pull(
                &name,
                &PullCreate {
                    title,
                    body,
                    intent,
                    task,
                    source,
                    target,
                },
            )?;
            emit(json, &pr, || {
                println!("Opened pull request #{}: {}", pr.number, pr.title)
            });
        }
        PrCmd::List { state } => {
            let list = c.pulls(&name, Some(&state))?;
            emit(json, &list, || {
                for p in &list {
                    println!(
                        "#{:<4} {:<8} {:<40} {} -> {} by {}",
                        p.number,
                        format!("{:?}", p.state).to_lowercase(),
                        p.title,
                        p.source.trim_start_matches("refs/heads/"),
                        p.target.trim_start_matches("refs/heads/"),
                        p.author.id
                    );
                }
            });
        }
        PrCmd::View { number } => {
            let d = c.pull(&name, number)?;
            emit(json, &d, || {
                let p = &d.pr;
                println!("#{} {} [{:?}]", p.number, p.title, p.state);
                println!(
                    "{} -> {}  by {}  {}",
                    p.source,
                    p.target,
                    p.author.id,
                    fmt_time(p.created_ms)
                );
                if let Some(i) = &p.intent {
                    println!("Intent: {i}");
                }
                if let Some(t) = &p.task {
                    println!("Task:   {t}");
                }
                println!(
                    "mergeable: {}  conflicts: {}",
                    d.mergeable,
                    d.conflicts.len()
                );
                if !p.body.is_empty() {
                    println!("\n{}\n", p.body);
                }
                println!("commits ({}):", d.commits.len());
                for cm in &d.commits {
                    println!("  {} {}", cm.id.short(), cm.commit.summary());
                }
                println!("files ({}):", d.changes.len());
                for ch in &d.changes {
                    println!("  {} {}", ch.kind.letter(), ch.path);
                }
                if !p.reviews.is_empty() {
                    println!("reviews:");
                    for r in &p.reviews {
                        println!("  {} {:?} {}", r.reviewer, r.decision, r.body);
                    }
                }
                for cm in &p.comments {
                    println!(
                        "comment #{} {} {}{}: {}",
                        cm.id,
                        cm.author,
                        cm.path.clone().unwrap_or_default(),
                        cm.line.map(|l| format!(":{l}")).unwrap_or_default(),
                        cm.body
                    );
                }
            });
        }
        PrCmd::Review {
            number,
            decision,
            body,
        } => {
            let decision = match decision.as_str() {
                "approve" => ReviewDecision::Approve,
                "request_changes" | "request-changes" => ReviewDecision::RequestChanges,
                "comment" => ReviewDecision::Comment,
                other => bail!("unknown decision {other}; use approve|request_changes|comment"),
            };
            let pr = c.review_pull(&name, number, &ReviewCreate { decision, body })?;
            emit(json, &pr, || {
                println!("Reviewed #{} ({:?})", pr.number, decision)
            });
        }
        PrCmd::Comment {
            number,
            body,
            path,
            line,
            kind,
        } => {
            let kind = match kind.as_str() {
                "question" => CommentKind::Question,
                "suggestion" => CommentKind::Suggestion,
                "blocker" => CommentKind::Blocker,
                _ => CommentKind::Note,
            };
            let cm = c.comment_pull(
                &name,
                number,
                &CommentCreate {
                    path,
                    line,
                    kind,
                    body,
                },
            )?;
            emit(json, &cm, || {
                println!("Added comment #{} to #{number}", cm.id)
            });
        }
        PrCmd::Merge {
            number,
            strategy,
            message,
        } => {
            let strategy = match strategy.as_str() {
                "ff" => MergeStrategy::Ff,
                _ => MergeStrategy::Merge,
            };
            let r = c.merge_pull(&name, number, &MergeRequest { strategy, message })?;
            emit(json, &r, || {
                println!("Merged #{} as {}", r.pr.number, r.merged_commit.short())
            });
        }
        PrCmd::Close { number } => {
            let pr = c.close_pull(&name, number)?;
            emit(json, &pr, || println!("Closed #{}", pr.number));
        }
    }
    Ok(())
}

pub fn run_know(
    repo: &Repository,
    json: bool,
    remote: &str,
    cmd: KnowCmd,
    agent: Option<&str>,
) -> Result<()> {
    let (c, name) = client_as(repo, remote, agent)?;
    match cmd {
        KnowCmd::Add {
            title,
            body,
            body_file,
            tags,
            paths,
            commits,
            confidence,
        } => {
            let body = match body_file {
                Some(p) if p.as_os_str() == "-" => {
                    let mut s = String::new();
                    std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
                    s
                }
                Some(p) => std::fs::read_to_string(p)?,
                None => body,
            };
            let commits = commits
                .iter()
                .map(|r| Ok(repo.resolve(r)?))
                .collect::<Result<Vec<_>>>()?;
            let n = c.create_note(
                &name,
                &NoteCreate {
                    title,
                    body,
                    tags,
                    paths,
                    commits,
                    pulls: vec![],
                    confidence,
                    ..Default::default()
                },
            )?;
            emit(json, &n, || println!("note {} created: {}", n.id, n.title));
        }
        KnowCmd::Search {
            tags,
            path,
            query,
            limit,
        } => {
            let mut q = format!("?limit={limit}");
            if !tags.is_empty() {
                q.push_str(&format!("&tags={}", tags.join(",")));
            }
            if let Some(p) = path {
                q.push_str(&format!("&path={}", agit_protocol::client::urlencode(&p)));
            }
            if let Some(s) = query {
                q.push_str(&format!("&q={}", agit_protocol::client::urlencode(&s)));
            }
            let list = c.notes(&name, &q)?;
            emit(json, &list, || {
                for n in &list {
                    println!(
                        "{} {:<40} [{}] {}",
                        n.id,
                        n.title,
                        n.tags.join(", "),
                        n.paths.join(" ")
                    );
                }
            });
        }
        KnowCmd::Get { id } => {
            let n = c.note(&name, &id)?;
            emit(json, &n, || {
                println!("# {}\n", n.title);
                println!(
                    "tags: {}\npaths: {}\nauthor: {}  confidence: {}\n",
                    n.tags.join(", "),
                    n.paths.join(" "),
                    n.author,
                    n.confidence
                );
                println!("{}", n.body);
            });
        }
        KnowCmd::Context { path } => {
            let ctx = c.context(&name, &path)?;
            emit(json, &ctx, || {
                println!("context for {}", ctx.path);
                if !ctx.suggested_tags.is_empty() {
                    println!("suggested tags: {}", ctx.suggested_tags.join(", "));
                }
                println!("\nnotes ({}):", ctx.notes.len());
                for n in &ctx.notes {
                    println!("  {} {} [{}]", n.id, n.title, n.tags.join(", "));
                }
                println!("\nrecent commits ({}):", ctx.recent_commits.len());
                for cm in &ctx.recent_commits {
                    println!(
                        "  {} {} {}",
                        cm.id.short(),
                        cm.commit.summary(),
                        cm.commit
                            .agent
                            .as_ref()
                            .map(|a| format!("({})", a.id))
                            .unwrap_or_default()
                    );
                }
                println!("\nleases ({}):", ctx.leases.len());
                for l in &ctx.leases {
                    println!("  {} {} {}", l.id, l.holder, l.paths.join(" "));
                }
                println!("\nopen pull requests ({}):", ctx.open_pulls.len());
                for p in &ctx.open_pulls {
                    println!("  #{} {} ({})", p.number, p.title, p.author.id);
                }
            });
        }
        KnowCmd::Tags => {
            let list = c.tags(&name)?;
            emit(json, &list, || {
                for t in &list {
                    println!("{:<30} {}", t.tag, t.count);
                }
            });
        }
    }
    Ok(())
}
