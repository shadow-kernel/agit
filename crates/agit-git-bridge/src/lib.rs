//! Import git history into an agit repository.
//!
//! Uses the `git` command line (`rev-list`, `cat-file --batch`,
//! `for-each-ref`) so no libgit2 build is needed. Every imported commit keeps
//! its original SHA in `meta["git.sha"]`, trailers such as `Intent:`,
//! `Task:`, `Agent:` and `Model:` are lifted into the structured fields, and a
//! mapping table `git-map.json` (git sha -> agit id) is written into `.agit/`
//! so later incremental imports and an eventual export can reuse it.

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use agit_core::index::TreeBuilder;
use agit_core::object::{EntryMode, ObjectKind};
use agit_core::refs::{Expect, HEADS_PREFIX, TAGS_PREFIX};
use agit_core::{AgentInfo, Commit, Error, ObjectId, Repository, Result, Signature};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ImportReport {
    pub commits: usize,
    pub trees: usize,
    pub blobs: usize,
    pub branches: Vec<String>,
    pub tags: Vec<String>,
    pub skipped_commits: usize,
}

#[derive(Debug, Clone, Default)]
pub struct ImportOptions {
    /// Only these branches (short names); all local branches when empty.
    pub branches: Vec<String>,
    /// Import tags too.
    pub tags: bool,
    /// Check out the default branch into the working tree afterwards.
    pub checkout: bool,
    /// Emit progress on stderr.
    pub verbose: bool,
}

/// Streaming `git cat-file --batch` session.
struct CatFile {
    child: Child,
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
}

impl CatFile {
    fn open(git_dir: &Path) -> Result<CatFile> {
        let mut child = Command::new("git")
            .arg("-C")
            .arg(git_dir)
            .args(["cat-file", "--batch"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| Error::Invalid(format!("cannot run git: {e}")))?;
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Ok(CatFile {
            child,
            stdin,
            stdout,
        })
    }

    /// Returns (type, bytes).
    fn get(&mut self, sha: &str) -> Result<(String, Vec<u8>)> {
        writeln!(self.stdin, "{sha}")?;
        self.stdin.flush()?;
        let mut header = String::new();
        self.stdout.read_line(&mut header)?;
        let parts: Vec<&str> = header.trim_end().split(' ').collect();
        if parts.len() < 3 {
            return Err(Error::Invalid(format!(
                "git object {sha} not found ({})",
                header.trim()
            )));
        }
        let size: usize = parts[2]
            .parse()
            .map_err(|_| Error::Invalid(format!("bad cat-file header {header:?}")))?;
        let mut buf = vec![0u8; size];
        self.stdout.read_exact(&mut buf)?;
        let mut nl = [0u8; 1];
        self.stdout.read_exact(&mut nl)?;
        Ok((parts[1].to_string(), buf))
    }
}

impl Drop for CatFile {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| Error::Invalid(format!("cannot run git: {e}")))?;
    if !out.status.success() {
        return Err(Error::Invalid(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

struct GitCommit {
    tree: String,
    parents: Vec<String>,
    author: Signature,
    time_ms: i64,
    message: String,
}

fn parse_commit(raw: &[u8]) -> Result<GitCommit> {
    let text = String::from_utf8_lossy(raw);
    let (headers, message) = text.split_once("\n\n").unwrap_or((&text, ""));
    let mut tree = String::new();
    let mut parents = Vec::new();
    let mut author = Signature {
        name: "unknown".into(),
        email: String::new(),
    };
    let mut time_ms = 0;
    for line in headers.lines() {
        if let Some(t) = line.strip_prefix("tree ") {
            tree = t.to_string();
        } else if let Some(p) = line.strip_prefix("parent ") {
            parents.push(p.to_string());
        } else if let Some(a) = line.strip_prefix("author ") {
            // "Name <email> 1700000000 +0100"
            if let Some((name_email, rest)) = a.rsplit_once('>') {
                let (name, email) = name_email.split_once('<').unwrap_or((name_email, ""));
                author = Signature {
                    name: name.trim().to_string(),
                    email: email.trim().to_string(),
                };
                let ts = rest.trim().split(' ').next().unwrap_or("0");
                time_ms = ts.parse::<i64>().unwrap_or(0) * 1000;
            }
        }
    }
    Ok(GitCommit {
        tree,
        parents,
        author,
        time_ms,
        message: message.to_string(),
    })
}

/// Lift `Key: value` trailers from the message into structured fields.
fn split_trailers(message: &str) -> (String, BTreeMap<String, String>) {
    let mut trailers = BTreeMap::new();
    let lines: Vec<&str> = message.trim_end().lines().collect();
    let mut cut = lines.len();
    for (i, line) in lines.iter().enumerate().rev() {
        if line.trim().is_empty() {
            if i == lines.len() - 1 {
                continue;
            }
            break;
        }
        match line.split_once(": ") {
            Some((k, v))
                if !k.contains(' ') && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') =>
            {
                trailers.insert(k.to_ascii_lowercase(), v.trim().to_string());
                cut = i;
            }
            _ => break,
        }
    }
    let body = lines[..cut].join("\n").trim_end().to_string();
    (body, trailers)
}

struct Importer<'a> {
    repo: &'a Repository,
    cat: CatFile,
    blob_map: HashMap<String, (ObjectId, u64)>,
    tree_map: HashMap<String, ObjectId>,
    commit_map: HashMap<String, ObjectId>,
    report: ImportReport,
    verbose: bool,
}

impl Importer<'_> {
    fn import_blob(&mut self, sha: &str) -> Result<(ObjectId, u64)> {
        if let Some(x) = self.blob_map.get(sha) {
            return Ok(*x);
        }
        let (_, bytes) = self.cat.get(sha)?;
        let id = self.repo.store().write(ObjectKind::Blob, &bytes)?;
        self.blob_map
            .insert(sha.to_string(), (id, bytes.len() as u64));
        self.report.blobs += 1;
        Ok((id, bytes.len() as u64))
    }

    fn import_tree(&mut self, sha: &str, prefix: &str, builder: &mut TreeBuilder) -> Result<()> {
        let (_, raw) = self.cat.get(sha)?;
        let mut pos = 0;
        while pos < raw.len() {
            let sp = raw[pos..]
                .iter()
                .position(|&b| b == b' ')
                .ok_or_else(|| Error::Invalid("bad git tree".into()))?
                + pos;
            let mode = std::str::from_utf8(&raw[pos..sp]).unwrap_or("100644");
            let nul = raw[sp..]
                .iter()
                .position(|&b| b == 0)
                .ok_or_else(|| Error::Invalid("bad git tree".into()))?
                + sp;
            let name = String::from_utf8_lossy(&raw[sp + 1..nul]).to_string();
            let sha_bytes = &raw[nul + 1..nul + 21];
            let entry_sha = hex::encode(sha_bytes);
            pos = nul + 21;
            let path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            match mode {
                "40000" | "040000" => self.import_tree(&entry_sha, &path, builder)?,
                "160000" => {
                    // Submodule: keep the pin as a (fake) id derived from the sha.
                    let mut bytes = [0u8; 32];
                    bytes[..20].copy_from_slice(sha_bytes);
                    builder.insert(&path, EntryMode::Submodule, ObjectId::from_bytes(bytes), 0);
                }
                "120000" => {
                    let (id, size) = self.import_blob(&entry_sha)?;
                    builder.insert(&path, EntryMode::Symlink, id, size);
                }
                "100755" => {
                    let (id, size) = self.import_blob(&entry_sha)?;
                    builder.insert(&path, EntryMode::Executable, id, size);
                }
                _ => {
                    let (id, size) = self.import_blob(&entry_sha)?;
                    builder.insert(&path, EntryMode::File, id, size);
                }
            }
        }
        Ok(())
    }

    fn import_root_tree(&mut self, sha: &str) -> Result<ObjectId> {
        if let Some(id) = self.tree_map.get(sha) {
            return Ok(*id);
        }
        let mut builder = TreeBuilder::default();
        self.import_tree(sha, "", &mut builder)?;
        let id = builder.write(self.repo.store())?;
        self.tree_map.insert(sha.to_string(), id);
        self.report.trees += 1;
        Ok(id)
    }

    fn import_commit(&mut self, sha: &str) -> Result<ObjectId> {
        if let Some(id) = self.commit_map.get(sha) {
            return Ok(*id);
        }
        let (kind, raw) = self.cat.get(sha)?;
        if kind != "commit" {
            return Err(Error::Invalid(format!("{sha} is a {kind}, not a commit")));
        }
        let gc = parse_commit(&raw)?;
        let tree = self.import_root_tree(&gc.tree)?;
        let mut parents = Vec::new();
        for p in &gc.parents {
            match self.commit_map.get(p) {
                Some(id) => parents.push(*id),
                None => {
                    // rev-list is topological, so this only happens for shallow clones.
                    self.report.skipped_commits += 1;
                }
            }
        }
        let (message, trailers) = split_trailers(&gc.message);
        let mut meta = BTreeMap::new();
        meta.insert("git.sha".to_string(), serde_json::json!(sha));
        for (k, v) in &trailers {
            if !matches!(k.as_str(), "intent" | "task" | "agent" | "model" | "run-id") {
                meta.insert(format!("git.trailer.{k}"), serde_json::json!(v));
            }
        }
        let agent = trailers.get("agent").map(|a| AgentInfo {
            id: a.clone(),
            model: trailers.get("model").cloned(),
            run_id: trailers.get("run-id").cloned(),
            on_behalf_of: None,
        });
        let commit = Commit {
            tree,
            parents,
            author: gc.author,
            time_ms: gc.time_ms,
            message,
            intent: trailers.get("intent").cloned(),
            task: trailers.get("task").cloned(),
            agent,
            checkpoint: false,
            meta,
        };
        let id = self.repo.store().write_commit(&commit)?;
        self.commit_map.insert(sha.to_string(), id);
        self.report.commits += 1;
        if self.verbose && self.report.commits.is_multiple_of(100) {
            eprintln!(
                "imported {} commits, {} blobs",
                self.report.commits, self.report.blobs
            );
        }
        Ok(id)
    }
}

fn map_path(repo: &Repository) -> PathBuf {
    repo.common_dir().join("git-map.json")
}

fn load_map(repo: &Repository) -> HashMap<String, ObjectId> {
    std::fs::read(map_path(repo))
        .ok()
        .and_then(|b| serde_json::from_slice::<HashMap<String, ObjectId>>(&b).ok())
        .unwrap_or_default()
}

/// Import `git_dir` (a working copy or bare repository) into `repo`.
pub fn import(git_dir: &Path, repo: &Repository, opts: &ImportOptions) -> Result<ImportReport> {
    git(git_dir, &["rev-parse", "--git-dir"])?;
    let mut imp = Importer {
        repo,
        cat: CatFile::open(git_dir)?,
        blob_map: HashMap::new(),
        tree_map: HashMap::new(),
        commit_map: load_map(repo),
        report: ImportReport::default(),
        verbose: opts.verbose,
    };

    // Branches to import.
    let refs_out = git(
        git_dir,
        &[
            "for-each-ref",
            "--format=%(refname) %(objectname) %(*objectname)",
            "refs/heads",
            "refs/tags",
        ],
    )?;
    let mut branches: Vec<(String, String)> = Vec::new();
    let mut tags: Vec<(String, String)> = Vec::new();
    for line in refs_out.lines() {
        let parts: Vec<&str> = line.split(' ').collect();
        if parts.len() < 2 {
            continue;
        }
        let (name, sha) = (parts[0], parts[1]);
        let peeled = parts
            .get(2)
            .filter(|s| !s.is_empty())
            .copied()
            .unwrap_or(sha);
        if let Some(short) = name.strip_prefix("refs/heads/") {
            if opts.branches.is_empty() || opts.branches.iter().any(|b| b == short) {
                branches.push((short.to_string(), sha.to_string()));
            }
        } else if let Some(short) = name.strip_prefix("refs/tags/") {
            if opts.tags {
                tags.push((short.to_string(), peeled.to_string()));
            }
        }
    }
    if branches.is_empty() {
        return Err(Error::Invalid("no branches to import".into()));
    }

    // Topological order over everything reachable from the selected tips.
    let mut args: Vec<String> = vec!["rev-list".into(), "--reverse".into(), "--topo-order".into()];
    for (_, sha) in branches.iter().chain(tags.iter()) {
        args.push(sha.clone());
    }
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let list = git(git_dir, &arg_refs)?;
    for sha in list.lines() {
        imp.import_commit(sha.trim())?;
    }

    // Refs.
    for (short, sha) in &branches {
        let id = imp.commit_map[sha];
        repo.refs()
            .update(&format!("{HEADS_PREFIX}{short}"), Expect::Any, Some(id))?;
        imp.report.branches.push(short.clone());
    }
    for (short, sha) in &tags {
        if let Some(id) = imp.commit_map.get(sha) {
            repo.refs()
                .update(&format!("{TAGS_PREFIX}{short}"), Expect::Any, Some(*id))?;
            imp.report.tags.push(short.clone());
        }
    }
    agit_core::refs::write_atomic(&map_path(repo), &serde_json::to_vec(&imp.commit_map)?)?;

    // HEAD: git's current branch if imported, else the first branch.
    let head_branch = git(git_dir, &["symbolic-ref", "--short", "-q", "HEAD"])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|b| branches.iter().any(|(n, _)| n == b))
        .unwrap_or_else(|| branches[0].0.clone());
    repo.set_head(&agit_core::Head::Branch(head_branch.clone()))?;
    let mut cfg = repo.config()?;
    cfg.default_branch = head_branch;
    repo.save_config(&cfg)?;
    if opts.checkout {
        if let Some(tree) = repo.head_tree()? {
            agit_core::worktree::checkout_tree(repo, None, &tree, true)?;
        }
    }
    repo.events().append(
        "git.import",
        Some(repo.workspace()),
        None,
        serde_json::json!({ "source": git_dir, "commits": imp.report.commits, "branches": imp.report.branches }),
    )?;
    Ok(imp.report)
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExportReport {
    pub commits: usize,
    pub reused: usize,
    pub branches: Vec<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ExportOptions {
    /// Only these branches (short names); all when empty.
    pub branches: Vec<String>,
    pub tags: bool,
}

fn fi_data(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(format!("data {}\n", bytes.len()).as_bytes());
    out.extend_from_slice(bytes);
    out.push(b'\n');
}

fn quote_path(p: &str) -> String {
    if p.contains(['"', '\n', '\\']) || p.starts_with('"') {
        format!(
            "\"{}\"",
            p.replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n")
        )
    } else {
        p.to_string()
    }
}

fn commit_message_with_trailers(c: &Commit) -> String {
    let mut msg = c.message.trim_end().to_string();
    let mut trailers: Vec<String> = Vec::new();
    if let Some(i) = &c.intent {
        trailers.push(format!("Intent: {i}"));
    }
    if let Some(t) = &c.task {
        trailers.push(format!("Task: {t}"));
    }
    if let Some(a) = &c.agent {
        trailers.push(format!("Agent: {}", a.id));
        if let Some(m) = &a.model {
            trailers.push(format!("Model: {m}"));
        }
        if let Some(r) = &a.run_id {
            trailers.push(format!("Run-Id: {r}"));
        }
    }
    for (k, v) in &c.meta {
        if let Some(name) = k.strip_prefix("git.trailer.") {
            if let Some(s) = v.as_str() {
                let mut chars = name.chars();
                let cap = match chars.next() {
                    Some(f) => f.to_ascii_uppercase().to_string() + chars.as_str(),
                    None => String::new(),
                };
                trailers.push(format!("{cap}: {s}"));
            }
        }
    }
    if !trailers.is_empty() {
        msg.push_str("\n\n");
        msg.push_str(&trailers.join("\n"));
    }
    msg.push('\n');
    msg
}

/// Export agit history into `git_dir` (initialized when missing) using
/// `git fast-import`. Commits that originally came from git (known in the
/// mapping table and present in the target) are referenced, not recreated,
/// so repeated exports only add what is new.
pub fn export(repo: &Repository, git_dir: &Path, opts: &ExportOptions) -> Result<ExportReport> {
    if !git_dir.join(".git").exists() && git(git_dir, &["rev-parse", "--git-dir"]).is_err() {
        std::fs::create_dir_all(git_dir)?;
        let branch = repo.config()?.default_branch;
        git(git_dir, &["init", "-q", "-b", &branch])?;
    }
    let mut map: HashMap<String, ObjectId> = load_map(repo);
    let mut known: HashMap<ObjectId, String> =
        map.iter().map(|(sha, id)| (*id, sha.clone())).collect();
    // Only trust mappings whose git object exists in the target.
    known.retain(|_, sha| git(git_dir, &["cat-file", "-e", &format!("{sha}^{{commit}}")]).is_ok());

    let heads = repo.refs().list(HEADS_PREFIX)?;
    let branches: Vec<(String, ObjectId)> = heads
        .into_iter()
        .map(|(n, id)| (n[HEADS_PREFIX.len()..].to_string(), id))
        .filter(|(n, _)| opts.branches.is_empty() || opts.branches.iter().any(|b| b == n))
        .collect();
    let tags: Vec<(String, ObjectId)> = if opts.tags {
        repo.refs()
            .list(TAGS_PREFIX)?
            .into_iter()
            .map(|(n, id)| (n[TAGS_PREFIX.len()..].to_string(), id))
            .collect()
    } else {
        vec![]
    };
    if branches.is_empty() {
        return Err(Error::Invalid("no branches to export".into()));
    }

    // Topological order (parents first) of commits not yet in git.
    let store = repo.store();
    let mut order: Vec<ObjectId> = Vec::new();
    let mut seen: std::collections::HashSet<ObjectId> = std::collections::HashSet::new();
    let mut stack: Vec<(ObjectId, bool)> = branches
        .iter()
        .chain(tags.iter())
        .map(|(_, id)| (*id, false))
        .collect();
    while let Some((id, expanded)) = stack.pop() {
        if known.contains_key(&id) {
            continue;
        }
        if expanded {
            if seen.insert(id) {
                order.push(id);
            }
            continue;
        }
        if seen.contains(&id) {
            continue;
        }
        stack.push((id, true));
        for p in store.read_commit(&id)?.parents {
            if !seen.contains(&p) && !known.contains_key(&p) {
                stack.push((p, false));
            }
        }
    }

    let mut stream: Vec<u8> = Vec::new();
    let mut marks: HashMap<ObjectId, usize> = HashMap::new();
    let mut blob_marks: HashMap<ObjectId, usize> = HashMap::new();
    let mut next_mark = 1usize;
    let mut report = ExportReport {
        reused: known.len().min(order.len() + known.len()),
        ..Default::default()
    };

    let mut blob_ref =
        |id: &ObjectId, stream: &mut Vec<u8>, next_mark: &mut usize| -> Result<String> {
            if let Some(m) = blob_marks.get(id) {
                return Ok(format!(":{m}"));
            }
            let bytes = store.read_blob(id)?;
            let m = *next_mark;
            *next_mark += 1;
            stream.extend_from_slice(format!("blob\nmark :{m}\n").as_bytes());
            fi_data(stream, &bytes);
            blob_marks.insert(*id, m);
            Ok(format!(":{m}"))
        };

    for (i, id) in order.iter().enumerate() {
        let c = store.read_commit(id)?;
        let first_parent = c.parents.first().copied();
        let parent_tree = match first_parent {
            Some(p) => Some(store.read_commit(&p)?.tree),
            None => None,
        };
        // Blobs must precede the commit command: emit them now and buffer
        // the file modifications.
        let mut lines: Vec<String> = Vec::new();
        for ch in agit_core::diff::diff_trees(store, parent_tree.as_ref(), Some(&c.tree))? {
            match &ch.new {
                None => lines.push(format!("D {}", quote_path(&ch.path))),
                Some(side) => {
                    let mode = match side.mode {
                        EntryMode::Executable => "100755",
                        EntryMode::Symlink => "120000",
                        EntryMode::Submodule => {
                            // Pins from an earlier import carry the git sha in the first 20 bytes.
                            let sha = hex::encode(&side.id.as_bytes()[..20]);
                            lines.push(format!("M 160000 {sha} {}", quote_path(&ch.path)));
                            continue;
                        }
                        _ => "100644",
                    };
                    let r = blob_ref(&side.id, &mut stream, &mut next_mark)?;
                    lines.push(format!("M {mode} {r} {}", quote_path(&ch.path)));
                }
            }
        }
        let m = next_mark;
        next_mark += 1;
        marks.insert(*id, m);
        let parent_ref = |p: &ObjectId| -> Option<String> {
            marks
                .get(p)
                .map(|m| format!(":{m}"))
                .or_else(|| known.get(p).cloned())
        };
        // Write to a temporary branch; real refs are set at the end.
        stream.extend_from_slice(format!("commit refs/agit-export/tmp\nmark :{m}\n").as_bytes());
        let email = if c.author.email.is_empty() {
            "unknown@agit"
        } else {
            &c.author.email
        };
        let secs = c.time_ms.div_euclid(1000);
        stream.extend_from_slice(
            format!("author {} <{}> {} +0000\n", c.author.name, email, secs).as_bytes(),
        );
        stream.extend_from_slice(
            format!("committer {} <{}> {} +0000\n", c.author.name, email, secs).as_bytes(),
        );
        fi_data(&mut stream, commit_message_with_trailers(&c).as_bytes());
        match first_parent.and_then(|p| parent_ref(&p)) {
            Some(r) => stream.extend_from_slice(format!("from {r}\n").as_bytes()),
            None => {
                let _ = i;
                stream.extend_from_slice(b"deleteall\n");
            }
        }
        for p in c.parents.iter().skip(1) {
            if let Some(r) = parent_ref(p) {
                stream.extend_from_slice(format!("merge {r}\n").as_bytes());
            }
        }
        for l in &lines {
            stream.extend_from_slice(l.as_bytes());
            stream.push(b'\n');
        }
        stream.push(b'\n');
        report.commits += 1;
    }
    // Refs.
    let reference = |id: &ObjectId| -> Option<String> {
        marks
            .get(id)
            .map(|m| format!(":{m}"))
            .or_else(|| known.get(id).cloned())
    };
    for (name, id) in &branches {
        if let Some(r) = reference(id) {
            stream.extend_from_slice(format!("reset refs/heads/{name}\nfrom {r}\n\n").as_bytes());
            report.branches.push(name.clone());
        }
    }
    for (name, id) in &tags {
        if let Some(r) = reference(id) {
            stream.extend_from_slice(format!("reset refs/tags/{name}\nfrom {r}\n\n").as_bytes());
            report.tags.push(name.clone());
        }
    }
    stream.extend_from_slice(b"reset refs/agit-export/tmp\n\n");

    // Run fast-import and read back marks to extend the mapping table.
    let marks_file = repo.common_dir().join("git-export-marks.txt");
    let mut child = Command::new("git")
        .arg("-C")
        .arg(git_dir)
        .args([
            "fast-import",
            "--quiet",
            "--force",
            &format!("--export-marks={}", marks_file.display()),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Invalid(format!("cannot run git fast-import: {e}")))?;
    child.stdin.take().unwrap().write_all(&stream)?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        return Err(Error::Invalid(format!(
            "git fast-import failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    let _ = git(git_dir, &["update-ref", "-d", "refs/agit-export/tmp"]);
    if let Ok(text) = std::fs::read_to_string(&marks_file) {
        let by_mark: HashMap<usize, ObjectId> = marks.iter().map(|(id, m)| (*m, *id)).collect();
        for line in text.lines() {
            if let Some((m, sha)) = line.split_once(' ') {
                if let Some(id) = m
                    .strip_prefix(':')
                    .and_then(|n| n.parse::<usize>().ok())
                    .and_then(|n| by_mark.get(&n))
                {
                    map.insert(sha.to_string(), *id);
                }
            }
        }
        let _ = std::fs::remove_file(&marks_file);
    }
    agit_core::refs::write_atomic(&map_path(repo), &serde_json::to_vec(&map)?)?;
    // Working tree of a non-bare target: keep HEAD's checkout in sync.
    if git_dir.join(".git").is_dir() {
        let _ = git(git_dir, &["reset", "-q", "--hard"]);
    }
    repo.events().append(
        "git.export",
        Some(repo.workspace()),
        None,
        serde_json::json!({ "target": git_dir, "commits": report.commits, "branches": report.branches }),
    )?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trailers() {
        let (body, t) = split_trailers(
            "Fix login\n\nLonger text.\n\nIntent: close SEC-1\nTask: SEC-1\nAgent: coder-1\n",
        );
        assert_eq!(body, "Fix login\n\nLonger text.");
        assert_eq!(t["intent"], "close SEC-1");
        assert_eq!(t["agent"], "coder-1");
        let (body, t) = split_trailers("Just a message\n");
        assert_eq!(body, "Just a message");
        assert!(t.is_empty());
    }
}
