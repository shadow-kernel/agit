//! In-process micro-commit benchmark: `cargo run --release -p agit-core --example bench_commits -- 5000`
use std::time::Instant;

use agit_core::ops::{self, CommitOptions};
use agit_core::worktree;
use agit_core::Repository;

fn main() {
    let n: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(2000);
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(&dir.path().join("r")).unwrap();
    let root = repo.work_dir().to_path_buf();
    // Seed with 2000 files so status/add work against a realistic tree.
    for i in 0..2000 {
        let p = root.join(format!("src/mod{}/file{}.rs", i % 50, i));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, format!("// file {i}\nfn f{i}() {{}}\n")).unwrap();
    }
    worktree::add(&repo, &[], false).unwrap();
    ops::commit(&repo, CommitOptions { message: "seed".into(), ..Default::default() }).unwrap();

    let t = Instant::now();
    for i in 0..n {
        let rel = format!("src/mod{}/file{}.rs", i % 50, i % 2000);
        std::fs::write(root.join(&rel), format!("// file {i} edited {}\n", t.elapsed().as_nanos())).unwrap();
        worktree::add(&repo, &[rel], false).unwrap();
        ops::commit(&repo, CommitOptions { message: format!("micro {i}"), ..Default::default() }).unwrap();
    }
    let el = t.elapsed();
    println!("{n} micro-commits (write+add+commit, 2000-file tree) in {:.2?} = {:.0} commits/s", el, n as f64 / el.as_secs_f64());

    let t = Instant::now();
    for _ in 0..20 {
        worktree::status(&repo, true).unwrap();
    }
    println!("status on 2000 files: {:.2?} per call", t.elapsed() / 20);
}
