//! Server load test: N agents push micro-commits concurrently to an
//! in-process agit-server. Reports commits per second end to end (local
//! commit + object upload + CAS ref update) and for raw ref updates.
//!
//! `cargo run --release -p agit-server --example load -- <agents> <commits_per_agent>`

#[path = "../src/api.rs"]
#[allow(dead_code)]
mod api;
#[path = "../src/auth.rs"]
#[allow(dead_code)]
mod auth;
#[path = "../src/error.rs"]
#[allow(dead_code)]
mod error;
#[path = "../src/keys.rs"]
#[allow(dead_code)]
mod keys;
#[path = "../src/knowledge.rs"]
#[allow(dead_code)]
mod knowledge;
#[path = "../src/policy.rs"]
#[allow(dead_code)]
mod policy;
#[path = "../src/pulls.rs"]
#[allow(dead_code)]
mod pulls;
#[path = "../src/state.rs"]
#[allow(dead_code)]
mod state;
#[path = "../src/webhooks.rs"]
#[allow(dead_code)]
mod webhooks;

use std::sync::Arc;
use std::time::Instant;

use agit_core::ops::{self, CommitOptions};
use agit_core::worktree;
use agit_core::{AgentInfo, Repository};
use agit_protocol::sync;
use agit_protocol::types::{ExpectSpec, RefUpdate, RefUpdateRequest};
use agit_protocol::Client;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let agents: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(8);
    let per_agent: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(50);
    let tmp = tempfile::tempdir().unwrap();
    let st = Arc::new(state::AppState::open(tmp.path().join("data"), None).unwrap());
    let app = api::router(st, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let base = format!("http://{addr}/api/v1");
    let root = tmp.path().to_path_buf();

    // Phase 1: every agent on its own branch, full push path per commit.
    let t = Instant::now();
    let handles: Vec<_> = (0..agents)
        .map(|i| {
            let base = base.clone();
            let root = root.clone();
            tokio::task::spawn_blocking(move || {
                let c = Client::new(&base).with_agent(Some(format!("agent-{i}")));
                if i == 0 {
                    c.create_repo("load", Some("main")).unwrap();
                }
                while c.repo("load").is_err() {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                let repo = Repository::init(&root.join(format!("a{i}")))?;
                std::fs::write(repo.work_dir().join("seed.txt"), "seed\n")?;
                worktree::add(&repo, &[], false)?;
                ops::commit(
                    &repo,
                    CommitOptions {
                        message: "seed".into(),
                        ..Default::default()
                    },
                )?;
                ops::create_branch(&repo, &format!("agent-{i}"), None)?;
                ops::switch(&repo, &format!("agent-{i}"), false, false)?;
                sync::add_remote(&repo, "origin", &format!("{base}/repos/load"))?;
                for j in 0..per_agent {
                    std::fs::write(
                        repo.work_dir().join(format!("f{i}.txt")),
                        format!("{i}-{j}\n"),
                    )?;
                    worktree::add(&repo, &[], false)?;
                    ops::commit(
                        &repo,
                        CommitOptions {
                            message: format!("micro {j}"),
                            intent: Some("load".into()),
                            agent: Some(AgentInfo {
                                id: format!("agent-{i}"),
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                    )?;
                    sync::push(&repo, "origin", &format!("agent-{i}"), false, false)?;
                }
                Ok::<_, agit_core::Error>(())
            })
        })
        .collect();
    for h in handles {
        h.await.unwrap().unwrap();
    }
    let total = agents * per_agent;
    let el = t.elapsed();
    println!("phase 1 (commit + push per micro-commit, {agents} agents x {per_agent}): {total} in {el:.2?} = {:.0} commits/s end to end", total as f64 / el.as_secs_f64());

    // Phase 2: raw CAS ref updates from many clients advancing one branch
    // along a linear history, one commit per update (objects already
    // present): measures the server's ref commit path with group commit.
    let c = Client::new(&base);
    let history: Vec<_> = c
        .get_json_public::<Vec<agit_protocol::types::CommitEntry>>(
            "/repos/load/commits?rev=agent-0&limit=1000",
        )
        .unwrap()
        .into_iter()
        .map(|e| e.id)
        .rev()
        .collect();
    c.update_refs(
        "load",
        &RefUpdateRequest {
            updates: vec![RefUpdate {
                name: "refs/heads/hot".into(),
                expect: ExpectSpec::Any(true),
                new: Some(history[0]),
            }],
            message: None,
        },
    )
    .unwrap();
    let t = Instant::now();
    let total_steps = history.len() - 1;
    let handles: Vec<_> = (0..agents)
        .map(|i| {
            let base = base.clone();
            let history = history.clone();
            tokio::task::spawn_blocking(move || {
                let c = Client::new(&base).with_agent(Some(format!("agent-{i}")));
                let (mut conflicts, mut done) = (0usize, 0usize);
                loop {
                    let current = c.refs("load", "refs/heads/hot").unwrap()[0].id;
                    let Some(pos) = history.iter().position(|h| *h == current) else {
                        break;
                    };
                    if pos + 1 >= history.len() {
                        break;
                    }
                    match c.update_refs(
                        "load",
                        &RefUpdateRequest {
                            updates: vec![RefUpdate {
                                name: "refs/heads/hot".into(),
                                expect: ExpectSpec::Is(current),
                                new: Some(history[pos + 1]),
                            }],
                            message: None,
                        },
                    ) {
                        Ok(_) => done += 1,
                        Err(agit_core::Error::RefConflict { .. }) => conflicts += 1,
                        Err(e) => panic!("{e}"),
                    }
                }
                (done, conflicts)
            })
        })
        .collect();
    let (mut done, mut conflicts) = (0, 0);
    for h in handles {
        let (d, c) = h.await.unwrap();
        done += d;
        conflicts += c;
    }
    let el = t.elapsed();
    println!("phase 2 (CAS ref updates advancing one branch, {agents} clients): {done} of {total_steps} in {el:.2?} = {:.0} updates/s, {conflicts} ref_conflict retries", done as f64 / el.as_secs_f64());

    // Phase 3: independent branches, no conflicts: pure group-commit
    // throughput of the server's ref path.
    let t = Instant::now();
    let steps = history.len() - 1;
    let handles: Vec<_> = (0..agents)
        .map(|i| {
            let base = base.clone();
            let history = history.clone();
            tokio::task::spawn_blocking(move || {
                let c = Client::new(&base).with_agent(Some(format!("agent-{i}")));
                let name = format!("refs/heads/lane-{i}");
                let mut prev: Option<agit_core::ObjectId> = None;
                for h in history.iter() {
                    c.update_refs(
                        "load",
                        &RefUpdateRequest {
                            updates: vec![RefUpdate {
                                name: name.clone(),
                                expect: match prev {
                                    Some(p) => ExpectSpec::Is(p),
                                    None => ExpectSpec::Absent(true),
                                },
                                new: Some(*h),
                            }],
                            message: None,
                        },
                    )
                    .unwrap();
                    prev = Some(*h);
                }
            })
        })
        .collect();
    for h in handles {
        h.await.unwrap();
    }
    let el = t.elapsed();
    let total = agents * (steps + 1);
    println!("phase 3 (CAS ref updates on independent branches, {agents} clients): {total} in {el:.2?} = {:.0} updates/s", total as f64 / el.as_secs_f64());
}
