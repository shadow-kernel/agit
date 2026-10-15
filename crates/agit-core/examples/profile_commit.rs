use std::time::Instant;
use agit_core::ops::{self, CommitOptions};
use agit_core::worktree;
use agit_core::Repository;

fn main() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(&dir.path().join("r")).unwrap();
    let root = repo.work_dir().to_path_buf();
    for i in 0..2000 {
        let p = root.join(format!("src/mod{}/file{}.rs", i % 50, i));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, format!("// file {i}\nfn f{i}() {{}}\n")).unwrap();
    }
    worktree::add(&repo, &[], false).unwrap();
    ops::commit(&repo, CommitOptions { message: "seed".into(), ..Default::default() }).unwrap();
    let n = 300;
    let mut t_add = 0f64; let mut t_commit = 0f64;
    let mut t_load = 0f64; let mut t_wt = 0f64; let mut t_diff = 0f64; let mut t_ev = 0f64; let mut t_author = 0f64;
    for i in 0..n {
        let rel = format!("src/mod{}/file{}.rs", i % 50, i % 2000);
        std::fs::write(root.join(&rel), format!("// edited {i}\n")).unwrap();
        let t = Instant::now(); worktree::add(&repo, &[rel], false).unwrap(); t_add += t.elapsed().as_secs_f64();
        let t = Instant::now(); let idx = repo.load_index().unwrap(); t_load += t.elapsed().as_secs_f64();
        let t = Instant::now(); let tree = idx.write_tree(repo.store()).unwrap(); t_wt += t.elapsed().as_secs_f64();
        let t = Instant::now(); let ht = repo.head_tree().unwrap(); agit_core::diff::diff_trees(repo.store(), ht.as_ref(), Some(&tree)).unwrap(); t_diff += t.elapsed().as_secs_f64();
        let t = Instant::now(); repo.author().unwrap(); t_author += t.elapsed().as_secs_f64();
        let t = Instant::now(); repo.events().append("x", None, None, serde_json::json!({})).unwrap(); t_ev += t.elapsed().as_secs_f64();
        let t = Instant::now(); ops::commit(&repo, CommitOptions { message: format!("m{i}"), ..Default::default() }).unwrap(); t_commit += t.elapsed().as_secs_f64();
    }
    let f = 1000.0 / n as f64;
    println!("per op (ms): add {:.2} | commit {:.2} || load_index {:.2} write_tree {:.2} diff_trees {:.2} author {:.2} event {:.2}",
        t_add*f, t_commit*f, t_load*f, t_wt*f, t_diff*f, t_author*f, t_ev*f);
}
