use std::fs;
use std::path::Path;
use std::process::Command;

use agit_core::Repository;
use agit_git_bridge::{import, ImportOptions};

fn git(dir: &Path, args: &[&str]) {
    let st = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Ann")
        .env("GIT_AUTHOR_EMAIL", "ann@example.com")
        .env("GIT_COMMITTER_NAME", "Ann")
        .env("GIT_COMMITTER_EMAIL", "ann@example.com")
        .env("GIT_AUTHOR_DATE", "2024-01-02T03:04:05Z")
        .env("GIT_COMMITTER_DATE", "2024-01-02T03:04:05Z")
        .status()
        .unwrap();
    assert!(st.success(), "git {args:?} failed");
}

#[test]
fn imports_history_branches_tags_and_trailers() {
    if Command::new("git").arg("--version").output().is_err() {
        eprintln!("git not installed; skipping");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let g = tmp.path().join("g");
    fs::create_dir_all(g.join("src")).unwrap();
    git(&g, &["init", "-q", "-b", "main"]);
    fs::write(g.join("README.md"), "hello\n").unwrap();
    fs::write(g.join("src/main.rs"), "fn main() {}\n").unwrap();
    git(&g, &["add", "-A"]);
    git(
        &g,
        &[
            "commit",
            "-q",
            "-m",
            "initial\n\nIntent: bootstrap\nTask: T-1\nAgent: coder-1\nModel: m1",
        ],
    );
    git(&g, &["checkout", "-q", "-b", "feature"]);
    fs::write(g.join("src/lib.rs"), "pub fn x() {}\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::write(g.join("run.sh"), "#!/bin/sh\n").unwrap();
        fs::set_permissions(g.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink("README.md", g.join("link")).unwrap();
    }
    git(&g, &["add", "-A"]);
    git(&g, &["commit", "-q", "-m", "feature work"]);
    git(&g, &["checkout", "-q", "main"]);
    fs::write(g.join("README.md"), "hello world\n").unwrap();
    git(&g, &["commit", "-q", "-am", "readme"]);
    git(
        &g,
        &["merge", "-q", "--no-ff", "-m", "merge feature", "feature"],
    );
    git(&g, &["tag", "v1"]);
    git(&g, &["tag", "-a", "-m", "annotated", "v1-annotated"]);

    let repo = Repository::init(&tmp.path().join("a")).unwrap();
    let report = import(
        &g,
        &repo,
        &ImportOptions {
            tags: true,
            checkout: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(report.commits, 4);
    assert_eq!(report.branches, vec!["feature", "main"]);
    assert_eq!(report.tags, vec!["v1", "v1-annotated"]);
    assert_eq!(repo.current_branch().unwrap().as_deref(), Some("main"));

    // Working tree checked out from the merge commit.
    assert_eq!(
        fs::read_to_string(repo.work_dir().join("README.md")).unwrap(),
        "hello world\n"
    );
    assert!(repo.work_dir().join("src/lib.rs").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert!(
            fs::metadata(repo.work_dir().join("run.sh"))
                .unwrap()
                .permissions()
                .mode()
                & 0o111
                != 0
        );
        assert_eq!(
            fs::read_link(repo.work_dir().join("link"))
                .unwrap()
                .to_string_lossy(),
            "README.md"
        );
    }
    let st = agit_core::worktree::status(&repo, true).unwrap();
    assert!(st.is_clean() && st.untracked.is_empty(), "{:?}", st);

    // History shape and metadata.
    let head = repo.head_commit().unwrap().unwrap();
    let merge = repo.store().read_commit(&head).unwrap();
    assert_eq!(merge.parents.len(), 2);
    assert_eq!(merge.message, "merge feature");
    assert!(merge.meta.contains_key("git.sha"));
    let root = repo.resolve("HEAD~2").unwrap();
    let first = repo.store().read_commit(&root).unwrap();
    assert_eq!(first.message, "initial");
    assert_eq!(first.intent.as_deref(), Some("bootstrap"));
    assert_eq!(first.task.as_deref(), Some("T-1"));
    assert_eq!(first.agent.as_ref().unwrap().id, "coder-1");
    assert_eq!(first.agent.as_ref().unwrap().model.as_deref(), Some("m1"));
    assert_eq!(first.author.name, "Ann");
    assert_eq!(first.time_ms, 1_704_164_645_000);
    assert_eq!(repo.resolve("v1").unwrap(), head);
    assert_eq!(repo.resolve("v1-annotated").unwrap(), head);

    // Re-import is idempotent thanks to the mapping table.
    let again = import(
        &g,
        &repo,
        &ImportOptions {
            tags: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(again.commits, 0);
    assert_eq!(repo.head_commit().unwrap(), Some(head));
    assert!(agit_core::ops::fsck(&repo).unwrap().corrupt.is_empty());
}

#[test]
fn export_roundtrip_incremental() {
    use agit_core::ops::{self, CommitOptions};
    use agit_git_bridge::{export, ExportOptions};
    if Command::new("git").arg("--version").output().is_err() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    // Native agit history: two commits + a branch with a merge.
    let repo = Repository::init(&tmp.path().join("a")).unwrap();
    let w = |rel: &str, c: &str| {
        let p = repo.work_dir().join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, c).unwrap();
    };
    let commit = |msg: &str, intent: Option<&str>| {
        agit_core::worktree::add(&repo, &[], false).unwrap();
        ops::commit(
            &repo,
            CommitOptions {
                message: msg.into(),
                intent: intent.map(str::to_string),
                task: Some("T-7".into()),
                agent: Some(agit_core::AgentInfo {
                    id: "coder-1".into(),
                    model: Some("m".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .unwrap()
    };
    w("a.txt", "1\n");
    w("dir/b.txt", "b\n");
    commit("first", Some("start"));
    ops::switch(&repo, "side", true, false).unwrap();
    w("dir/c.txt", "c\n");
    commit("side work", None);
    ops::switch(&repo, "main", false, false).unwrap();
    w("a.txt", "2\n");
    commit("second", Some("bump"));
    ops::merge(&repo, "side", Default::default()).unwrap();
    ops::create_tag(&repo, "v0", None).unwrap();

    let g = tmp.path().join("g");
    let r = export(
        &repo,
        &g,
        &ExportOptions {
            tags: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(r.commits, 4);
    assert_eq!(r.branches, vec!["main", "side"]);
    let out = Command::new("git")
        .arg("-C")
        .arg(&g)
        .args(["log", "--format=%s", "--all"])
        .output()
        .unwrap();
    let log = String::from_utf8_lossy(&out.stdout);
    for m in ["first", "side work", "second", "Merge side into main"] {
        assert!(log.contains(m), "{log}");
    }
    let out = Command::new("git")
        .arg("-C")
        .arg(&g)
        .args(["log", "-1", "--format=%B", "main~1"])
        .output()
        .unwrap();
    let body = String::from_utf8_lossy(&out.stdout);
    assert!(
        body.contains("Intent: bump")
            && body.contains("Task: T-7")
            && body.contains("Agent: coder-1"),
        "{body}"
    );
    let out = Command::new("git")
        .arg("-C")
        .arg(&g)
        .args(["show", "main:a.txt"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "2\n");
    let out = Command::new("git")
        .arg("-C")
        .arg(&g)
        .args(["rev-list", "--count", "main"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "4");
    let out = Command::new("git")
        .arg("-C")
        .arg(&g)
        .args(["tag"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "v0");

    // Incremental: one more agit commit exports exactly one git commit.
    w("a.txt", "3\n");
    commit("third", Some("again"));
    let r = export(&repo, &g, &ExportOptions::default()).unwrap();
    assert_eq!(r.commits, 1);
    let out = Command::new("git")
        .arg("-C")
        .arg(&g)
        .args(["rev-list", "--count", "main"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "5");

    // And importing that git repo back yields no new commits (mapping table).
    let again = import(&g, &repo, &ImportOptions::default()).unwrap();
    assert_eq!(again.commits, 0);
}
