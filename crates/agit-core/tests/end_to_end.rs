//! End-to-end tests of the core operations on real temporary repositories.

use std::fs;
use std::path::Path;

use agit_core::lease::{LeaseMode, LeaseRequest};
use agit_core::ops::{self, CommitOptions, LogOptions, MergeOptions, MergeResult};
use agit_core::worktree;
use agit_core::{AgentInfo, Error, Repository};

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
            agent: agent.map(|a| AgentInfo { id: a.into(), ..Default::default() }),
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
    assert_eq!(st.untracked, vec![".agitignore", "README.md", "src/main.rs"]);

    let r = worktree::add(&repo, &[], false).unwrap();
    assert_eq!(r.added.len(), 3);

    let c = ops::commit(
        &repo,
        CommitOptions {
            message: "initial".into(),
            intent: Some("bootstrap".into()),
            task: Some("T-1".into()),
            agent: Some(AgentInfo { id: "agent-a".into(), model: Some("m".into()), ..Default::default() }),
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
        ops::commit(&repo, CommitOptions { message: "x".into(), ..Default::default() }),
        Err(Error::NothingToCommit)
    ));
    commit_all(&repo, "second", Some("agent-b"));

    let log = ops::log(&repo, &[repo.head_commit().unwrap().unwrap()], &LogOptions::default()).unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].commit.message, "second");
    let by_agent = ops::log(
        &repo,
        &[repo.head_commit().unwrap().unwrap()],
        &LogOptions { agent: Some("agent-a".into()), ..Default::default() },
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
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "0\n1\n2\n3\n4\n5\n6\n");
    assert!(root.join("b.txt").exists());
    let head = repo.store().read_commit(&repo.head_commit().unwrap().unwrap()).unwrap();
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
    let MergeResult::Conflicts { conflicts, .. } = r else { panic!("expected conflicts") };
    assert_eq!(conflicts.len(), 1);
    let content = fs::read_to_string(root.join("a.txt")).unwrap();
    assert!(content.contains("<<<<<<< main") && content.contains(">>>>>>> x"));
    let st = worktree::status(&repo, true).unwrap();
    assert!(st.merge_in_progress);
    assert_eq!(st.conflicts, vec!["a.txt"]);
    assert!(ops::commit(&repo, CommitOptions { message: "m".into(), ..Default::default() }).is_err());

    write(&root, "a.txt", "resolved\n");
    worktree::add(&repo, &["a.txt".into()], false).unwrap();
    let c = ops::commit(&repo, CommitOptions { message: "merge".into(), ..Default::default() }).unwrap();
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
            agent: Some(AgentInfo { id: "agent-b".into(), ..Default::default() }),
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
            agent: Some(AgentInfo { id: "agent-a".into(), ..Default::default() }),
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
    let head = repo.store().read_commit(&repo.head_commit().unwrap().unwrap()).unwrap();
    assert_eq!(head.message, "base");

    write(&root, "a.txt", "v3\n");
    fs::remove_file(root.join("scratch.txt")).unwrap();
    ops::restore(&repo, &cp.id.to_hex()).unwrap();
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "v2\n");
    assert_eq!(fs::read_to_string(root.join("scratch.txt")).unwrap(), "untracked\n");

    let cp2 = ops::checkpoint(&repo, None, None).unwrap();
    assert_eq!(cp2.previous, Some(cp.id));
    assert_eq!(ops::list_checkpoints(&repo, 10).unwrap().len(), 2);
    // Checkpoints never show up in the branch log.
    let log = ops::log(&repo, &[repo.head_commit().unwrap().unwrap()], &LogOptions::default()).unwrap();
    assert_eq!(log.len(), 1);
}

#[test]
fn workspaces_share_store_and_merge_back() {
    let (_d, repo) = fresh();
    let root = repo.work_dir().to_path_buf();
    write(&root, "a.txt", "base\n");
    write(&root, "b.txt", "base\n");
    commit_all(&repo, "base", None);

    let (ws1, meta1) = repo.create_workspace("agent1", None, None, Some("agent-1")).unwrap();
    let (ws2, _) = repo.create_workspace("agent2", None, None, Some("agent-2")).unwrap();
    assert!(meta1.path.join("a.txt").exists());
    assert_eq!(ws1.current_branch().unwrap().as_deref(), Some("ws/agent1"));

    // Discovery from inside a workspace finds the shared common dir.
    let rediscovered = Repository::discover(&meta1.path.join("sub")).unwrap_or_else(|_| Repository::discover(&meta1.path).unwrap());
    assert_eq!(rediscovered.common_dir(), repo.common_dir());
    assert_eq!(rediscovered.workspace(), "agent1");

    write(ws1.work_dir(), "a.txt", "from agent 1\n");
    commit_all(&ws1, "agent 1 work", Some("agent-1"));
    write(ws2.work_dir(), "b.txt", "from agent 2\n");
    commit_all(&ws2, "agent 2 work", Some("agent-2"));

    // Primary workspace is untouched until we merge.
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "base\n");
    assert!(matches!(ops::merge(&repo, "ws/agent1", MergeOptions::default()).unwrap(), MergeResult::FastForward { .. }));
    assert!(matches!(ops::merge(&repo, "ws/agent2", MergeOptions::default()).unwrap(), MergeResult::Merged { .. }));
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "from agent 1\n");
    assert_eq!(fs::read_to_string(root.join("b.txt")).unwrap(), "from agent 2\n");

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
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "local edit\n");
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
