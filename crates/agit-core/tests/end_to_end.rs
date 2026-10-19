//! End-to-end tests of the core operations on real temporary repositories.

use std::fs;
use std::path::Path;

use agit_core::lease::{LeaseMode, LeaseRequest};
use agit_core::ops::{self, CommitOptions, LogOptions, MergeOptions, MergeResult};
use agit_core::worktree;
use agit_core::{AgentInfo, Error, ObjectId, Repository};

fn write(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, content).unwrap();
}

fn commit_all(repo: &Repository, msg: &str, agent: Option<&str>) -> ops::CommitResult {
    worktree::add(repo, &[], false).unwrap();
    ops::commit(
        repo,
        CommitOptions {
            message: msg.into(),
            agent: agent.map(|a| AgentInfo {
                id: a.into(),
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .unwrap()
}

fn fresh() -> (tempfile::TempDir, Repository) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    let repo = Repository::init(&root).unwrap();
    (dir, repo)
}

#[test]
fn init_add_commit_status_log() {
    let (_d, repo) = fresh();
    let root = repo.work_dir().to_path_buf();
    write(&root, "README.md", "hello\n");
    write(&root, "src/main.rs", "fn main() {}\n");
    write(&root, "target/junk", "x");
    write(&root, ".agitignore", "target/\n");

    let st = worktree::status(&repo, true).unwrap();
    assert_eq!(
        st.untracked,
        vec![".agitignore", "README.md", "src/main.rs"]
    );

    let r = worktree::add(&repo, &[], false).unwrap();
    assert_eq!(r.added.len(), 3);

    let c = ops::commit(
        &repo,
        CommitOptions {
            message: "initial".into(),
            intent: Some("bootstrap".into()),
            task: Some("T-1".into()),
            agent: Some(AgentInfo {
                id: "agent-a".into(),
                model: Some("m".into()),
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(c.changes.len(), 3);
    assert_eq!(c.branch.as_deref(), Some("main"));

    let st = worktree::status(&repo, true).unwrap();
    assert!(st.is_clean() && st.untracked.is_empty());

    // second status must hit the stat cache and still be clean
    let st = worktree::status(&repo, true).unwrap();
    assert!(st.is_clean());

    // modify, see unstaged, then nothing-to-commit
    write(&root, "README.md", "hello world\n");
    let st = worktree::status(&repo, true).unwrap();
    assert_eq!(st.unstaged.len(), 1);
    assert!(matches!(
        ops::commit(
            &repo,
            CommitOptions {
                message: "x".into(),
                ..Default::default()
            }
        ),
        Err(Error::NothingToCommit)
    ));
    commit_all(&repo, "second", Some("agent-b"));

    let log = ops::log(
        &repo,
        &[repo.head_commit().unwrap().unwrap()],
        &LogOptions::default(),
    )
    .unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].commit.message, "second");
    let by_agent = ops::log(
        &repo,
        &[repo.head_commit().unwrap().unwrap()],
        &LogOptions {
            agent: Some("agent-a".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(by_agent.len(), 1);
    assert_eq!(by_agent[0].commit.intent.as_deref(), Some("bootstrap"));

    // revision syntax
    assert_eq!(repo.resolve("HEAD~1").unwrap(), log[1].id);
    assert_eq!(repo.resolve("main").unwrap(), log[0].id);
    assert_eq!(repo.resolve(&log[0].id.to_hex()[..10]).unwrap(), log[0].id);

    // events were recorded
    let events = repo.events().read_after(0).unwrap();
    assert!(events.iter().filter(|e| e.kind == "commit").count() == 2);

    // fsck clean
    let f = ops::fsck(&repo).unwrap();
    assert!(f.corrupt.is_empty() && f.missing.is_empty());
}

#[test]
fn branches_switch_and_merge_clean() {
    let (_d, repo) = fresh();
    let root = repo.work_dir().to_path_buf();
    write(&root, "a.txt", "1\n2\n3\n4\n5\n");
    commit_all(&repo, "base", None);

    ops::switch(&repo, "feature", true, false).unwrap();
    write(&root, "a.txt", "1\n2\n3\n4\n5\n6\n");
    write(&root, "b.txt", "new\n");
    commit_all(&repo, "feature work", Some("agent-f"));

    ops::switch(&repo, "main", false, false).unwrap();
    assert!(!root.join("b.txt").exists());
    write(&root, "a.txt", "0\n1\n2\n3\n4\n5\n");
    commit_all(&repo, "main work", None);

    let r = ops::merge(&repo, "feature", MergeOptions::default()).unwrap();
    match r {
        MergeResult::Merged { auto_merged, .. } => assert_eq!(auto_merged, vec!["a.txt"]),
        other => panic!("expected merge, got {other:?}"),
    }
    assert_eq!(
        fs::read_to_string(root.join("a.txt")).unwrap(),
        "0\n1\n2\n3\n4\n5\n6\n"
    );
    assert!(root.join("b.txt").exists());
    let head = repo
        .store()
        .read_commit(&repo.head_commit().unwrap().unwrap())
        .unwrap();
    assert_eq!(head.parents.len(), 2);

    // fast-forward case
    ops::switch(&repo, "feature", false, false).unwrap();
    let r = ops::merge(&repo, "main", MergeOptions::default()).unwrap();
    assert!(matches!(r, MergeResult::FastForward { .. }));
    let r = ops::merge(&repo, "main", MergeOptions::default()).unwrap();
    assert!(matches!(r, MergeResult::AlreadyUpToDate { .. }));
}

#[test]
fn merge_conflict_then_resolve() {
    let (_d, repo) = fresh();
    let root = repo.work_dir().to_path_buf();
    write(&root, "a.txt", "line\n");
    commit_all(&repo, "base", None);
    ops::switch(&repo, "x", true, false).unwrap();
    write(&root, "a.txt", "from x\n");
    commit_all(&repo, "x", None);
    ops::switch(&repo, "main", false, false).unwrap();
    write(&root, "a.txt", "from main\n");
    commit_all(&repo, "main", None);

    let r = ops::merge(&repo, "x", MergeOptions::default()).unwrap();
    let MergeResult::Conflicts { conflicts, .. } = r else {
        panic!("expected conflicts")
    };
    assert_eq!(conflicts.len(), 1);
    let content = fs::read_to_string(root.join("a.txt")).unwrap();
    assert!(content.contains("<<<<<<< main") && content.contains(">>>>>>> x"));
    let st = worktree::status(&repo, true).unwrap();
    assert!(st.merge_in_progress);
    assert_eq!(st.conflicts, vec!["a.txt"]);
    assert!(ops::commit(
        &repo,
        CommitOptions {
            message: "m".into(),
            ..Default::default()
        }
    )
    .is_err());

    write(&root, "a.txt", "resolved\n");
    worktree::add(&repo, &["a.txt".into()], false).unwrap();
    let c = ops::commit(
        &repo,
        CommitOptions {
            message: "merge".into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(c.parents.len(), 2);
    assert!(!worktree::status(&repo, true).unwrap().merge_in_progress);
}

#[test]
fn leases_block_other_agents_commits() {
    let (_d, repo) = fresh();
    let root = repo.work_dir().to_path_buf();
    write(&root, "src/auth/login.rs", "a\n");
    write(&root, "src/db/pool.rs", "b\n");
    commit_all(&repo, "base", None);

    repo.leases()
        .claim(LeaseRequest {
            holder: "agent-a".into(),
            paths: vec!["src/auth".into()],
            mode: LeaseMode::Exclusive,
            ttl: Some(std::time::Duration::from_secs(600)),
            note: None,
            workspace: None,
        })
        .unwrap();

    write(&root, "src/auth/login.rs", "changed by b\n");
    worktree::add(&repo, &[], false).unwrap();
    let err = ops::commit(
        &repo,
        CommitOptions {
            message: "b".into(),
            agent: Some(AgentInfo {
                id: "agent-b".into(),
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(matches!(err, Error::LeaseConflict { .. }));
    assert_eq!(err.code(), "lease_conflict");

    // The holder can commit; and b can commit elsewhere.
    ops::commit(
        &repo,
        CommitOptions {
            message: "a".into(),
            agent: Some(AgentInfo {
                id: "agent-a".into(),
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .unwrap();
    write(&root, "src/db/pool.rs", "changed by b\n");
    commit_all(&repo, "b db", Some("agent-b"));
}

#[test]
fn checkpoints_and_restore() {
    let (_d, repo) = fresh();
    let root = repo.work_dir().to_path_buf();
    write(&root, "a.txt", "v1\n");
    commit_all(&repo, "base", None);
    write(&root, "a.txt", "v2\n");
    write(&root, "scratch.txt", "untracked\n");
    let cp = ops::checkpoint(&repo, Some("wip".into()), None).unwrap();
    assert_eq!(cp.changes_vs_head, 2);
    // HEAD unchanged
    let head = repo
        .store()
        .read_commit(&repo.head_commit().unwrap().unwrap())
        .unwrap();
    assert_eq!(head.message, "base");

    write(&root, "a.txt", "v3\n");
    fs::remove_file(root.join("scratch.txt")).unwrap();
    ops::restore(&repo, &cp.id.to_hex()).unwrap();
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "v2\n");
    assert_eq!(
        fs::read_to_string(root.join("scratch.txt")).unwrap(),
        "untracked\n"
    );

    let cp2 = ops::checkpoint(&repo, None, None).unwrap();
    assert_eq!(cp2.previous, Some(cp.id));
    assert_eq!(ops::list_checkpoints(&repo, 10).unwrap().len(), 2);
    // Checkpoints never show up in the branch log.
    let log = ops::log(
        &repo,
        &[repo.head_commit().unwrap().unwrap()],
        &LogOptions::default(),
    )
    .unwrap();
    assert_eq!(log.len(), 1);
}

#[test]
fn workspaces_share_store_and_merge_back() {
    let (_d, repo) = fresh();
    let root = repo.work_dir().to_path_buf();
    write(&root, "a.txt", "base\n");
    write(&root, "b.txt", "base\n");
    commit_all(&repo, "base", None);

    let (ws1, meta1) = repo
        .create_workspace("agent1", None, None, Some("agent-1"))
        .unwrap();
    let (ws2, _) = repo
        .create_workspace("agent2", None, None, Some("agent-2"))
        .unwrap();
    assert!(meta1.path.join("a.txt").exists());
    assert_eq!(ws1.current_branch().unwrap().as_deref(), Some("ws/agent1"));

    // Discovery from inside a workspace finds the shared common dir.
    let rediscovered = Repository::discover(&meta1.path.join("sub"))
        .unwrap_or_else(|_| Repository::discover(&meta1.path).unwrap());
    assert_eq!(rediscovered.common_dir(), repo.common_dir());
    assert_eq!(rediscovered.workspace(), "agent1");

    write(ws1.work_dir(), "a.txt", "from agent 1\n");
    commit_all(&ws1, "agent 1 work", Some("agent-1"));
    write(ws2.work_dir(), "b.txt", "from agent 2\n");
    commit_all(&ws2, "agent 2 work", Some("agent-2"));

    // Primary workspace is untouched until we merge.
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "base\n");
    assert!(matches!(
        ops::merge(&repo, "ws/agent1", MergeOptions::default()).unwrap(),
        MergeResult::FastForward { .. }
    ));
    assert!(matches!(
        ops::merge(&repo, "ws/agent2", MergeOptions::default()).unwrap(),
        MergeResult::Merged { .. }
    ));
    assert_eq!(
        fs::read_to_string(root.join("a.txt")).unwrap(),
        "from agent 1\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("b.txt")).unwrap(),
        "from agent 2\n"
    );

    assert_eq!(repo.list_workspaces().unwrap().len(), 3);
    repo.remove_workspace("agent2", true).unwrap();
    assert_eq!(repo.list_workspaces().unwrap().len(), 2);
}

#[test]
fn dirty_worktree_protects_switch() {
    let (_d, repo) = fresh();
    let root = repo.work_dir().to_path_buf();
    write(&root, "a.txt", "1\n");
    commit_all(&repo, "base", None);
    ops::switch(&repo, "f", true, false).unwrap();
    write(&root, "a.txt", "2\n");
    commit_all(&repo, "f", None);
    ops::switch(&repo, "main", false, false).unwrap();
    write(&root, "a.txt", "local edit\n");
    let err = ops::switch(&repo, "f", false, false).unwrap_err();
    assert!(matches!(err, Error::DirtyWorktree(_)));
    assert_eq!(
        fs::read_to_string(root.join("a.txt")).unwrap(),
        "local edit\n"
    );
    ops::switch(&repo, "f", false, true).unwrap();
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "2\n");
}

#[test]
fn concurrent_commits_from_two_handles_use_cas() {
    let (_d, repo) = fresh();
    let root = repo.work_dir().to_path_buf();
    write(&root, "a.txt", "1\n");
    commit_all(&repo, "base", None);
    let head = repo.head_commit().unwrap().unwrap();
    // Simulate a stale writer: move the branch behind the repo's back.
    write(&root, "a.txt", "2\n");
    commit_all(&repo, "second", None);
    let err = repo.advance_head(Some(head), head).unwrap_err();
    assert!(matches!(err, Error::RefConflict { .. }));
    assert!(err.is_retryable());
}

#[test]
fn reflog_reset_and_gc() {
    use agit_core::ops::{GcOptions, ResetMode};
    let (_d, repo) = fresh();
    let root = repo.work_dir().to_path_buf();
    write(&root, "a.txt", "1\n");
    let c1 = commit_all(&repo, "one", None);
    write(&root, "a.txt", "2\n");
    let c2 = commit_all(&repo, "two", None);
    write(&root, "a.txt", "3\n");
    let c3 = commit_all(&repo, "three", None);

    // Reflog recorded every move of the branch and of HEAD.
    let log = repo.refs().reflog("refs/heads/main").unwrap();
    assert_eq!(log.len(), 3);
    assert_eq!(log[2].new, Some(c3.id));
    assert!(log[2].message.starts_with("commit: three"));
    assert_eq!(repo.head_reflog().unwrap().len(), 3);

    // Hard reset back two commits, then undo it through the reflog.
    let r = ops::reset(&repo, &c1.id.to_hex(), ResetMode::Hard).unwrap();
    assert_eq!((r.from, r.to), (Some(c3.id), c1.id));
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "1\n");
    assert!(worktree::status(&repo, true).unwrap().is_clean());
    let log = repo.refs().reflog("refs/heads/main").unwrap();
    assert_eq!(log.last().unwrap().old, Some(c3.id));
    ops::reset(
        &repo,
        &log.last().unwrap().old.unwrap().to_hex(),
        ResetMode::Hard,
    )
    .unwrap();
    assert_eq!(repo.head_commit().unwrap(), Some(c3.id));
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "3\n");

    // Soft reset keeps the working tree and index.
    ops::reset(&repo, "HEAD~1", ResetMode::Soft).unwrap();
    assert_eq!(repo.head_commit().unwrap(), Some(c2.id));
    let st = worktree::status(&repo, true).unwrap();
    assert_eq!(st.staged.len(), 1);
    ops::reset(&repo, &c3.id.to_hex(), ResetMode::Mixed).unwrap();
    assert!(worktree::status(&repo, true).unwrap().is_clean());

    // GC: a dangling blob is removed once it is old enough; reachable and
    // reflog-referenced objects survive.
    let dangling = repo
        .store()
        .write(agit_core::ObjectKind::Blob, b"dangling")
        .unwrap();
    let before = repo.store().iter_ids().unwrap().len();
    let r = ops::gc(
        &repo,
        &GcOptions {
            grace_secs: 3600,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(r.objects_removed, 0);
    assert_eq!(r.objects_kept_young, 1);
    let r = ops::gc(
        &repo,
        &GcOptions {
            grace_secs: 0,
            dry_run: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(r.objects_removed, 1);
    assert!(repo.store().contains(&dangling));
    let r = ops::gc(
        &repo,
        &GcOptions {
            grace_secs: 0,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(r.objects_removed, 1);
    assert!(!repo.store().contains(&dangling));
    assert_eq!(repo.store().iter_ids().unwrap().len(), before - 1);
    // Every commit is still readable (c1 is only reachable through the reflog
    // and the history; both must be roots).
    for c in [c1.id, c2.id, c3.id] {
        repo.store().read_commit(&c).unwrap();
    }
    assert!(ops::fsck(&repo).unwrap().missing.is_empty());

    // Expiring the reflog (0 seconds) and running gc drops nothing that is
    // still reachable from the branch.
    let r = ops::gc(
        &repo,
        &GcOptions {
            grace_secs: 0,
            reflog_expire_secs: Some(0),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(r.reflog_entries_expired > 0);
    assert_eq!(r.objects_removed, 0);
    assert!(ops::fsck(&repo).unwrap().missing.is_empty());

    // Checkpoints expire by age: a fresh one stays.
    ops::checkpoint(&repo, None, None).unwrap();
    let r = ops::gc(
        &repo,
        &GcOptions {
            grace_secs: 0,
            checkpoint_expire_secs: Some(3600),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(r.checkpoints_expired, 0);
    assert_eq!(ops::list_checkpoints(&repo, 10).unwrap().len(), 1);
    let r = ops::gc(
        &repo,
        &GcOptions {
            grace_secs: 0,
            checkpoint_expire_secs: Some(0),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(r.checkpoints_expired, 1);
    assert_eq!(ops::list_checkpoints(&repo, 10).unwrap().len(), 0);
}

#[test]
fn blame_revert_cherry_pick_stash_grep() {
    use agit_core::history::{self, ApplyResult};
    let (_d, repo) = fresh();
    let root = repo.work_dir().to_path_buf();
    write(&root, "f.txt", "one\ntwo\nthree\n");
    let c1 = commit_all(&repo, "first", Some("a1"));
    write(&root, "f.txt", "one\ntwo\nthree\nfour\n");
    let c2 = commit_all(&repo, "second", Some("a2"));
    write(&root, "f.txt", "zero\none\ntwo\nthree\nfour\n");
    let c3 = commit_all(&repo, "third", Some("a3"));

    // blame
    let b = history::blame(repo.store(), c3.id, "f.txt", 100).unwrap();
    let owners: Vec<ObjectId> = b.lines.iter().map(|l| l.commit).collect();
    assert_eq!(owners, vec![c3.id, c1.id, c1.id, c1.id, c2.id]);
    assert_eq!(b.commits[&c2.id].agent.as_deref(), Some("a2"));

    // revert the middle commit: "four" disappears, rest stays
    let r = history::revert(&repo, &c2.id.to_hex(), None).unwrap();
    assert!(matches!(r, ApplyResult::Committed { .. }));
    assert_eq!(
        fs::read_to_string(root.join("f.txt")).unwrap(),
        "zero\none\ntwo\nthree\n"
    );
    let head = repo
        .store()
        .read_commit(&repo.head_commit().unwrap().unwrap())
        .unwrap();
    assert!(head.message.starts_with("Revert \"second\""));
    assert_eq!(head.meta["agit.reverts"], serde_json::json!(c2.id));

    // cherry-pick it back on a branch
    ops::switch(&repo, "pick", true, false).unwrap();
    let r = history::cherry_pick(&repo, &c2.id.to_hex(), None).unwrap();
    assert!(matches!(r, ApplyResult::Committed { .. }));
    assert_eq!(
        fs::read_to_string(root.join("f.txt")).unwrap(),
        "zero\none\ntwo\nthree\nfour\n"
    );
    let head = repo
        .store()
        .read_commit(&repo.head_commit().unwrap().unwrap())
        .unwrap();
    assert_eq!(head.message, "second");
    assert_eq!(
        head.meta["agit.cherry_picked_from"],
        serde_json::json!(c2.id)
    );

    // stash: dirty tree saved and restored
    write(&root, "f.txt", "zero\none\ntwo\nthree\nfour\nfive\n");
    write(&root, "new.txt", "untracked\n");
    let e = history::stash_push(&repo, Some("wip".into())).unwrap();
    assert_eq!(e.index, 0);
    assert!(worktree::status(&repo, true).unwrap().is_clean());
    assert!(!root.join("new.txt").exists());
    assert_eq!(history::stash_list(&repo).unwrap().len(), 1);
    let r = history::stash_apply(&repo, 0, true).unwrap();
    assert!(matches!(r, ApplyResult::Committed { .. }));
    assert_eq!(
        fs::read_to_string(root.join("new.txt")).unwrap(),
        "untracked\n"
    );
    assert!(fs::read_to_string(root.join("f.txt"))
        .unwrap()
        .ends_with("five\n"));
    assert!(history::stash_list(&repo).unwrap().is_empty());
    let st = worktree::status(&repo, true).unwrap();
    assert!(!st.unstaged.is_empty() || !st.untracked.is_empty());

    // grep in worktree and in a revision
    let m = history::grep(&repo, "^f(our|ive)$", None, false, 1 << 20).unwrap();
    assert_eq!(m.len(), 2);
    let m = history::grep(&repo, "FOUR", Some(&c2.id.to_hex()), true, 1 << 20).unwrap();
    assert_eq!((m[0].path.as_str(), m[0].line), ("f.txt", 4));

    // rename detection
    commit_all(&repo, "wip in", None);
    fs::rename(root.join("f.txt"), root.join("g.txt")).unwrap();
    commit_all(&repo, "rename", None);
    let head = repo.head_commit().unwrap().unwrap();
    let c = repo.store().read_commit(&head).unwrap();
    let parent_tree = repo.store().read_commit(&c.parents[0]).unwrap().tree;
    let changes = agit_core::diff::detect_renames(
        repo.store(),
        agit_core::diff::diff_trees(repo.store(), Some(&parent_tree), Some(&c.tree)).unwrap(),
        50,
    );
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].kind, agit_core::diff::ChangeKind::Renamed);
    assert_eq!(changes[0].from.as_deref(), Some("f.txt"));
    assert_eq!(changes[0].similarity, Some(100));
}

#[test]
fn concurrent_workspaces_and_leases_stress() {
    use std::sync::{Arc, Barrier};
    let (_d, repo) = fresh();
    let root = repo.work_dir().to_path_buf();
    write(&root, "README.md", "base\n");
    commit_all(&repo, "base", None);

    const AGENTS: usize = 8;
    const COMMITS: usize = 15;
    let mut paths = Vec::new();
    for i in 0..AGENTS {
        let (_, meta) = repo
            .create_workspace(&format!("w{i}"), None, None, Some(&format!("agent-{i}")))
            .unwrap();
        paths.push(meta.path);
    }
    let barrier = Arc::new(Barrier::new(AGENTS));
    let handles: Vec<_> = (0..AGENTS)
        .map(|i| {
            let path = paths[i].clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let ws = Repository::discover(&path).unwrap();
                barrier.wait();
                let mut lease_wins = 0;
                for j in 0..COMMITS {
                    write(
                        ws.work_dir(),
                        &format!("agent{i}/file{j}.txt"),
                        &format!("{i}-{j}\n"),
                    );
                    worktree::add(&ws, &[], false).unwrap();
                    ops::commit(
                        &ws,
                        CommitOptions {
                            message: format!("agent {i} commit {j}"),
                            agent: Some(AgentInfo {
                                id: format!("agent-{i}"),
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                    )
                    .unwrap();
                    // Everyone fights for the same path; exactly one holder at a time.
                    if let Ok(l) = ws.leases().claim(agit_core::lease::LeaseRequest {
                        holder: format!("agent-{i}"),
                        paths: vec!["shared/hot.rs".into()],
                        mode: agit_core::lease::LeaseMode::Exclusive,
                        ttl: Some(std::time::Duration::from_secs(60)),
                        note: None,
                        workspace: None,
                    }) {
                        lease_wins += 1;
                        let holders: Vec<String> = ws
                            .leases()
                            .list()
                            .unwrap()
                            .into_iter()
                            .filter(|x| x.paths[0] == "shared/hot.rs")
                            .map(|x| x.holder)
                            .collect();
                        assert_eq!(
                            holders.len(),
                            1,
                            "two exclusive holders at once: {holders:?}"
                        );
                        ws.leases().release(&l.id).unwrap();
                    }
                }
                (ws.head_commit().unwrap().unwrap(), lease_wins)
            })
        })
        .collect();
    let results: Vec<(ObjectId, usize)> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert!(results.iter().map(|(_, w)| *w).sum::<usize>() > 0);

    // Integrate everything into main; every file must be there and the store intact.
    for i in 0..AGENTS {
        let r = ops::merge(&repo, &format!("ws/w{i}"), MergeOptions::default()).unwrap();
        assert!(!matches!(r, MergeResult::Conflicts { .. }));
    }
    for i in 0..AGENTS {
        for j in 0..COMMITS {
            assert_eq!(
                fs::read_to_string(root.join(format!("agent{i}/file{j}.txt"))).unwrap(),
                format!("{i}-{j}\n")
            );
        }
    }
    let f = ops::fsck(&repo).unwrap();
    assert!(f.corrupt.is_empty() && f.missing.is_empty(), "{f:?}");
    let log = ops::log(
        &repo,
        &[repo.head_commit().unwrap().unwrap()],
        &LogOptions::default(),
    )
    .unwrap();
    assert!(log.len() > AGENTS * COMMITS);
    // Reflog recorded every branch move; gc keeps everything reachable.
    let r = ops::gc(
        &repo,
        &ops::GcOptions {
            grace_secs: 0,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(r.objects_removed, 0);
    assert!(ops::fsck(&repo).unwrap().missing.is_empty());
}

#[test]
fn user_agit_dir_in_home_is_not_a_repository() {
    // `~/.agit` holds signing keys and sessions; directories below the home
    // directory must not be mistaken for a repository rooted there.
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    fs::create_dir_all(home.join(".agit/keys")).unwrap();
    fs::write(home.join(".agit/sessions.json"), "{}").unwrap();
    let plain = home.join("work/notes");
    fs::create_dir_all(&plain).unwrap();

    match Repository::discover(&plain) {
        Err(Error::NotARepository(_)) => {}
        other => panic!("expected not_a_repository, got {other:?}"),
    }
    // agit 0.1.0 wrote a config.json into ~/.agit when `remote add` ran
    // outside a repository; that alone must not turn it into one.
    fs::write(
        home.join(".agit/config.json"),
        r#"{"format_version":1,"default_branch":"main","remotes":{"origin":{"url":"http://h/api/v1/repos/x"}}}"#,
    )
    .unwrap();
    match Repository::discover(&plain) {
        Err(Error::NotARepository(_)) => {}
        other => panic!("expected not_a_repository with a stray config.json, got {other:?}"),
    }
    match Repository::init(home) {
        Err(Error::Invalid(msg)) => assert!(msg.contains("not a repository"), "{msg}"),
        other => panic!(
            "expected invalid, got {:?}",
            other.map(|r| r.work_dir().to_path_buf())
        ),
    }

    // A real repository below the home directory is found from a subdirectory.
    let proj = home.join("projekt");
    Repository::init(&proj).unwrap();
    fs::create_dir_all(proj.join("src/deep")).unwrap();
    let found = Repository::discover(&proj.join("src/deep")).unwrap();
    assert_eq!(
        found.work_dir(),
        proj.canonicalize().unwrap_or(proj.clone())
    );

    // A stray `.agit` directory inside a repository does not shadow it.
    fs::create_dir_all(proj.join("vendor/.agit/keys")).unwrap();
    let found = Repository::discover(&proj.join("vendor")).unwrap();
    assert_eq!(
        found.work_dir(),
        proj.canonicalize().unwrap_or(proj.clone())
    );
}
