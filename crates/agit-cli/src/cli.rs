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
    #[arg(long, global = true, env = "AGIT_JSON", value_parser = clap::builder::FalseyValueParser::new())]
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
    /// `reset [paths]` unstages; `reset [--soft|--mixed|--hard] <rev>` moves the branch (undo via `agit reflog`).
    Reset {
        /// Paths to unstage, or a single revision when moving the branch.
        args: Vec<String>,
        #[arg(long, conflicts_with_all = ["soft", "mixed"])]
        hard: bool,
        #[arg(long)]
        soft: bool,
        #[arg(long)]
        mixed: bool,
    },
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
        /// Sign the commit (optionally naming the key; default: AGIT_KEY, the agent id, or `default`).
        #[arg(long, num_args = 0..=1, default_missing_value = "", value_name = "KEY")]
        sign: Option<String>,
        #[command(flatten)]
        agent: AgentArgs,
    },
    /// Snapshot the working tree (tracked + untracked) without moving HEAD.
    Checkpoint {
        #[arg(short, long)]
        message: Option<String>,
        /// Do nothing when the tree equals the last checkpoint (or HEAD).
        #[arg(long)]
        if_changed: bool,
        #[command(subcommand)]
        cmd: Option<CheckpointCmd>,
        #[command(flatten)]
        agent: AgentArgs,
    },
    /// Restore the working tree to any commit or checkpoint.
    Restore { rev: String },
    /// Print repository locations and state as JSON-friendly fields (works on an unborn branch).
    Root,
    /// Line-by-line attribution of a file.
    Blame {
        path: PathBuf,
        #[arg(default_value = "HEAD")]
        rev: String,
        #[arg(long, default_value_t = 500)]
        max_commits: usize,
    },
    /// Undo a commit's changes with a new commit.
    Revert {
        rev: String,
        #[command(flatten)]
        agent: AgentArgs,
    },
    /// Apply a commit's changes on top of HEAD.
    CherryPick {
        rev: String,
        #[command(flatten)]
        agent: AgentArgs,
    },
    /// Shelve working tree changes (tracked and untracked) and restore HEAD.
    Stash {
        #[command(subcommand)]
        cmd: Option<StashCmd>,
    },
    /// Regex search over the working tree or a revision.
    Grep {
        pattern: String,
        #[arg(long)]
        rev: Option<String>,
        #[arg(short = 'i', long)]
        ignore_case: bool,
        /// Skip files larger than this many bytes.
        #[arg(long, default_value_t = 1_048_576)]
        max_file_bytes: u64,
    },
    /// Show where a ref (default: HEAD) pointed over time, newest first.
    Reflog {
        #[arg(default_value = "HEAD")]
        name: String,
        #[arg(short = 'n', long)]
        limit: Option<usize>,
    },
    /// Remove unreachable objects, expire old checkpoints and reflog entries.
    Gc {
        /// Report only.
        #[arg(long)]
        dry_run: bool,
        /// Keep objects younger than this many seconds (default 120).
        #[arg(long, default_value_t = 120)]
        grace: u64,
        /// Expire reflog entries older than this (e.g. 90d, 0 = keep all).
        #[arg(long, default_value = "90d")]
        reflog_expire: String,
        /// Expire checkpoints older than this (e.g. 30d, 0 = keep all).
        #[arg(long, default_value = "30d")]
        checkpoint_expire: String,
    },
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
        /// Draw an ASCII graph of the history.
        #[arg(long)]
        graph: bool,
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
        /// Sign the merge commit (see `commit --sign`).
        #[arg(long, num_args = 0..=1, default_missing_value = "", value_name = "KEY")]
        sign: Option<String>,
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
    Config {
        key: Option<String>,
        value: Option<String>,
    },
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
        /// Also push every local tag.
        #[arg(long)]
        tags: bool,
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
        #[command(flatten)]
        agent: AgentArgs,
    },
    /// Linked repositories pinned to a commit (.agitmodules).
    Submodule {
        #[command(subcommand)]
        cmd: SubmoduleCmd,
    },
    /// Manage Ed25519 signing keys (~/.agit/keys).
    Key {
        #[command(subcommand)]
        cmd: KeyCmd,
    },
    /// Sign in to a server with an SSH key; manage authorized keys and tokens.
    Auth {
        /// Server base URL (default: the remote's server, then AGIT_SERVER_URL).
        #[arg(long)]
        url: Option<String>,
        #[arg(long, default_value = "origin")]
        remote: String,
        #[command(subcommand)]
        cmd: crate::auth::AuthCmd,
    },
    /// Verify a commit's signature.
    Verify {
        #[arg(default_value = "HEAD")]
        rev: String,
    },
    /// Bridge to git repositories.
    Git {
        #[command(subcommand)]
        cmd: GitCmd,
    },
    /// Run as an MCP server over stdio (tools for AI agents).
    Mcp {
        /// Remote used by pr/know/context tools.
        #[arg(long, default_value = "origin")]
        remote: String,
    },
    /// Knowledge base: notes, tags, context for a path (server first, local `.agit/knowledge` when offline).
    Know {
        #[arg(long, default_value = "origin")]
        remote: String,
        /// Use only the local store, never the server.
        #[arg(long)]
        local: bool,
        #[command(subcommand)]
        cmd: crate::remote::KnowCmd,
        #[command(flatten)]
        agent: AgentArgs,
    },
}

#[derive(Subcommand)]
pub enum StashCmd {
    /// Save changes and reset to HEAD (default when no subcommand is given).
    Push {
        #[arg(short, long)]
        message: Option<String>,
    },
    List,
    /// Re-apply and drop an entry (default: the newest).
    Pop {
        #[arg(default_value_t = 0)]
        index: usize,
    },
    /// Re-apply an entry, keeping it.
    Apply {
        #[arg(default_value_t = 0)]
        index: usize,
    },
    Drop {
        #[arg(default_value_t = 0)]
        index: usize,
    },
}

#[derive(Subcommand)]
pub enum KeyCmd {
    /// Generate a key (default name: AGIT_AGENT or `default`).
    Generate {
        name: Option<String>,
        /// Register the public key with the remote repository for --agent (default: key name).
        #[arg(long)]
        register: bool,
        #[arg(long, default_value = "origin")]
        remote: String,
        #[arg(long)]
        agent: Option<String>,
    },
    List,
    /// Print the public key.
    Show {
        name: Option<String>,
    },
    /// Register an existing key's public half with the remote repository.
    Register {
        name: Option<String>,
        #[arg(long, default_value = "origin")]
        remote: String,
        #[arg(long)]
        agent: Option<String>,
    },
    Delete {
        name: String,
    },
}

#[derive(Subcommand)]
pub enum GitCmd {
    /// Import a git repository's history (commits, trees, blobs, branches, tags).
    Import {
        /// Path to a git working copy or bare repository.
        source: PathBuf,
        /// Target directory for the new agit repository (default: current repository).
        #[arg(long)]
        into: Option<PathBuf>,
        /// Only these branches.
        #[arg(long = "branch")]
        branches: Vec<String>,
        #[arg(long)]
        no_tags: bool,
        /// Do not check out the default branch afterwards.
        #[arg(long)]
        no_checkout: bool,
    },
    /// Export this repository's history into a git repository (created when missing).
    Export {
        /// Target git repository directory.
        target: PathBuf,
        #[arg(long = "branch")]
        branches: Vec<String>,
        #[arg(long)]
        no_tags: bool,
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
            let target = if path.is_absolute() {
                path
            } else {
                base.join(path)
            };
            let repo = Repository::init(&target)?;
            emit(
                json,
                &serde_json::json!({ "path": repo.work_dir(), "common_dir": repo.common_dir() }),
                || {
                    println!(
                        "Initialized empty agit repository in {}",
                        repo.common_dir().display()
                    );
                },
            );
        }
        Cmd::Add { paths, all, force } => {
            let repo = open(&cli.dir)?;
            if paths.is_empty() && !all {
                bail!("nothing specified; use `agit add <paths>` or `agit add -A`");
            }
            let rel = if all {
                vec![]
            } else {
                rels(&repo, &cwd(&cli.dir)?, &paths)?
            };
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
        Cmd::Reset {
            args,
            hard,
            soft,
            mixed,
        } => {
            let repo = open(&cli.dir)?;
            let explicit_mode = hard || soft || mixed;
            let is_rev = args.len() == 1
                && !repo.work_dir().join(&args[0]).exists()
                && repo.resolve(&args[0]).is_ok();
            if explicit_mode || is_rev {
                let rev = args.first().cloned().unwrap_or_else(|| "HEAD".into());
                let mode = if hard {
                    ops::ResetMode::Hard
                } else if soft {
                    ops::ResetMode::Soft
                } else {
                    ops::ResetMode::Mixed
                };
                let r = ops::reset(&repo, &rev, mode)?;
                emit(json, &r, || {
                    println!("HEAD is now at {} ({:?})", r.to.short(), r.mode)
                });
            } else {
                let paths: Vec<PathBuf> = args.iter().map(PathBuf::from).collect();
                let rel = rels(&repo, &cwd(&cli.dir)?, &paths)?;
                let touched = worktree::unstage(&repo, &rel)?;
                emit(json, &serde_json::json!({ "unstaged": touched }), || {
                    println!("Unstaged {} path(s)", touched.len());
                });
            }
        }
        Cmd::Blame {
            path,
            rev,
            max_commits,
        } => {
            let repo = open(&cli.dir)?;
            let rel = repo.rel_path(&cwd(&cli.dir)?, &path)?;
            let id = repo.resolve(&rev)?;
            let b = agit_core::history::blame(repo.store(), id, &rel, max_commits)?;
            emit(json, &b, || {
                for l in &b.lines {
                    let c = &b.commits[&l.commit];
                    let who = c.agent.clone().unwrap_or_else(|| c.author.clone());
                    println!(
                        "{} ({:<12} {}) {:>4}: {}",
                        l.commit.short(),
                        truncate(&who, 12),
                        fmt_time(c.time_ms),
                        l.line,
                        l.text
                    );
                }
            });
        }
        Cmd::Revert { rev, agent } => {
            let repo = open(&cli.dir)?;
            let r = agit_core::history::revert(&repo, &rev, agent.info())?;
            print_apply(json, &r, "Reverted")?;
        }
        Cmd::CherryPick { rev, agent } => {
            let repo = open(&cli.dir)?;
            let r = agit_core::history::cherry_pick(&repo, &rev, agent.info())?;
            print_apply(json, &r, "Cherry-picked")?;
        }
        Cmd::Stash { cmd } => {
            let repo = open(&cli.dir)?;
            use agit_core::history as h;
            match cmd.unwrap_or(StashCmd::Push { message: None }) {
                StashCmd::Push { message } => {
                    let e = h::stash_push(&repo, message)?;
                    emit(json, &e, || {
                        println!("Saved working tree as stash@{{0}}: {}", e.message)
                    });
                }
                StashCmd::List => {
                    let l = h::stash_list(&repo)?;
                    emit(json, &l, || {
                        for e in &l {
                            println!(
                                "stash@{{{}}}: {} ({})",
                                e.index,
                                e.message,
                                fmt_time(e.time_ms)
                            );
                        }
                    });
                }
                StashCmd::Pop { index } => {
                    let r = h::stash_apply(&repo, index, true)?;
                    print_apply(json, &r, "Applied and dropped")?;
                }
                StashCmd::Apply { index } => {
                    let r = h::stash_apply(&repo, index, false)?;
                    print_apply(json, &r, "Applied")?;
                }
                StashCmd::Drop { index } => {
                    let id = h::stash_drop(&repo, index)?;
                    emit(json, &serde_json::json!({ "dropped": id }), || {
                        println!("Dropped stash@{{{index}}} ({})", id.short())
                    });
                }
            }
        }
        Cmd::Grep {
            pattern,
            rev,
            ignore_case,
            max_file_bytes,
        } => {
            let repo = open(&cli.dir)?;
            let m = agit_core::history::grep(
                &repo,
                &pattern,
                rev.as_deref(),
                ignore_case,
                max_file_bytes,
            )?;
            emit(json, &m, || {
                for x in &m {
                    println!("{}:{}:{}", x.path, x.line, x.text);
                }
            });
            if m.is_empty() {
                std::process::exit(1);
            }
        }
        Cmd::Root => {
            let repo = open(&cli.dir)?;
            let head = repo.head_commit()?;
            let info = serde_json::json!({
                "work_dir": repo.work_dir(),
                "common_dir": repo.common_dir(),
                "workspace": repo.workspace(),
                "branch": repo.current_branch()?,
                "head": head,
                "merge_in_progress": repo.merge_head()?.is_some(),
            });
            emit(json, &info, || println!("{}", repo.work_dir().display()));
        }
        Cmd::Reflog { name, limit } => {
            let repo = open(&cli.dir)?;
            let mut entries = if name == "HEAD" {
                repo.head_reflog()?
            } else {
                let full = if name.starts_with("refs/") {
                    name.clone()
                } else {
                    format!("refs/heads/{name}")
                };
                repo.refs().reflog(&full)?
            };
            entries.reverse();
            if let Some(l) = limit {
                entries.truncate(l);
            }
            emit(json, &entries, || {
                for (i, e) in entries.iter().enumerate() {
                    println!(
                        "{} {name}@{{{i}}}: {} ({}, {})",
                        e.new.map(|n| n.short()).unwrap_or_else(|| "deleted".into()),
                        e.message,
                        e.actor,
                        fmt_time(e.time_ms)
                    );
                }
            });
        }
        Cmd::Gc {
            dry_run,
            grace,
            reflog_expire,
            checkpoint_expire,
        } => {
            let repo = open(&cli.dir)?;
            let dur = |s: &str| -> Result<Option<u64>> {
                if s == "0" {
                    Ok(None)
                } else {
                    Ok(Some(parse_duration(s)?.as_secs()))
                }
            };
            let r = ops::gc(
                &repo,
                &ops::GcOptions {
                    grace_secs: grace,
                    reflog_expire_secs: dur(&reflog_expire)?,
                    checkpoint_expire_secs: dur(&checkpoint_expire)?,
                    dry_run,
                },
            )?;
            emit(json, &r, || {
                println!(
                    "{}{} object(s) total, {} reachable, {} removed ({} bytes), {} kept (younger than grace), {} reflog entries and {} checkpoints expired",
                    if r.dry_run { "[dry run] " } else { "" },
                    r.objects_total, r.objects_reachable, r.objects_removed, r.bytes_removed, r.objects_kept_young, r.reflog_entries_expired, r.checkpoints_expired
                );
            });
        }
        Cmd::Status { short } => {
            let repo = open(&cli.dir)?;
            let st = worktree::status(&repo, true)?;
            emit(json, &st, || print_status(&st, short));
        }
        Cmd::Commit {
            message,
            intent,
            task,
            meta,
            all,
            allow_empty,
            force,
            sign,
            agent,
        } => {
            let repo = open(&cli.dir)?;
            let sign_with = signing_key(sign.as_deref(), agent.agent.as_deref())?;
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
                    sign_with,
                },
            )?;
            emit(json, &r, || {
                let b = r.branch.clone().unwrap_or_else(|| "detached HEAD".into());
                println!("[{b} {}] {}", r.id.short(), r.summary);
                println!(" {} file(s) changed", r.changes.len());
            });
        }
        Cmd::Checkpoint {
            message,
            if_changed,
            cmd,
            agent,
        } => {
            let repo = open(&cli.dir)?;
            match cmd {
                None => {
                    let r = ops::checkpoint_opts(&repo, message, agent.info(), if_changed)?;
                    emit(json, &r, || {
                        println!(
                            "checkpoint {} ({} file(s) differ from HEAD)",
                            r.id.short(),
                            r.changes_vs_head
                        );
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
                            println!(
                                "{} {} {}",
                                e.id.short(),
                                fmt_time(e.commit.time_ms),
                                e.commit.summary()
                            );
                        }
                    });
                }
                Some(CheckpointCmd::Restore { rev }) => {
                    let changes = ops::restore(&repo, &rev)?;
                    emit(json, &changes, || {
                        println!("Restored {} file(s)", changes.len())
                    });
                }
            }
        }
        Cmd::Restore { rev } => {
            let repo = open(&cli.dir)?;
            let changes = ops::restore(&repo, &rev)?;
            emit(json, &changes, || {
                println!("Restored {} file(s)", changes.len())
            });
        }
        Cmd::Log {
            rev,
            limit,
            agent,
            task,
            path,
            first_parent,
            oneline,
            graph,
        } => {
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
            let entries = ops::log(
                &repo,
                &start,
                &LogOptions {
                    limit,
                    agent,
                    task,
                    path,
                    first_parent,
                },
            )?;
            emit(json, &entries, || {
                let prefixes = if graph {
                    agit_core::history::graph_prefixes(
                        &entries
                            .iter()
                            .map(|e| (e.id, e.commit.parents.clone()))
                            .collect::<Vec<_>>(),
                    )
                } else {
                    vec![]
                };
                for (i, e) in entries.iter().enumerate() {
                    let (prefix, connectors) = prefixes.get(i).cloned().unwrap_or_default();
                    let pad = if graph {
                        format!("{prefix} ")
                    } else {
                        String::new()
                    };
                    if oneline || graph {
                        println!(
                            "{pad}{} {}{}",
                            e.id.short(),
                            e.commit.summary(),
                            e.commit
                                .agent
                                .as_ref()
                                .map(|a| format!(" [{}]", a.id))
                                .unwrap_or_default()
                        );
                    } else {
                        print_commit_header(&e.id, &e.commit);
                        println!();
                    }
                    for c in connectors {
                        println!("{c}");
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
                    let changes =
                        diff::diff_trees(repo.store(), parent_tree.as_ref(), Some(&c.tree))?;
                    let patch = if no_patch {
                        None
                    } else {
                        Some(render_patch(&repo, &changes, 3)?)
                    };
                    emit(
                        json,
                        &serde_json::json!({ "id": id, "commit": c, "changes": changes, "patch": patch }),
                        || {
                            print_commit_header(&id, &c);
                            println!();
                            if let Some(p) = patch {
                                print!("{p}");
                            } else {
                                for ch in &changes {
                                    println!("{} {}", ch.kind.letter(), ch.path);
                                }
                            }
                        },
                    );
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
                        emit(
                            json,
                            &serde_json::json!({ "id": id, "size": b.len(), "binary": diff::is_binary(&b), "content": String::from_utf8_lossy(&b) }),
                            || {},
                        );
                    } else {
                        use std::io::Write;
                        std::io::stdout().write_all(&b)?;
                    }
                }
            }
        }
        Cmd::Diff {
            staged,
            revs,
            stat,
            name_only,
            unified,
        } => {
            let repo = open(&cli.dir)?;
            let (old, new): (BTreeMap<String, diff::Side>, BTreeMap<String, diff::Side>) =
                match revs.len() {
                    0 if staged => {
                        let head = repo.head_tree()?;
                        (
                            diff::tree_to_sides(repo.store(), head.as_ref())?,
                            index_sides(&repo)?,
                        )
                    }
                    0 => (index_sides(&repo)?, worktree_sides(&repo)?),
                    1 => {
                        let t = repo.resolve_commit(&revs[0])?.1.tree;
                        (
                            diff::tree_to_sides(repo.store(), Some(&t))?,
                            worktree_sides(&repo)?,
                        )
                    }
                    2 => {
                        let a = repo.resolve_commit(&revs[0])?.1.tree;
                        let b = repo.resolve_commit(&revs[1])?.1.tree;
                        (
                            diff::tree_to_sides(repo.store(), Some(&a))?,
                            diff::tree_to_sides(repo.store(), Some(&b))?,
                        )
                    }
                    _ => bail!("diff takes at most two revisions"),
                };
            let changes = diff::detect_renames(repo.store(), diff::diff_maps(&old, &new), 50);
            let from_worktree = revs.len() <= 1 && !staged;
            let patch = if name_only || stat {
                None
            } else {
                Some(render_patch_sides(&repo, &changes, unified, from_worktree)?)
            };
            let stats: Vec<(String, usize, usize)> = if stat || json {
                changes
                    .iter()
                    .map(|c| {
                        let (a, d) = blob_stats(&repo, c, from_worktree).unwrap_or((0, 0));
                        (c.path.clone(), a, d)
                    })
                    .collect()
            } else {
                vec![]
            };
            emit(
                json,
                &serde_json::json!({ "changes": changes, "stats": stats.iter().map(|(p,a,d)| serde_json::json!({"path":p,"added":a,"removed":d})).collect::<Vec<_>>(), "patch": patch }),
                || {
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
                        println!(
                            " {} file(s) changed, {ta} insertion(s), {td} deletion(s)",
                            changes.len()
                        );
                    } else if let Some(p) = patch {
                        print!("{p}");
                    }
                },
            );
        }
        Cmd::Branch {
            name,
            start,
            delete,
            force_delete,
        } => {
            let repo = open(&cli.dir)?;
            match name {
                None => {
                    let list = ops::list_branches(&repo)?;
                    emit(json, &list, || {
                        for b in &list {
                            println!(
                                "{} {} {}",
                                if b.current { "*" } else { " " },
                                b.name,
                                b.id.short()
                            );
                        }
                    });
                }
                Some(n) if delete || force_delete => {
                    let id = ops::delete_branch(&repo, &n, force_delete)?;
                    emit(
                        json,
                        &serde_json::json!({ "deleted": n, "was": id }),
                        || println!("Deleted branch {n} (was {})", id.short()),
                    );
                }
                Some(n) => {
                    let b = ops::create_branch(&repo, &n, start.as_deref())?;
                    emit(json, &b, || {
                        println!("Created branch {} at {}", b.name, b.id.short())
                    });
                }
            }
        }
        Cmd::Switch {
            target,
            create,
            force,
        } => {
            let repo = open(&cli.dir)?;
            let r = ops::switch(&repo, &target, create, force)?;
            emit(json, &r, || match &r.head {
                ops::Head2::Branch { name } => println!("Switched to branch '{name}'"),
                ops::Head2::Detached => println!("HEAD is now at {}", r.id.short()),
            });
        }
        Cmd::Merge {
            rev,
            message,
            intent,
            no_ff,
            abort,
            force,
            sign,
            agent,
        } => {
            let repo = open(&cli.dir)?;
            if abort {
                ops::merge_abort(&repo)?;
                emit(json, &serde_json::json!({ "aborted": true }), || {
                    println!("Merge aborted")
                });
                return Ok(());
            }
            let rev = rev.context("merge needs a revision")?;
            let sign_with = signing_key(sign.as_deref(), agent.agent.as_deref())?;
            let r = ops::merge(
                &repo,
                &rev,
                MergeOptions {
                    message,
                    intent,
                    agent: agent.info(),
                    no_ff,
                    force,
                    sign_with,
                },
            )?;
            let conflicted = matches!(r, MergeResult::Conflicts { .. });
            emit(json, &r, || match &r {
                MergeResult::AlreadyUpToDate { .. } => println!("Already up to date."),
                MergeResult::FastForward {
                    to, files_changed, ..
                } => println!("Fast-forward to {} ({files_changed} file(s))", to.short()),
                MergeResult::Merged {
                    commit,
                    auto_merged,
                    ..
                } => println!(
                    "Merge made commit {} ({} file(s) auto-merged)",
                    commit.short(),
                    auto_merged.len()
                ),
                MergeResult::Conflicts { conflicts, .. } => {
                    for c in conflicts {
                        println!("CONFLICT ({:?}): {}", c.kind, c.path);
                    }
                    println!("Automatic merge failed; fix conflicts, `agit add` them and commit.");
                }
            });
            if conflicted {
                if json {
                    // The result document already describes the conflicts.
                    std::process::exit(3);
                }
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
                    emit(
                        json,
                        &tags
                            .iter()
                            .map(|(n, id)| serde_json::json!({"name": n, "id": id}))
                            .collect::<Vec<_>>(),
                        || {
                            for (n, _) in &tags {
                                println!("{n}");
                            }
                        },
                    );
                }
                Some(n) if delete => {
                    let id = ops::delete_tag(&repo, &n)?;
                    emit(
                        json,
                        &serde_json::json!({ "deleted": n, "was": id }),
                        || println!("Deleted tag {n}"),
                    );
                }
                Some(n) => {
                    let id = ops::create_tag(&repo, &n, rev.as_deref())?;
                    emit(json, &serde_json::json!({ "name": n, "id": id }), || {
                        println!("Tagged {} as {n}", id.short())
                    });
                }
            }
        }
        Cmd::Lease { cmd } => run_lease(&cli.dir, json, cmd)?,
        Cmd::Remote { cmd } => crate::remote::run_remote(&open(&cli.dir)?, json, cmd)?,
        Cmd::Push {
            remote,
            branch,
            force,
            create,
            tags,
        } => crate::remote::run_push(&open(&cli.dir)?, json, &remote, branch, force, create, tags)?,
        Cmd::Fetch { remote, branches } => {
            crate::remote::run_fetch(&open(&cli.dir)?, json, &remote, &branches)?
        }
        Cmd::Pull {
            remote,
            no_ff,
            agent,
        } => crate::remote::run_pull(&open(&cli.dir)?, json, &remote, no_ff, agent.info())?,
        Cmd::Clone { url, dest } => crate::remote::run_clone(json, &cwd(&cli.dir)?, &url, dest)?,
        Cmd::Pr { remote, cmd, agent } => {
            crate::remote::run_pr(&open(&cli.dir)?, json, &remote, cmd, agent.agent.as_deref())?
        }
        Cmd::Know {
            remote,
            local,
            cmd,
            agent,
        } => crate::remote::run_know(
            &open(&cli.dir)?,
            json,
            &remote,
            cmd,
            agent.agent.as_deref(),
            local,
        )?,
        Cmd::Mcp { remote } => crate::mcp::serve(cwd(&cli.dir)?, remote)?,
        Cmd::Key { cmd } => run_key(&cli.dir, json, cmd)?,
        Cmd::Auth { url, remote, cmd } => {
            crate::auth::run_auth(&cli.dir, json, url.as_deref(), &remote, cmd)?
        }
        Cmd::Verify { rev } => {
            let repo = open(&cli.dir)?;
            let (id, c) = repo.resolve_commit(&rev)?;
            let status = agit_core::sign::verify_commit(&c);
            let ok = status.is_valid();
            emit(
                json,
                &serde_json::json!({ "id": id, "signature": status }),
                || match &status {
                    agit_core::sign::SignatureStatus::Unsigned => {
                        println!("{}: unsigned", id.short())
                    }
                    agit_core::sign::SignatureStatus::Valid { key_id, signer, .. } => {
                        println!("{}: valid signature by {signer} (key {key_id})", id.short())
                    }
                    agit_core::sign::SignatureStatus::Invalid { reason } => {
                        println!("{}: INVALID signature: {reason}", id.short())
                    }
                },
            );
            if !ok {
                if json {
                    std::process::exit(1);
                }
                bail!("commit {} has no valid signature", id.short());
            }
        }
        Cmd::Git { cmd } => match cmd {
            GitCmd::Import {
                source,
                into,
                branches,
                no_tags,
                no_checkout,
            } => {
                let base = cwd(&cli.dir)?;
                let source = if source.is_absolute() {
                    source
                } else {
                    base.join(source)
                };
                let repo = match into {
                    Some(dir) => {
                        let dir = if dir.is_absolute() {
                            dir
                        } else {
                            base.join(dir)
                        };
                        if dir.join(agit_core::repo::AGIT_DIR).exists() {
                            Repository::discover(&dir)?
                        } else {
                            Repository::init(&dir)?
                        }
                    }
                    None => open(&cli.dir)?,
                };
                let report = agit_git_bridge::import(
                    &source,
                    &repo,
                    &agit_git_bridge::ImportOptions {
                        branches,
                        tags: !no_tags,
                        checkout: !no_checkout,
                        verbose: !json,
                    },
                )?;
                emit(json, &report, || {
                    println!(
                        "imported {} commit(s), {} tree(s), {} blob(s) into {}",
                        report.commits,
                        report.trees,
                        report.blobs,
                        repo.work_dir().display()
                    );
                    println!(
                        "branches: {}  tags: {}",
                        report.branches.join(", "),
                        report.tags.join(", ")
                    );
                });
            }
            GitCmd::Export {
                target,
                branches,
                no_tags,
            } => {
                let base = cwd(&cli.dir)?;
                let target = if target.is_absolute() {
                    target
                } else {
                    base.join(target)
                };
                let repo = open(&cli.dir)?;
                let report = agit_git_bridge::export(
                    &repo,
                    &target,
                    &agit_git_bridge::ExportOptions {
                        branches,
                        tags: !no_tags,
                    },
                )?;
                emit(json, &report, || {
                    println!(
                        "exported {} new commit(s) to {} (branches: {})",
                        report.commits,
                        target.display(),
                        report.branches.join(", ")
                    );
                });
            }
        },
        Cmd::Submodule { cmd } => {
            let repo = open(&cli.dir)?;
            let cwd = cwd(&cli.dir)?;
            match cmd {
                SubmoduleCmd::Add { url, path, branch } => {
                    let rel = repo.rel_path(&cwd, std::path::Path::new(&path))?;
                    let m = agit_protocol::submodule::add(&repo, &url, &rel, branch.as_deref())?;
                    emit(json, &m, || {
                        println!("added submodule {} from {}", m.path, m.url)
                    });
                }
                SubmoduleCmd::Update { paths } => {
                    let rels = rels(
                        &repo,
                        &cwd,
                        &paths.iter().map(PathBuf::from).collect::<Vec<_>>(),
                    )?;
                    let r = agit_protocol::submodule::update(
                        &repo,
                        if rels.is_empty() { None } else { Some(&rels) },
                    )?;
                    emit(json, &r, || {
                        for u in &r {
                            println!(
                                "{:<12} {} {}",
                                u.action,
                                u.path,
                                u.commit.map(|c| c.short()).unwrap_or_default()
                            );
                        }
                    });
                }
                SubmoduleCmd::Status => {
                    let r = agit_protocol::submodule::status(&repo)?;
                    emit(json, &r, || {
                        for s in &r {
                            println!(
                                "{:<9} {} pinned={} actual={}",
                                s.state,
                                s.path,
                                s.pinned.map(|c| c.short()).unwrap_or_else(|| "-".into()),
                                s.actual.map(|c| c.short()).unwrap_or_else(|| "-".into())
                            );
                        }
                    });
                }
                SubmoduleCmd::Remove { path, delete } => {
                    let rel = repo.rel_path(&cwd, std::path::Path::new(&path))?;
                    agit_protocol::submodule::remove(&repo, &rel, delete)?;
                    emit(json, &serde_json::json!({ "removed": rel }), || {
                        println!("removed submodule {rel}")
                    });
                }
            }
        }
        Cmd::Ws { cmd } => run_ws(&cli.dir, json, cmd)?,
        Cmd::Events {
            since,
            follow,
            kind,
        } => {
            let repo = open(&cli.dir)?;
            let mut cursor = since;
            loop {
                let events = repo.events().read_after(cursor)?;
                for ev in events {
                    cursor = ev.seq;
                    if kind
                        .as_ref()
                        .is_some_and(|k| !ev.kind.starts_with(k.as_str()))
                    {
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
            emit(json, &serde_json::json!({ "rev": rev, "id": id }), || {
                println!("{id}")
            });
        }
        Cmd::CatFile { id, type_only } => {
            let repo = open(&cli.dir)?;
            let oid: ObjectId = match id.parse() {
                Ok(o) => o,
                Err(_) => repo
                    .resolve(&id)
                    .or_else(|_| repo.store().resolve_prefix(&id))?,
            };
            let kind = repo.store().kind(&oid)?;
            if type_only {
                emit(
                    json,
                    &serde_json::json!({ "id": oid, "type": kind }),
                    || println!("{kind}"),
                );
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
                    emit(json, &c, || {
                        println!("{}", serde_json::to_string_pretty(&c).unwrap())
                    });
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
                (None, _) => emit(json, &cfg, || {
                    println!("{}", serde_json::to_string_pretty(&cfg).unwrap())
                }),
                (Some(k), None) => {
                    let v = match k.as_str() {
                        "user.name" => serde_json::json!(cfg.user.name),
                        "user.email" => serde_json::json!(cfg.user.email),
                        "default_branch" => serde_json::json!(cfg.default_branch),
                        _ => bail!("unknown config key {k}"),
                    };
                    emit(json, &serde_json::json!({ "key": k, "value": v }), || {
                        println!("{}", v.as_str().unwrap_or(""))
                    });
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
        LeaseCmd::Claim {
            paths,
            holder,
            ttl,
            note,
            shared,
        } => {
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
                mode: if shared {
                    LeaseMode::Shared
                } else {
                    LeaseMode::Exclusive
                },
                ttl: Some(parse_duration(&ttl)?),
                note,
                workspace: Some(repo.workspace().to_string()),
            })?;
            repo.events().append(
                "lease.claim",
                Some(repo.workspace()),
                Some(&holder),
                serde_json::to_value(&lease)?,
            )?;
            emit(json, &lease, || {
                println!(
                    "lease {} granted to {} for {}",
                    lease.id,
                    lease.holder,
                    lease.paths.join(", ")
                )
            });
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
                repo.events().append(
                    "lease.release",
                    Some(repo.workspace()),
                    Some(&l.holder),
                    serde_json::json!({ "id": l.id, "paths": l.paths }),
                )?;
            }
            emit(json, &released, || {
                println!("released {} lease(s)", released.len())
            });
        }
        LeaseCmd::Renew { id, ttl } => {
            let l = leases.renew(&id, parse_duration(&ttl)?)?;
            repo.events().append(
                "lease.renew",
                Some(repo.workspace()),
                Some(&l.holder),
                serde_json::json!({ "id": l.id, "expires_ms": l.expires_ms }),
            )?;
            emit(json, &l, || {
                println!(
                    "lease {} renewed until {}",
                    l.id,
                    l.expires_ms.map(fmt_time).unwrap_or_default()
                )
            });
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
                .map(|p| {
                    if p.contains(['*', '?', '[', '{']) {
                        Ok(p.clone())
                    } else {
                        Ok(repo.rel_path(&cwd, std::path::Path::new(p))?)
                    }
                })
                .collect::<Result<_>>()?;
            ops::enforce_leases(leases, &holder, &paths)?;
            emit(
                json,
                &serde_json::json!({ "allowed": true, "holder": holder, "paths": paths }),
                || println!("ok"),
            );
        }
    }
    Ok(())
}

fn run_ws(dir: &Option<PathBuf>, json: bool, cmd: WsCmd) -> Result<()> {
    let repo = open(dir)?;
    match cmd {
        WsCmd::New {
            name,
            path,
            from,
            agent,
        } => {
            let start = match from {
                Some(r) => Some(repo.resolve(&r)?),
                None => None,
            };
            let path = match path {
                Some(p) => Some(if p.is_absolute() {
                    p
                } else {
                    cwd(dir)?.join(p)
                }),
                None => None,
            };
            let (_ws, meta) =
                repo.create_workspace(&name, path.as_deref(), start, agent.as_deref())?;
            emit(json, &meta, || {
                println!(
                    "workspace {} created at {} (branch ws/{})",
                    meta.name,
                    meta.path.display(),
                    meta.name
                )
            });
        }
        WsCmd::List => {
            let list = repo.list_workspaces()?;
            emit(json, &list, || {
                for w in &list {
                    println!(
                        "{} {}{}",
                        if w.name == repo.workspace() { "*" } else { " " },
                        w.name,
                        w.agent
                            .as_ref()
                            .map(|a| format!(" ({a})"))
                            .unwrap_or_default()
                    );
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
        let value =
            serde_json::from_str(v).unwrap_or_else(|_| serde_json::Value::String(v.to_string()));
        out.insert(k.to_string(), value);
    }
    Ok(out)
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

fn render_patch(repo: &Repository, changes: &[diff::Change], ctx: usize) -> Result<String> {
    render_patch_sides(repo, changes, ctx, false)
}

fn render_patch_sides(
    repo: &Repository,
    changes: &[diff::Change],
    ctx: usize,
    from_worktree: bool,
) -> Result<String> {
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
    println!(
        "commit {id}{}",
        if c.checkpoint { " (checkpoint)" } else { "" }
    );
    if c.parents.len() > 1 {
        println!(
            "Merge:  {}",
            c.parents
                .iter()
                .map(|p| p.short())
                .collect::<Vec<_>>()
                .join(" ")
        );
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
        None => println!(
            "HEAD detached at {}",
            st.head.map(|h| h.short()).unwrap_or_default()
        ),
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
        ChangeKind::Renamed => "renamed:",
    }
}

/// Resolve which key (if any) to sign with.
///
/// `--sign` (empty = default name) forces signing. Without it, signing is
/// automatic when `AGIT_SIGN` is `1`/`auto` (default `auto`) and a key named
/// after the agent (or `AGIT_KEY`) exists.
pub fn signing_key(explicit: Option<&str>, agent: Option<&str>) -> Result<Option<String>> {
    let default_name = std::env::var("AGIT_KEY")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| agent.map(str::to_string))
        .unwrap_or_else(|| "default".into());
    match explicit {
        Some("") => Ok(Some(default_name)),
        Some(name) => Ok(Some(name.to_string())),
        None => {
            let mode = std::env::var("AGIT_SIGN")
                .unwrap_or_else(|_| "auto".into())
                .to_lowercase();
            match mode.as_str() {
                "0" | "off" | "false" | "no" => Ok(None),
                "1" | "on" | "true" | "yes" => Ok(Some(default_name)),
                _ => Ok(if agit_core::sign::Keypair::exists(&default_name) {
                    Some(default_name)
                } else {
                    None
                }),
            }
        }
    }
}

fn run_key(dir: &Option<PathBuf>, json: bool, cmd: KeyCmd) -> Result<()> {
    use agit_core::sign::Keypair;
    let default_name = || {
        std::env::var("AGIT_KEY")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| std::env::var("AGIT_AGENT").ok())
            .unwrap_or_else(|| "default".into())
    };
    match cmd {
        KeyCmd::Generate {
            name,
            register,
            remote,
            agent,
        } => {
            let name = name.unwrap_or_else(default_name);
            let kp = Keypair::generate(&name);
            let path = kp.save()?;
            let info = kp.info();
            let mut registered = None;
            if register {
                let repo = open(dir)?;
                let (client, repo_name) = agit_protocol::sync::client_for(&repo, &remote)?;
                registered = Some(client.register_key(
                    &repo_name,
                    agent.as_deref().unwrap_or(&name),
                    &info.public_key,
                )?);
            }
            emit(
                json,
                &serde_json::json!({ "key": info, "path": path, "registered": registered }),
                || {
                    println!(
                        "generated key {} ({}) at {}",
                        info.name,
                        info.key_id,
                        path.display()
                    );
                    if let Some(r) = &registered {
                        println!("registered for agent {} on {remote}", r.agent);
                    }
                },
            );
        }
        KeyCmd::List => {
            let list = Keypair::list()?;
            emit(json, &list, || {
                for k in &list {
                    println!("{:<20} {} {}", k.name, k.key_id, k.public_key);
                }
            });
        }
        KeyCmd::Show { name } => {
            let kp = Keypair::load(&name.unwrap_or_else(default_name))?;
            let info = kp.info();
            emit(json, &info, || {
                println!("{} {} {}", agit_core::sign::ALG, info.public_key, info.name)
            });
        }
        KeyCmd::Register {
            name,
            remote,
            agent,
        } => {
            let name = name.unwrap_or_else(default_name);
            let kp = Keypair::load(&name)?;
            let repo = open(dir)?;
            let (client, repo_name) = agit_protocol::sync::client_for(&repo, &remote)?;
            let r = client.register_key(
                &repo_name,
                agent.as_deref().unwrap_or(&name),
                &kp.public_key_hex(),
            )?;
            emit(json, &r, || {
                println!(
                    "registered key {} for agent {} on {remote}",
                    r.key_id, r.agent
                )
            });
        }
        KeyCmd::Delete { name } => {
            Keypair::delete(&name)?;
            emit(json, &serde_json::json!({ "deleted": name }), || {
                println!("deleted key {name}")
            });
        }
    }
    Ok(())
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n.saturating_sub(1)).collect::<String>() + "…"
    }
}

fn print_apply(json: bool, r: &agit_core::history::ApplyResult, verb: &str) -> Result<()> {
    use agit_core::history::ApplyResult;
    emit(json, r, || match r {
        ApplyResult::Committed {
            commit,
            files_changed,
        } => println!("{verb} as {} ({files_changed} file(s))", commit.short()),
        ApplyResult::Conflicts { conflicts, .. } => {
            for c in conflicts {
                println!("CONFLICT ({:?}): {}", c.kind, c.path);
            }
            println!("Resolve the conflicts, `agit add` the files and commit.");
        }
    });
    if let ApplyResult::Conflicts { conflicts, .. } = r {
        if json {
            std::process::exit(3);
        }
        return Err(agit_core::Error::MergeConflicts(conflicts.len()).into());
    }
    Ok(())
}
