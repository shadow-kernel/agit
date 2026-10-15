use std::fs;
use std::path::Path;

use agit_core::ops::{self, CommitOptions};
use agit_core::worktree;
use agit_core::{EntryMode, Repository};
use agit_protocol::submodule;

fn write(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, content).unwrap();
}

fn commit_all(repo: &Repository, msg: &str) -> ops::CommitResult {
    worktree::add(repo, &[], false).unwrap();
    ops::commit(
        repo,
        CommitOptions {
            message: msg.into(),
            ..Default::default()
        },
    )
    .unwrap()
}

#[test]
fn add_update_status_with_local_clone() {
    let tmp = tempfile::tempdir().unwrap();
    // A library repository with two commits.
    let lib = Repository::init(&tmp.path().join("lib")).unwrap();
    write(lib.work_dir(), "lib.rs", "v1\n");
    let lib_c1 = commit_all(&lib, "lib v1");
    write(lib.work_dir(), "lib.rs", "v2\n");
    let lib_c2 = commit_all(&lib, "lib v2");

    // The application adds it as a submodule.
    let app = Repository::init(&tmp.path().join("app")).unwrap();
    write(app.work_dir(), "main.rs", "fn main() {}\n");
    commit_all(&app, "app");
    let m = submodule::add(
        &app,
        &lib.work_dir().display().to_string(),
        "vendor/lib",
        None,
    )
    .unwrap();
    assert_eq!(m.path, "vendor/lib");
    assert!(app.work_dir().join("vendor/lib/.agit").is_dir());
    assert_eq!(
        fs::read_to_string(app.work_dir().join("vendor/lib/lib.rs")).unwrap(),
        "v2\n"
    );

    // Staged: .agitmodules and the submodule entry pinned at lib HEAD.
    let st = worktree::status(&app, true).unwrap();
    let staged: Vec<(&str, EntryMode)> = st
        .staged
        .iter()
        .map(|c| (c.path.as_str(), c.new.as_ref().unwrap().mode))
        .collect();
    assert!(staged.contains(&(".agitmodules", EntryMode::File)));
    assert!(staged.contains(&("vendor/lib", EntryMode::Submodule)));
    assert!(
        st.untracked.is_empty(),
        "nested repo files must not leak: {:?}",
        st.untracked
    );
    let c = ops::commit(
        &app,
        CommitOptions {
            message: "add lib".into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(c
        .changes
        .iter()
        .any(|ch| ch.path == "vendor/lib" && ch.new.as_ref().unwrap().id == lib_c2.id));
    let st = worktree::status(&app, true).unwrap();
    assert!(
        st.is_clean(),
        "status after commit: {}",
        serde_json::to_string_pretty(&st).unwrap()
    );

    // Tree object carries the submodule mode and the pinned commit id.
    let flat = agit_core::index::flatten_tree(app.store(), &c.tree).unwrap();
    let e = &flat["vendor/lib"];
    assert_eq!(e.mode, EntryMode::Submodule);
    assert_eq!(e.id, lib_c2.id);

    // Move the nested checkout to v1: the outer repo sees a modification.
    let nested = Repository::open(&app.work_dir().join("vendor/lib")).unwrap();
    ops::switch(&nested, &lib_c1.id.to_hex(), false, false).unwrap();
    let st = worktree::status(&app, true).unwrap();
    assert_eq!(st.unstaged.len(), 1);
    assert_eq!(st.unstaged[0].path, "vendor/lib");
    let s = submodule::status(&app).unwrap();
    assert_eq!(s[0].state, "modified");
    assert_eq!(s[0].actual, Some(lib_c1.id));

    // `submodule update` restores the pinned commit.
    let u = submodule::update(&app, None).unwrap();
    assert_eq!(u[0].action, "checked_out");
    assert_eq!(nested.head_commit().unwrap(), Some(lib_c2.id));
    assert!(worktree::status(&app, true).unwrap().is_clean());
    assert_eq!(submodule::status(&app).unwrap()[0].state, "ok");

    // A fresh clone of the app has the pin but no checkout; update clones it.
    let (clone, _) =
        agit_protocol::sync::clone_local(app.work_dir(), &tmp.path().join("app2")).unwrap();
    assert_eq!(submodule::status(&clone).unwrap()[0].state, "missing");
    let u = submodule::update(&clone, None).unwrap();
    assert_eq!(u[0].action, "cloned");
    assert_eq!(
        fs::read_to_string(clone.work_dir().join("vendor/lib/lib.rs")).unwrap(),
        "v2\n"
    );
    assert_eq!(submodule::status(&clone).unwrap()[0].state, "ok");

    // Patch rendering shows subproject lines.
    let changes = agit_core::diff::diff_trees(
        app.store(),
        Some(&app.resolve_commit("HEAD~1").unwrap().1.tree),
        Some(&c.tree),
    )
    .unwrap();
    let patch = agit_core::diff::render_patch(app.store(), &changes, 3).unwrap();
    assert!(patch.contains(&format!("+Subproject commit {}", lib_c2.id)));

    // Remove keeps the directory unless asked.
    submodule::remove(&app, "vendor/lib", false).unwrap();
    assert!(app.work_dir().join("vendor/lib/.agit").exists());
    assert!(submodule::status(&app).unwrap().is_empty());
}

#[test]
fn switch_clone_and_local_fetch_with_submodule() {
    let tmp = tempfile::tempdir().unwrap();
    let lib = Repository::init(&tmp.path().join("lib")).unwrap();
    write(lib.work_dir(), "lib.rs", "v1\n");
    commit_all(&lib, "lib v1");
    let app = Repository::init(&tmp.path().join("app")).unwrap();
    write(app.work_dir(), "main.rs", "fn main() {}\n");
    commit_all(&app, "app");
    submodule::add(
        &app,
        &lib.work_dir().display().to_string(),
        "vendor/lib",
        None,
    )
    .unwrap();
    commit_all(&app, "add lib");

    // Switching branches with a submodule entry must not touch the nested repo.
    ops::switch(&app, "feature", true, false).unwrap();
    write(app.work_dir(), "x.txt", "x\n");
    commit_all(&app, "feature");
    ops::switch(&app, "main", false, false).unwrap();
    assert!(app.work_dir().join("vendor/lib/.agit").is_dir());
    assert!(worktree::status(&app, true).unwrap().is_clean());
    ops::switch(&app, "feature", false, false).unwrap();
    assert!(worktree::status(&app, true).unwrap().is_clean());

    // A clone without the nested checkout is clean; after update it is still clean.
    let (clone, _) =
        agit_protocol::sync::clone_local(app.work_dir(), &tmp.path().join("app2")).unwrap();
    let st = worktree::status(&clone, true).unwrap();
    assert!(st.is_clean(), "{st:?}");
    submodule::update(&clone, None).unwrap();
    assert!(worktree::status(&clone, true).unwrap().is_clean());
    // Pull from the local path parent works (fetch_local).
    let r = agit_protocol::sync::fetch(&clone, "origin", None).unwrap();
    assert_eq!(r.remote, "origin");

    // Inside the nested repo, fetching from its local-path origin brings new commits.
    write(lib.work_dir(), "lib.rs", "v2\n");
    let lib_c2 = commit_all(&lib, "lib v2");
    let nested = Repository::open(&clone.work_dir().join("vendor/lib")).unwrap();
    let f = agit_protocol::sync::fetch(&nested, "origin", None).unwrap();
    assert!(f.refs.iter().any(|r| r.new == lib_c2.id));
    assert!(nested.store().contains(&lib_c2.id));
}
