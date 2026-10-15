use std::collections::BTreeMap;
use std::path::PathBuf;

use agit_core::diff::{self, ChangeKind};
use agit_core::lease::{LeaseMode, LeaseRequest};
use agit_core::ops::{self, CommitOptions, LogOptions, MergeOptions, MergeResult};
use agit_core::worktree;
use agit_core::{AgentInfo, Object, ObjectId, Repository};
use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};

use crate::output::{emit, fmt_time, parse_duration};

#[derive(Parser)]
#[command(
    name = "agit",
    version,
    about = "Version control with git's workflow, built for AI agents",
    long_about = None,
    propagate_version = true
)]
pub struct Cli {
    /// Machine-readable JSON output (stable envelope: {ok, result|error}).
    #[arg(long, global = true, env = "AGIT_JSON")]
    pub json: bool,
    /// Run as if started in this directory.
    #[arg(short = 'C', global = true, value_name = "DIR")]
    pub dir: Option<PathBuf>,
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Args, Clone, Default)]
pub struct AgentArgs {
    /// Agent identity recorded in the commit (also used as lease holder).
    #[arg(long, env = "AGIT_AGENT", global = true)]
    pub agent: Option<String>,
    /// Model name/version of the agent.
    #[arg(long, env = "AGIT_MODEL", global = true)]
    pub model: Option<String>,
    /// Run/session/trace id for correlation.
    #[arg(long, env = "AGIT_RUN_ID", global = true)]
    pub run_id: Option<String>,
    /// Human or agent this agent acts on behalf of.
    #[arg(long, env = "AGIT_ON_BEHALF_OF", global = true)]
    pub on_behalf_of: Option<String>,
}

impl AgentArgs {
    fn info(&self) -> Option<AgentInfo> {
        self.agent.as_ref().map(|id| AgentInfo {
            id: id.clone(),
            model: self.model.clone(),
            run_id: self.run_id.clone(),
            on_behalf_of: self.on_behalf_of.clone(),
        })
    }
}

#[derive(Subcommand)]
pub enum Cmd {
    /// Create an empty repository.
    Init {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Stage file contents (files, directories, or everything with -A).
    Add {
        paths: Vec<PathBuf>,
        /// Stage all changes in the working tree.
        #[arg(short = 'A', long)]
        all: bool,
        /// Also add files that match ignore rules.
        #[arg(short, long)]
        force: bool,
    },
    /// Remove files from the index (and the working tree unless --cached).
    Rm {
        paths: Vec<PathBuf>,
        #[arg(long)]
        cached: bool,
    },
    /// Unstage paths (reset index entries to HEAD).
    Reset { paths: Vec<PathBuf> },
    /// Show staged, unstaged, untracked and conflicted paths.
    Status {
        /// Compact one-line-per-path output.
        #[arg(short, long)]
        short: bool,
    },
    /// Record the staged changes as a commit.
    Commit {
        #[arg(short, long)]
        message: String,
        /// Why this change exists (stored separately from the message).
        #[arg(long)]
        intent: Option<String>,
        /// External task/ticket id.
        #[arg(long)]
        task: Option<String>,
        /// Extra metadata, `key=value` (value parsed as JSON when possible).
        #[arg(long = "meta", value_name = "K=V")]
        meta: Vec<String>,
        /// Stage tracked modifications before committing.
        #[arg(short, long)]
        all: bool,
        #[arg(long)]
        allow_empty: bool,
        /// Ignore other agents' exclusive leases.
        #[arg(long)]
        force: bool,
        #[command(flatten)]
        agent: AgentArgs,
    },
    /// Snapshot the working tree (tracked + untracked) without moving HEAD.
    Checkpoint {
        #[arg(short, long)]
        message: Option<String>,
        #[command(subcommand)]
        cmd: Option<CheckpointCmd>,
        #[command(flatten)]
        agent: AgentArgs,
    },
    /// Restore the working tree to any commit or checkpoint.
    Restore { rev: String },
    /// Show commit history.
    Log {
        #[arg(default_value = "HEAD")]
        rev: String,
        #[arg(short = 'n', long)]
        limit: Option<usize>,
        /// Only commits by this agent id.
        #[arg(long)]
        agent: Option<String>,
        /// Only commits for this task id.
        #[arg(long)]
        task: Option<String>,
        /// Only commits touching this path.
        #[arg(long)]
        path: Option<PathBuf>,
        #[arg(long)]
        first_parent: bool,
        #[arg(long)]
        oneline: bool,
    },
    /// Show a commit (metadata + diff) or any object.
    Show {
        #[arg(default_value = "HEAD")]
        rev: String,
        /// Omit the patch.
        #[arg(long)]
        no_patch: bool,
    },
    /// Show changes between working tree, index and commits.
    Diff {
        /// Compare index with HEAD instead of working tree with index.
        #[arg(long, alias = "cached")]
        staged: bool,
        /// Revisions: none, one (vs working tree) or two.
        revs: Vec<String>,
        #[arg(long)]
        stat: bool,
        #[arg(long)]
        name_only: bool,
        /// Lines of context.
        #[arg(short = 'U', long, default_value_t = 3)]
        unified: usize,
    },
    /// List, create or delete branches.
    Branch {
        name: Option<String>,
        start: Option<String>,
        /// Delete (refuses unmerged branches).
        #[arg(short = 'd', long)]
        delete: bool,
        /// Force delete.
        #[arg(short = 'D')]
        force_delete: bool,
    },
    /// Switch branches or detach at a revision.
    #[command(alias = "checkout")]
    Switch {
        target: String,
        /// Create the branch first.
        #[arg(short = 'c', long)]
        create: bool,
        /// Discard local modifications that would be overwritten.
        #[arg(short, long)]
        force: bool,
    },
    /// Merge a revision into the current branch.
    Merge {
        rev: Option<String>,
        #[arg(short, long)]
        message: Option<String>,
        #[arg(long)]
        intent: Option<String>,
        #[arg(long)]
        no_ff: bool,
        /// Abort an in-progress merge.
        #[arg(long)]
        abort: bool,
        #[arg(long)]
        force: bool,
        #[command(flatten)]
        agent: AgentArgs,
    },
    /// List, create or delete tags.
    Tag {
        name: Option<String>,
        rev: Option<String>,
        #[arg(short = 'd', long)]
        delete: bool,
    },
    /// Claim, release and inspect path leases for multi-agent coordination.
    Lease {
        #[command(subcommand)]
        cmd: LeaseCmd,
    },
    /// Manage workspaces (separate working directories sharing one store).
    #[command(alias = "workspace")]
    Ws {
        #[command(subcommand)]
        cmd: WsCmd,
    },
    /// Read the repository event stream.
    Events {
        /// Only events with seq greater than this.
        #[arg(long, default_value_t = 0)]
        since: u64,
        /// Keep reading new events.
        #[arg(short, long)]
        follow: bool,
        /// Filter by kind prefix (e.g. `commit`, `lease.`).
        #[arg(long)]
        kind: Option<String>,
    },
    /// Resolve a revision expression to a full object id.
    RevParse { rev: String },
    /// Print an object's type or content.
    CatFile {
        id: String,
        /// Print the type only.
        #[arg(short = 't')]
        type_only: bool,
    },
    /// List the contents of a tree.
    LsTree {
        #[arg(default_value = "HEAD")]
        rev: String,
        #[arg(short, long)]
        recursive: bool,
    },
    /// Verify object integrity and reachability.
    Fsck,
    /// Get or set configuration (user.name, user.email, default_branch).
    Config { key: Option<String>, value: Option<String> },
    /// Manage server remotes.
    Remote {
        #[command(subcommand)]
        cmd: crate::remote::RemoteCmd,
    },
    /// Upload a branch to a remote (compare-and-swap on the remote ref).
    Push {
        #[arg(default_value = "origin")]
        remote: String,
        /// Branch to push (default: current).
        branch: Option<String>,
        #[arg(short, long)]
        force: bool,
        /// Create the remote repository if it does not exist.
        #[arg(long)]
        create: bool,
    },
    /// Download branches and tags from a remote into refs/remotes/<remote>/.
    Fetch {
        #[arg(default_value = "origin")]
        remote: String,
        branches: Vec<String>,
    },
    /// Fetch the current branch and merge it.
    Pull {
        #[arg(default_value = "origin")]
        remote: String,
        #[arg(long)]
        no_ff: bool,
        #[command(flatten)]
        agent: AgentArgs,
    },
    /// Clone a repository from a server URL (.../api/v1/repos/<name>).
    Clone {
        url: String,
        /// Destination directory (default: repository name).
        dest: Option<PathBuf>,
    },
    /// Pull requests on the remote.
    Pr {
        #[arg(long, default_value = "origin")]
        remote: String,
        #[command(subcommand)]
        cmd: crate::remote::PrCmd,
    },
    /// Linked repositories pinned to a commit (.agitmodules).
    Submodule {
        #[command(subcommand)]
        cmd: SubmoduleCmd,
    },
    /// Run as an MCP server over stdio (tools for AI agents).
    Mcp {
        /// Remote used by pr/know/context tools.
        #[arg(long, default_value = "origin")]
        remote: String,
    },
    /// Knowledge base on the remote: notes, tags, context for a path.
    Know {
        #[arg(long, default_value = "origin")]
        remote: String,
        #[command(subcommand)]
        cmd: crate::remote::KnowCmd,
    },
}

#[derive(Subcommand)]
pub enum SubmoduleCmd {
    /// Clone a repository into <path>, register it and stage the pin.
    Add {
        url: String,
        path: String,
        #[arg(long)]
        branch: Option<String>,
    },
    /// Clone missing submodules and check out the pinned commits.
    Update { paths: Vec<String> },
    /// Show pinned vs checked-out commit per submodule.
    Status,
    /// Unregister a submodule (and delete its directory with --delete).
    Remove {
        path: String,
        #[arg(long)]
        delete: bool,
    },
}

#[derive(Subcommand)]
pub enum CheckpointCmd {
    /// List checkpoints of this workspace.
    List {
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
    },
    /// Restore the working tree to a checkpoint.
    Restore { rev: String },
}

#[derive(Subcommand)]
pub enum LeaseCmd {
    /// Claim paths (literal prefixes or globs).
    Claim {
        paths: Vec<String>,
        #[arg(long, env = "AGIT_AGENT")]
        holder: String,
        /// Time to live, e.g. 30m, 2h (default 1h).
        #[arg(long, default_value = "1h")]
        ttl: String,
        #[arg(long)]
        note: Option<String>,
        /// Informational lease that does not block others.
        #[arg(long)]
        shared: bool,
    },
    /// Release a lease by id, or all leases of --holder.
    Release {
        id: Option<String>,
        #[arg(long, env = "AGIT_AGENT")]
        holder: Option<String>,
        #[arg(long)]
        all: bool,
    },
    /// Extend a lease.
    Renew {
        id: String,
        #[arg(long, default_value = "1h")]
        ttl: String,
    },
    /// List active leases.
    List,
    /// Check whether --holder may modify the given paths.
    Check {
        paths: Vec<String>,
        #[arg(long, env = "AGIT_AGENT")]
        holder: String,
    },
}

#[derive(Subcommand)]
pub enum WsCmd {
    /// Create a workspace on its own branch `ws/<name>`.
    New {
        name: String,
        /// Directory for the working tree (default: sibling `<repo>.<name>`).
        #[arg(long)]
        path: Option<PathBuf>,
        /// Start revision (default: current HEAD).
        #[arg(long)]
        from: Option<String>,
        /// Agent that owns the workspace.
        #[arg(long, env = "AGIT_AGENT")]
        agent: Option<String>,
    },
    List,
    /// Remove a workspace registration (and its directory with --delete).
    Remove {
        name: String,
        #[arg(long)]
        delete: bool,
    },
}

fn open(dir: &Option<PathBuf>) -> Result<Repository> {
    let start = match dir {
        Some(d) => d.clone(),
        None => std::env::current_dir()?,
    };
    Ok(Repository::discover(&start)?)
}

fn cwd(dir: &Option<PathBuf>) -> Result<PathBuf> {
    Ok(match dir {
        Some(d) => agit_core::repo::absolutize(d)?,
        None => std::env::current_dir()?,
    })
}

fn rels(repo: &Repository, cwd: &std::path::Path, paths: &[PathBuf]) -> Result<Vec<String>> {
    paths.iter().map(|p| Ok(repo.rel_path(cwd, p)?)).collect()
}

pub fn run(cli: Cli) -> Result<()> {
    let json = cli.json;
    match cli.cmd {
        Cmd::Init { path } => {
            let base = cwd(&cli.dir)?;
            let target = if path.is_absolute() { path } else { base.join(path) };
            let repo = Repository::init(&target)?;
            emit(json, &serde_json::json!({ "path": repo.work_dir(), "common_dir": repo.common_dir() }), || {
                println!("Initialized empty agit repository in {}", repo.common_dir().display());
            });
        }
        Cmd::Add { paths, all, force } => {
            let repo = open(&cli.dir)?;
            if paths.is_empty() && !all {
                bail!("nothing specified; use `agit add <paths>` or `agit add -A`");
            }
            let rel = if all { vec![] } else { rels(&repo, &cwd(&cli.dir)?, &paths)? };
            let r = worktree::add(&repo, &rel, force)?;
            emit(json, &r, || {
                for p in &r.added {
                    println!("add      {p}");
                }
                for p in &r.updated {
                    println!("update   {p}");
                }
                for p in &r.removed {
                    println!("remove   {p}");
                }
            });
        }
        Cmd::Rm { paths, cached } => {
            let repo = open(&cli.dir)?;
            let rel = rels(&repo, &cwd(&cli.dir)?, &paths)?;
            let removed = worktree::remove(&repo, &rel, !cached)?;
            emit(json, &serde_json::json!({ "removed": removed }), || {
                for p in &removed {
                    println!("rm '{p}'");
                }
            });
        }
        Cmd::Reset { paths } => {
            let repo = open(&cli.dir)?;
            let rel = rels(&repo, &cwd(&cli.dir)?, &paths)?;
            let touched = worktree::unstage(&repo, &rel)?;
            emit(json, &serde_json::json!({ "unstaged": touched }), || {
                println!("Unstaged {} path(s)", touched.len());
            });
        }
        Cmd::Status { short } => {
            let repo = open(&cli.dir)?;
            let st = worktree::status(&repo, true)?;
            emit(json, &st, || print_status(&st, short));
        }
        Cmd::Commit { message, intent, task, meta, all, allow_empty, force, agent } => {
            let repo = open(&cli.dir)?;
            let r = ops::commit(
                &repo,
                CommitOptions {
                    message,
                    intent,
                    task,
                    agent: agent.info(),
                    meta: parse_meta(&meta)?,
                    author: None,
                    allow_empty,
                    force,
                    all,
                },
            )?;
            emit(json, &r, || {
                let b = r.branch.clone().unwrap_or_else(|| "detached HEAD".into());
                println!("[{b} {}] {}", r.id.short(), r.summary);
                println!(" {} file(s) changed", r.changes.len());
            });
        }
        Cmd::Checkpoint { message, cmd, agent } => {
            let repo = open(&cli.dir)?;
            match cmd {
                None => {
                    let r = ops::checkpoint(&repo, message, agent.info())?;
                    emit(json, &r, || {
                        println!("checkpoint {} ({} file(s) differ from HEAD)", r.id.short(), r.changes_vs_head);
                    });
                }
                Some(CheckpointCmd::List { limit }) => {
                    let list = ops::list_checkpoints(&repo, limit)?;
                    let entries: Vec<ops::LogEntry> = list
                        .into_iter()
                        .map(|(id, commit)| ops::LogEntry { id, commit })
                        .collect();
                    emit(json, &entries, || {
                        for e in &entries {
                            println!("{} {} {}", e.id.short(), fmt_time(e.commit.time_ms), e.commit.summary());
                        }
                    });
                }
                Some(CheckpointCmd::Restore { rev }) => {
                    let changes = ops::restore(&repo, &rev)?;
                    emit(json, &changes, || println!("Restored {} file(s)", changes.len()));
                }
            }
        }
        Cmd::Restore { rev } => {
            let repo = open(&cli.dir)?;
            let changes = ops::restore(&repo, &rev)?;
            emit(json, &changes, || println!("Restored {} file(s)", changes.len()));
        }
        Cmd::Log { rev, limit, agent, task, path, first_parent, oneline } => {
            let repo = open(&cli.dir)?;
            let start = match repo.resolve(&rev) {
                Ok(id) => vec![id],
                Err(agit_core::Error::UnknownRevision(r)) if r.starts_with("HEAD") => vec![],
                Err(e) => return Err(e.into()),
            };
            let path = match path {
                Some(p) => Some(repo.rel_path(&cwd(&cli.dir)?, &p)?),
                None => None,
            };
            let entries = ops::log(&repo, &start, &LogOptions { limit, agent, task, path, first_parent })?;
            emit(json, &entries, || {
                for e in &entries {
                    if oneline {
                        println!("{} {}", e.id.short(), e.commit.summary());
                    } else {
                        print_commit_header(&e.id, &e.commit);
                        println!();
                    }
                }
            });
        }
        Cmd::Show { rev, no_patch } => {
            let repo = open(&cli.dir)?;
            let id = repo.resolve(&rev)?;
            match repo.store().read(&id)? {
                Object::Commit(c) => {
                    let parent_tree = match c.parents.first() {
                        Some(p) => Some(repo.store().read_commit(p)?.tree),
                        None => None,
                    };
                    let changes = diff::diff_trees(repo.store(), parent_tree.as_ref(), Some(&c.tree))?;
                    let patch = if no_patch { None } else { Some(render_patch(&repo, &changes, 3)?) };
                    emit(json, &serde_json::json!({ "id": id, "commit": c, "changes": changes, "patch": patch }), || {
                        print_commit_header(&id, &c);
                        println!();
                        if let Some(p) = patch {
                            print!("{p}");
                        } else {
                            for ch in &changes {
                                println!("{} {}", ch.kind.letter(), ch.path);
                            }
                        }
                    });
                }
                Object::Tree(t) => {
                    emit(json, &t.entries(), || {
                        for e in t.entries() {
                            println!("{:06o} {} {}\t{}", e.mode.code(), e.id, e.size, e.name);
                        }
                    });
                }
                Object::Blob(b) => {
                    if json {
                        emit(json, &serde_json::json!({ "id": id, "size": b.len(), "binary": diff::is_binary(&b), "content": String::from_utf8_lossy(&b) }), || {});
                    } else {
                        use std::io::Write;
                        std::io::stdout().write_all(&b)?;
                    }
                }
            }
        }
        Cmd::Diff { staged, revs, stat, name_only, unified } => {
            let repo = open(&cli.dir)?;
            let (old, new): (BTreeMap<String, diff::Side>, BTreeMap<String, diff::Side>) = match revs.len() {
                0 if staged => {
                    let head = repo.head_tree()?;
                    (diff::tree_to_sides(repo.store(), head.as_ref())?, index_sides(&repo)?)
                }
                0 => (index_sides(&repo)?, worktree_sides(&repo)?),
                1 => {
                    let t = repo.resolve_commit(&revs[0])?.1.tree;
                    (diff::tree_to_sides(repo.store(), Some(&t))?, worktree_sides(&repo)?)
                }
                2 => {
                    let a = repo.resolve_commit(&revs[0])?.1.tree;
                    let b = repo.resolve_commit(&revs[1])?.1.tree;
                    (diff::tree_to_sides(repo.store(), Some(&a))?, diff::tree_to_sides(repo.store(), Some(&b))?)
                }
                _ => bail!("diff takes at most two revisions"),
            };
            let changes = diff::diff_maps(&old, &new);
            let from_worktree = revs.len() <= 1 && !staged;
            let patch = if name_only || stat { None } else { Some(render_patch_sides(&repo, &changes, unified, from_worktree)?) };
            let stats: Vec<(String, usize, usize)> = if stat || json {
                changes.iter().map(|c| {
                    let (a, d) = blob_stats(&repo, c, from_worktree).unwrap_or((0, 0));
                    (c.path.clone(), a, d)
                }).collect()
            } else {
                vec![]
            };
            emit(json, &serde_json::json!({ "changes": changes, "stats": stats.iter().map(|(p,a,d)| serde_json::json!({"path":p,"added":a,"removed":d})).collect::<Vec<_>>(), "patch": patch }), || {
                if name_only {
                    for c in &changes {
                        println!("{}", c.path);
                    }
                } else if stat {
                    let (mut ta, mut td) = (0, 0);
                    for (p, a, d) in &stats {
                        ta += a;
                        td += d;
                        println!(" {p} | +{a} -{d}");
                    }
                    println!(" {} file(s) changed, {ta} insertion(s), {td} deletion(s)", changes.len());
                } else if let Some(p) = patch {
                    print!("{p}");
                }
            });
        }
        Cmd::Branch { name, start, delete, force_delete } => {
            let repo = open(&cli.dir)?;
            match name {
                None => {
                    let list = ops::list_branches(&repo)?;
                    emit(json, &list, || {
                        for b in &list {
                            println!("{} {} {}", if b.current { "*" } else { " " }, b.name, b.id.short());
                        }
                    });
                }
                Some(n) if delete || force_delete => {
                    let id = ops::delete_branch(&repo, &n, force_delete)?;
                    emit(json, &serde_json::json!({ "deleted": n, "was": id }), || println!("Deleted branch {n} (was {})", id.short()));
                }
                Some(n) => {
                    let b = ops::create_branch(&repo, &n, start.as_deref())?;
                    emit(json, &b, || println!("Created branch {} at {}", b.name, b.id.short()));
                }
            }
        }
        Cmd::Switch { target, create, force } => {
            let repo = open(&cli.dir)?;
            let r = ops::switch(&repo, &target, create, force)?;
            emit(json, &r, || match &r.head {
                ops::Head2::Branch { name } => println!("Switched to branch '{name}'"),
                ops::Head2::Detached => println!("HEAD is now at {}", r.id.short()),
            });
        }
        Cmd::Merge { rev, message, intent, no_ff, abort, force, agent } => {
            let repo = open(&cli.dir)?;
            if abort {
                ops::merge_abort(&repo)?;
                emit(json, &serde_json::json!({ "aborted": true }), || println!("Merge aborted"));
                return Ok(());
            }
            let rev = rev.context("merge needs a revision")?;
            let r = ops::merge(&repo, &rev, MergeOptions { message, intent, agent: agent.info(), no_ff, force })?;
            let conflicted = matches!(r, MergeResult::Conflicts { .. });
            emit(json, &r, || match &r {
                MergeResult::AlreadyUpToDate { .. } => println!("Already up to date."),
                MergeResult::FastForward { to, files_changed, .. } => println!("Fast-forward to {} ({files_changed} file(s))", to.short()),
                MergeResult::Merged { commit, auto_merged, .. } => println!("Merge made commit {} ({} file(s) auto-merged)", commit.short(), auto_merged.len()),
                MergeResult::Conflicts { conflicts, .. } => {
                    for c in conflicts {
                        println!("CONFLICT ({:?}): {}", c.kind, c.path);
                    }
                    println!("Automatic merge failed; fix conflicts, `agit add` them and commit.");
                }
            });
            if conflicted {
                if let MergeResult::Conflicts { conflicts, .. } = r {
                    return Err(agit_core::Error::MergeConflicts(conflicts.len()).into());
                }
            }
        }
        Cmd::Tag { name, rev, delete } => {
            let repo = open(&cli.dir)?;
            match name {
                None => {
                    let tags = ops::list_tags(&repo)?;
                    emit(json, &tags.iter().map(|(n, id)| serde_json::json!({"name": n, "id": id})).collect::<Vec<_>>(), || {
                        for (n, _) in &tags {
                            println!("{n}");
                        }
                    });
                }
                Some(n) if delete => {
                    let id = ops::delete_tag(&repo, &n)?;
                    emit(json, &serde_json::json!({ "deleted": n, "was": id }), || println!("Deleted tag {n}"));
                }
                Some(n) => {
                    let id = ops::create_tag(&repo, &n, rev.as_deref())?;
                    emit(json, &serde_json::json!({ "name": n, "id": id }), || println!("Tagged {} as {n}", id.short()));
                }
            }
        }
        Cmd::Lease { cmd } => run_lease(&cli.dir, json, cmd)?,
        Cmd::Remote { cmd } => crate::remote::run_remote(&open(&cli.dir)?, json, cmd)?,
        Cmd::Push { remote, branch, force, create } => crate::remote::run_push(&open(&cli.dir)?, json, &remote, branch, force, create)?,
        Cmd::Fetch { remote, branches } => crate::remote::run_fetch(&open(&cli.dir)?, json, &remote, &branches)?,
        Cmd::Pull { remote, no_ff, agent } => crate::remote::run_pull(&open(&cli.dir)?, json, &remote, no_ff, agent.info())?,
        Cmd::Clone { url, dest } => crate::remote::run_clone(json, &cwd(&cli.dir)?, &url, dest)?,
        Cmd::Pr { remote, cmd } => crate::remote::run_pr(&open(&cli.dir)?, json, &remote, cmd)?,
        Cmd::Know { remote, cmd } => crate::remote::run_know(&open(&cli.dir)?, json, &remote, cmd)?,
        Cmd::Mcp { remote } => crate::mcp::serve(cwd(&cli.dir)?, remote)?,
        Cmd::Submodule { cmd } => {
            let repo = open(&cli.dir)?;
            let cwd = cwd(&cli.dir)?;
            match cmd {
                SubmoduleCmd::Add { url, path, branch } => {
                    let rel = repo.rel_path(&cwd, std::path::Path::new(&path))?;
                    let m = agit_protocol::submodule::add(&repo, &url, &rel, branch.as_deref())?;
                    emit(json, &m, || println!("added submodule {} from {}", m.path, m.url));
                }
                SubmoduleCmd::Update { paths } => {
                    let rels = rels(&repo, &cwd, &paths.iter().map(PathBuf::from).collect::<Vec<_>>())?;
                    let r = agit_protocol::submodule::update(&repo, if rels.is_empty() { None } else { Some(&rels) })?;
                    emit(json, &r, || {
                        for u in &r {
                            println!("{:<12} {} {}", u.action, u.path, u.commit.map(|c| c.short()).unwrap_or_default());
                        }
                    });
                }
                SubmoduleCmd::Status => {
                    let r = agit_protocol::submodule::status(&repo)?;
                    emit(json, &r, || {
                        for s in &r {
                            println!("{:<9} {} pinned={} actual={}", s.state, s.path, s.pinned.map(|c| c.short()).unwrap_or_else(|| "-".into()), s.actual.map(|c| c.short()).unwrap_or_else(|| "-".into()));
                        }
                    });
                }
                SubmoduleCmd::Remove { path, delete } => {
                    let rel = repo.rel_path(&cwd, std::path::Path::new(&path))?;
                    agit_protocol::submodule::remove(&repo, &rel, delete)?;
                    emit(json, &serde_json::json!({ "removed": rel }), || println!("removed submodule {rel}"));
                }
            }
        }
        Cmd::Ws { cmd } => run_ws(&cli.dir, json, cmd)?,
        Cmd::Events { since, follow, kind } => {
            let repo = open(&cli.dir)?;
            let mut cursor = since;
            loop {
                let events = repo.events().read_after(cursor)?;
                for ev in events {
                    cursor = ev.seq;
                    if kind.as_ref().is_some_and(|k| !ev.kind.starts_with(k.as_str())) {
                        continue;
                    }
                    if json {
                        println!("{}", serde_json::to_string(&ev)?);
                    } else {
                        println!(
                            "#{} {} {:<16} {:<10} {} {}",
                            ev.seq,
                            fmt_time(ev.time_ms),
                            ev.kind,
                            ev.workspace.as_deref().unwrap_or("-"),
                            ev.actor.as_deref().unwrap_or("-"),
                            ev.data
                        );
                    }
                }
                if !follow {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        }
        Cmd::RevParse { rev } => {
            let repo = open(&cli.dir)?;
            let id = repo.resolve(&rev)?;
            emit(json, &serde_json::json!({ "rev": rev, "id": id }), || println!("{id}"));
        }
        Cmd::CatFile { id, type_only } => {
            let repo = open(&cli.dir)?;
            let oid: ObjectId = match id.parse() {
                Ok(o) => o,
                Err(_) => repo.resolve(&id).or_else(|_| repo.store().resolve_prefix(&id))?,
            };
            let kind = repo.store().kind(&oid)?;
            if type_only {
                emit(json, &serde_json::json!({ "id": oid, "type": kind }), || println!("{kind}"));
                return Ok(());
            }
            match repo.store().read(&oid)? {
                Object::Blob(b) => {
                    use std::io::Write;
                    std::io::stdout().write_all(&b)?;
                }
                Object::Tree(t) => {
                    emit(json, &t.entries(), || {
                        for e in t.entries() {
                            println!("{:06o} {} {}\t{}", e.mode.code(), e.id, e.size, e.name);
                        }
                    });
                }
                Object::Commit(c) => {
                    emit(json, &c, || println!("{}", serde_json::to_string_pretty(&c).unwrap()));
                }
            }
        }
        Cmd::LsTree { rev, recursive } => {
            let repo = open(&cli.dir)?;
            let tree = repo.resolve_commit(&rev)?.1.tree;
            if recursive {
                let flat = agit_core::index::flatten_tree(repo.store(), &tree)?;
                emit(json, &flat.iter().map(|(p, e)| serde_json::json!({"path": p, "mode": e.mode, "id": e.id, "size": e.size})).collect::<Vec<_>>(), || {
                    for (p, e) in &flat {
                        println!("{:06o} {} {}\t{}", e.mode.code(), e.id, e.size, p);
                    }
                });
            } else {
                let t = repo.store().read_tree(&tree)?;
                emit(json, &t.entries(), || {
                    for e in t.entries() {
                        println!("{:06o} {} {}\t{}", e.mode.code(), e.id, e.size, e.name);
                    }
                });
            }
        }
        Cmd::Fsck => {
            let repo = open(&cli.dir)?;
            let r = ops::fsck(&repo)?;
            let bad = !r.corrupt.is_empty() || !r.missing.is_empty() || !r.dangling_refs.is_empty();
            emit(json, &r, || {
                println!("{} object(s) checked", r.objects);
                for c in &r.corrupt {
                    println!("corrupt: {c}");
                }
                for m in &r.missing {
                    println!("missing: {m}");
                }
                for d in &r.dangling_refs {
                    println!("dangling ref: {d}");
                }
                if !bad {
                    println!("ok");
                }
            });
            if bad {
                bail!("fsck found problems");
            }
        }
        Cmd::Config { key, value } => {
            let repo = open(&cli.dir)?;
            let mut cfg = repo.config()?;
            match (key, value) {
                (None, _) => emit(json, &cfg, || println!("{}", serde_json::to_string_pretty(&cfg).unwrap())),
                (Some(k), None) => {
                    let v = match k.as_str() {
                        "user.name" => serde_json::json!(cfg.user.name),
                        "user.email" => serde_json::json!(cfg.user.email),
                        "default_branch" => serde_json::json!(cfg.default_branch),
                        _ => bail!("unknown config key {k}"),
                    };
                    emit(json, &serde_json::json!({ "key": k, "value": v }), || println!("{}", v.as_str().unwrap_or("")));
                }
                (Some(k), Some(v)) => {
                    match k.as_str() {
                        "user.name" => cfg.user.name = Some(v.clone()),
                        "user.email" => cfg.user.email = Some(v.clone()),
                        "default_branch" => cfg.default_branch = v.clone(),
                        _ => bail!("unknown config key {k}"),
                    }
                    repo.save_config(&cfg)?;
                    emit(json, &serde_json::json!({ "key": k, "value": v }), || {});
                }
            }
        }
    }
    Ok(())
}

fn run_lease(dir: &Option<PathBuf>, json: bool, cmd: LeaseCmd) -> Result<()> {
    let repo = open(dir)?;
    let leases = repo.leases();
    match cmd {
        LeaseCmd::Claim { paths, holder, ttl, note, shared } => {
            if paths.is_empty() {
                bail!("lease claim needs at least one path");
            }
            let cwd = cwd(dir)?;
            let paths: Vec<String> = paths
                .iter()
                .map(|p| {
                    if p.contains(['*', '?', '[', '{']) {
                        Ok(p.clone())
                    } else {
                        Ok(repo.rel_path(&cwd, std::path::Path::new(p))?)
                    }
                })
                .collect::<Result<_>>()?;
            let lease = leases.claim(LeaseRequest {
                holder: holder.clone(),
                paths,
                mode: if shared { LeaseMode::Shared } else { LeaseMode::Exclusive },
                ttl: Some(parse_duration(&ttl)?),
                note,
                workspace: Some(repo.workspace().to_string()),
            })?;
            repo.events().append("lease.claim", Some(repo.workspace()), Some(&holder), serde_json::to_value(&lease)?)?;
            emit(json, &lease, || println!("lease {} granted to {} for {}", lease.id, lease.holder, lease.paths.join(", ")));
        }
        LeaseCmd::Release { id, holder, all } => {
            let released = if all {
                let h = holder.context("--all needs --holder")?;
                leases.release_all(&h)?
            } else {
                let id = id.context("lease release needs an id or --all")?;
                vec![leases.release(&id)?]
            };
            for l in &released {
                repo.events().append("lease.release", Some(repo.workspace()), Some(&l.holder), serde_json::json!({ "id": l.id, "paths": l.paths }))?;
            }
            emit(json, &released, || println!("released {} lease(s)", released.len()));
        }
        LeaseCmd::Renew { id, ttl } => {
            let l = leases.renew(&id, parse_duration(&ttl)?)?;
            repo.events().append("lease.renew", Some(repo.workspace()), Some(&l.holder), serde_json::json!({ "id": l.id, "expires_ms": l.expires_ms }))?;
            emit(json, &l, || println!("lease {} renewed until {}", l.id, l.expires_ms.map(fmt_time).unwrap_or_default()));
        }
        LeaseCmd::List => {
            let list = leases.list()?;
            emit(json, &list, || {
                for l in &list {
                    println!(
                        "{} {:<10} {:<9} {} {}",
                        l.id,
                        l.holder,
                        format!("{:?}", l.mode).to_lowercase(),
                        l.expires_ms.map(fmt_time).unwrap_or_else(|| "never".into()),
                        l.paths.join(", ")
                    );
                }
            });
        }
        LeaseCmd::Check { paths, holder } => {
            let cwd = cwd(dir)?;
            let paths: Vec<String> = paths
                .iter()
                .map(|p| if p.contains(['*', '?', '[', '{']) { Ok(p.clone()) } else { Ok(repo.rel_path(&cwd, std::path::Path::new(p))?) })
                .collect::<Result<_>>()?;
            ops::enforce_leases(leases, &holder, &paths)?;
            emit(json, &serde_json::json!({ "allowed": true, "holder": holder, "paths": paths }), || println!("ok"));
        }
    }
    Ok(())
}

fn run_ws(dir: &Option<PathBuf>, json: bool, cmd: WsCmd) -> Result<()> {
    let repo = open(dir)?;
    match cmd {
        WsCmd::New { name, path, from, agent } => {
            let start = match from {
                Some(r) => Some(repo.resolve(&r)?),
                None => None,
            };
            let path = match path {
                Some(p) => Some(if p.is_absolute() { p } else { cwd(dir)?.join(p) }),
                None => None,
            };
            let (_ws, meta) = repo.create_workspace(&name, path.as_deref(), start, agent.as_deref())?;
            emit(json, &meta, || println!("workspace {} created at {} (branch ws/{})", meta.name, meta.path.display(), meta.name));
        }
        WsCmd::List => {
            let list = repo.list_workspaces()?;
            emit(json, &list, || {
                for w in &list {
                    println!("{} {}{}", if w.name == repo.workspace() { "*" } else { " " }, w.name, w.agent.as_ref().map(|a| format!(" ({a})")).unwrap_or_default());
                    println!("    {}", w.path.display());
                }
            });
        }
        WsCmd::Remove { name, delete } => {
            let meta = repo.remove_workspace(&name, delete)?;
            emit(json, &meta, || println!("workspace {} removed", meta.name));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn parse_meta(items: &[String]) -> Result<BTreeMap<String, serde_json::Value>> {
    let mut out = BTreeMap::new();
    for item in items {
        let (k, v) = item
            .split_once('=')
            .with_context(|| format!("--meta expects key=value, got {item:?}"))?;
        let value = serde_json::from_str(v).unwrap_or_else(|_| serde_json::Value::String(v.to_string()));
        out.insert(k.to_string(), value);
    }
    Ok(out)
}

fn index_sides(repo: &Repository) -> Result<BTreeMap<String, diff::Side>> {
    Ok(repo
        .load_index()?
        .iter()
        .map(|e| (e.path.clone(), diff::Side { id: e.id, mode: e.mode, size: e.size }))
        .collect())
}

/// Working tree as sides, using hashes (not stored) for modified files.
fn worktree_sides(repo: &Repository) -> Result<BTreeMap<String, diff::Side>> {
    let mut sides = index_sides(repo)?;
    let st = worktree::status(repo, true)?;
    for c in st.unstaged {
        match c.kind {
            ChangeKind::Deleted => {
                sides.remove(&c.path);
            }
            _ => {
                sides.insert(c.path.clone(), c.new.unwrap());
            }
        }
    }
    Ok(sides)
}

fn read_side(repo: &Repository, path: &str, side: &diff::Side, from_worktree: bool) -> Result<Vec<u8>> {
    if from_worktree && !repo.store().contains(&side.id) {
        return Ok(std::fs::read(repo.abs_path(path))?);
    }
    Ok(repo.store().read_blob(&side.id)?)
}

fn render_patch(repo: &Repository, changes: &[diff::Change], ctx: usize) -> Result<String> {
    render_patch_sides(repo, changes, ctx, false)
}

fn render_patch_sides(repo: &Repository, changes: &[diff::Change], ctx: usize, from_worktree: bool) -> Result<String> {
    let mut out = String::new();
    for c in changes {
        let old = match &c.old {
            Some(s) => repo.store().read_blob(&s.id).unwrap_or_default(),
            None => vec![],
        };
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

fn blob_stats(repo: &Repository, c: &diff::Change, from_worktree: bool) -> Result<(usize, usize)> {
    let old = match &c.old {
        Some(s) => repo.store().read_blob(&s.id).unwrap_or_default(),
        None => vec![],
    };
    let new = match &c.new {
        Some(s) => read_side(repo, &c.path, s, from_worktree)?,
        None => vec![],
    };
    Ok(diff::line_stats(&old, &new))
}

fn print_commit_header(id: &ObjectId, c: &agit_core::Commit) {
    println!("commit {id}{}", if c.checkpoint { " (checkpoint)" } else { "" });
    if c.parents.len() > 1 {
        println!("Merge:  {}", c.parents.iter().map(|p| p.short()).collect::<Vec<_>>().join(" "));
    }
    println!("Author: {} <{}>", c.author.name, c.author.email);
    if let Some(a) = &c.agent {
        let mut s = a.id.clone();
        if let Some(m) = &a.model {
            s.push_str(&format!(" ({m})"));
        }
        if let Some(r) = &a.run_id {
            s.push_str(&format!(" run={r}"));
        }
        println!("Agent:  {s}");
    }
    println!("Date:   {}", fmt_time(c.time_ms));
    if let Some(i) = &c.intent {
        println!("Intent: {i}");
    }
    if let Some(t) = &c.task {
        println!("Task:   {t}");
    }
    println!();
    for line in c.message.lines() {
        println!("    {line}");
    }
}

fn print_status(st: &worktree::Status, short: bool) {
    if short {
        for c in &st.staged {
            println!("{}  {}", c.kind.letter(), c.path);
        }
        for c in &st.unstaged {
            println!(" {} {}", c.kind.letter(), c.path);
        }
        for p in &st.conflicts {
            println!("UU {p}");
        }
        for p in &st.untracked {
            println!("?? {p}");
        }
        return;
    }
    match &st.branch {
        Some(b) => println!("On branch {b}"),
        None => println!("HEAD detached at {}", st.head.map(|h| h.short()).unwrap_or_default()),
    }
    if st.workspace != "main" {
        println!("Workspace {}", st.workspace);
    }
    if st.merge_in_progress {
        println!("Merge in progress (fix conflicts and commit, or `agit merge --abort`)");
    }
    if !st.conflicts.is_empty() {
        println!("\nUnmerged paths:");
        for p in &st.conflicts {
            println!("\tboth modified:   {p}");
        }
    }
    if !st.staged.is_empty() {
        println!("\nChanges to be committed:");
        for c in &st.staged {
            println!("\t{:<12} {}", label(c.kind), c.path);
        }
    }
    if !st.unstaged.is_empty() {
        println!("\nChanges not staged for commit:");
        for c in &st.unstaged {
            println!("\t{:<12} {}", label(c.kind), c.path);
        }
    }
    if !st.untracked.is_empty() {
        println!("\nUntracked files:");
        for p in &st.untracked {
            println!("\t{p}");
        }
    }
    if st.is_clean() && st.untracked.is_empty() {
        println!("nothing to commit, working tree clean");
    }
}

fn label(k: ChangeKind) -> &'static str {
    match k {
        ChangeKind::Added => "new file:",
        ChangeKind::Modified => "modified:",
        ChangeKind::Deleted => "deleted:",
        ChangeKind::TypeChanged => "typechange:",
    }
}
