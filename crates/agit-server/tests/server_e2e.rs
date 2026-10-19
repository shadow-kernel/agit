//! Server + client end-to-end: push, clone, pull, pull requests, knowledge.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use agit_core::ops::{self, CommitOptions};
use agit_core::worktree;
use agit_core::{AgentInfo, Repository};
use agit_protocol::sync;
use agit_protocol::types::*;
use agit_protocol::Client;

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

fn write(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, content).unwrap();
}

fn commit_all(repo: &Repository, msg: &str, agent: &str) -> ops::CommitResult {
    worktree::add(repo, &[], false).unwrap();
    ops::commit(
        repo,
        CommitOptions {
            message: msg.into(),
            intent: Some(format!("intent: {msg}")),
            agent: Some(AgentInfo {
                id: agent.into(),
                model: Some("test-model".into()),
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .unwrap()
}

async fn start_server(data: &Path) -> String {
    let st = Arc::new(state::AppState::open(data.to_path_buf(), None).unwrap());
    let app = api::router(st, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}/api/v1")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn push_clone_pull_pr_knowledge() {
    let tmp = tempfile::tempdir().unwrap();
    let base = start_server(&tmp.path().join("data")).await;
    let tmp_path = tmp.path().to_path_buf();

    tokio::task::spawn_blocking(move || {
        let client = Client::new(&base);
        assert_eq!(client.health().unwrap().status, "ok");
        client.create_repo("demo", Some("main")).unwrap();
        let url = format!("{base}/repos/demo");

        // Agent A: local repo, two commits, push (creates the remote branch).
        let a = Repository::init(&tmp_path.join("a")).unwrap();
        write(a.work_dir(), "README.md", "# demo\n");
        write(a.work_dir(), "src/lib.rs", "pub fn one() -> u32 { 1 }\n");
        commit_all(&a, "initial", "agent-a");
        write(
            a.work_dir(),
            "src/lib.rs",
            "pub fn one() -> u32 { 1 }\npub fn two() -> u32 { 2 }\n",
        );
        let c2 = commit_all(&a, "add two", "agent-a");
        sync::add_remote(&a, "origin", &url).unwrap();
        let p = sync::push(&a, "origin", "main", false, false).unwrap();
        assert!(!p.up_to_date);
        assert!(p.objects_sent >= 6);
        assert_eq!(p.new, c2.id);
        let p = sync::push(&a, "origin", "main", false, false).unwrap();
        assert!(p.up_to_date);
        assert_eq!(client.repo("demo").unwrap().head, Some(c2.id));

        // Agent B: clone, branch, change, push, open PR.
        let (b, f) = sync::clone(&url, &tmp_path.join("b")).unwrap();
        assert!(f.objects_received >= 6);
        assert_eq!(
            fs::read_to_string(b.work_dir().join("README.md")).unwrap(),
            "# demo\n"
        );
        assert_eq!(b.head_commit().unwrap(), Some(c2.id));
        ops::switch(&b, "feature/three", true, false).unwrap();
        write(
            b.work_dir(),
            "src/three.rs",
            "pub fn three() -> u32 { 3 }\n",
        );
        let c3 = commit_all(&b, "add three", "agent-b");
        sync::push(&b, "origin", "feature/three", false, false).unwrap();
        let bclient = Client::new(&base).with_agent(Some("agent-b".into()));
        let pr = bclient
            .create_pull(
                "demo",
                &PullCreate {
                    title: "Add three".into(),
                    body: Some("adds three()".into()),
                    intent: Some("complete numbers".into()),
                    task: Some("T-3".into()),
                    source: "feature/three".into(),
                    target: None,
                },
            )
            .unwrap();
        assert_eq!(pr.number, 1);
        assert_eq!(pr.author.id, "agent-b");
        let d = bclient.pull("demo", 1).unwrap();
        assert!(d.mergeable);
        assert_eq!(d.commits.len(), 1);
        assert_eq!(d.commits[0].id, c3.id);
        assert_eq!(d.changes.len(), 1);
        assert_eq!(d.changes[0].path, "src/three.rs");

        // Meanwhile agent A moves main forward (so the PR needs a real merge).
        write(a.work_dir(), "README.md", "# demo\n\nmore\n");
        let c4 = commit_all(&a, "readme", "agent-a");
        sync::push(&a, "origin", "main", false, false).unwrap();
        // Stale push from B's clone of main must be rejected as non-fast-forward.
        ops::switch(&b, "main", false, false).unwrap();
        write(b.work_dir(), "other.txt", "x\n");
        commit_all(&b, "stale", "agent-b");
        let err = sync::push(&b, "origin", "main", false, false).unwrap_err();
        assert!(err.to_string().contains("non-fast-forward"), "{err}");

        // Review + merge the PR server-side.
        let reviewer = Client::new(&base).with_agent(Some("reviewer-1".into()));
        reviewer
            .comment_pull(
                "demo",
                1,
                &CommentCreate {
                    path: Some("src/three.rs".into()),
                    line: Some(1),
                    kind: CommentKind::Suggestion,
                    body: "consider const".into(),
                },
            )
            .unwrap();
        let pr = reviewer
            .review_pull(
                "demo",
                1,
                &ReviewCreate {
                    decision: ReviewDecision::Approve,
                    body: "lgtm".into(),
                },
            )
            .unwrap();
        assert_eq!(pr.reviews.len(), 1);
        let m = reviewer
            .merge_pull("demo", 1, &MergeRequest::default())
            .unwrap();
        assert_eq!(m.pr.state, PullState::Merged);
        let merged = m.merged_commit;
        assert_eq!(client.repo("demo").unwrap().head, Some(merged));
        assert!(reviewer
            .merge_pull("demo", 1, &MergeRequest::default())
            .is_err());

        // Agent A pulls the merge: fast-forward over c4.
        let (_f, mr) = sync::pull(&a, "origin", Default::default()).unwrap();
        assert!(matches!(mr, ops::MergeResult::FastForward { to, .. } if to == merged));
        assert!(a.work_dir().join("src/three.rs").exists());
        let head = a.store().read_commit(&merged).unwrap();
        assert_eq!(head.parents, vec![c4.id, c3.id]);
        assert_eq!(head.meta.get("agit.pull").and_then(|v| v.as_u64()), Some(1));

        // Agent B pulls main: its stale commit gets merged with the remote.
        let (_f, mr) = sync::pull(&b, "origin", Default::default()).unwrap();
        assert!(matches!(mr, ops::MergeResult::Merged { .. }));
        assert!(b.work_dir().join("src/three.rs").exists());

        // Knowledge: rules, notes, context.
        let kclient = Client::new(&base).with_agent(Some("agent-a".into()));
        let rules: Vec<TagRule> = ureq::put(&format!("{base}/repos/demo/knowledge/rules"))
            .set("X-Agit-Agent", "agent-a")
            .send_json(
                serde_json::json!([{ "pattern": "src/**", "tags": ["area:core", "lang:rust"] }]),
            )
            .unwrap()
            .into_json()
            .unwrap();
        assert_eq!(rules.len(), 1);
        let n = kclient
            .create_note(
                "demo",
                &NoteCreate {
                    title: "numbers are u32".into(),
                    body: "All number helpers return u32 on purpose.".into(),
                    tags: vec!["area:core".into(), "kind:decision".into()],
                    paths: vec!["src/**".into()],
                    commits: vec![c2.id],
                    pulls: vec![1],
                    confidence: Some(0.9),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(kclient
            .create_note(
                "demo",
                &NoteCreate {
                    title: "bad".into(),
                    tags: vec!["Bad Tag".into()],
                    ..Default::default()
                }
            )
            .is_err());
        let found = kclient.notes("demo", "?tags=kind:decision").unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, n.id);
        assert!(kclient
            .notes("demo", "?tags=kind:gotcha")
            .unwrap()
            .is_empty());
        let ctx = kclient.context("demo", "src/lib.rs").unwrap();
        assert_eq!(ctx.notes.len(), 1);
        assert_eq!(ctx.suggested_tags, vec!["area:core", "lang:rust"]);
        assert!(ctx.recent_commits.iter().any(|c| c.id == c2.id));
        assert_eq!(kclient.tags("demo").unwrap().len(), 2);

        // Leases through the server.
        let l = kclient
            .claim_lease(
                "demo",
                &LeaseCreate {
                    holder: "agent-a".into(),
                    paths: vec!["src/lib.rs".into()],
                    mode: Default::default(),
                    ttl_secs: Some(60),
                    note: None,
                },
            )
            .unwrap();
        let err = bclient
            .claim_lease(
                "demo",
                &LeaseCreate {
                    holder: "agent-b".into(),
                    paths: vec!["src".into()],
                    mode: Default::default(),
                    ttl_secs: Some(60),
                    note: None,
                },
            )
            .unwrap_err();
        assert!(matches!(err, agit_core::Error::LeaseConflict { .. }));
        kclient.release_lease("demo", &l.id).unwrap();

        // Events recorded on the server.
        let events = client.events("demo", 0).unwrap();
        let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
        for k in [
            "repo.create",
            "ref.update",
            "pull.open",
            "pull.review",
            "pull.merge",
            "note.create",
            "lease.claim",
            "lease.release",
        ] {
            assert!(kinds.contains(&k), "missing event {k} in {kinds:?}");
        }

        // Browsing endpoints.
        let commits: Vec<CommitEntry> =
            client_get(&base, "/repos/demo/commits?limit=10&agent=agent-b");
        assert!(commits
            .iter()
            .all(|c| c.commit.agent.as_ref().unwrap().id == "agent-b"));
        let tree: TreeResponse = client_get(&base, "/repos/demo/tree/main?path=src");
        match tree {
            TreeResponse::Tree { entries, .. } => {
                assert!(entries.iter().any(|e| e.name == "three.rs"))
            }
            _ => panic!("expected tree"),
        }
        let blob: TreeResponse = client_get(&base, "/repos/demo/tree/main?path=src/three.rs");
        match blob {
            TreeResponse::Blob { text, .. } => assert!(text.unwrap().contains("three")),
            _ => panic!("expected blob"),
        }
        let agents: Vec<AgentActivity> = client_get(&base, "/repos/demo/agents");
        assert!(agents.iter().any(|a| a.id == "agent-a" && a.commits >= 3));
    })
    .await
    .unwrap();
}

fn client_get<T: serde::de::DeserializeOwned>(base: &str, path: &str) -> T {
    ureq::get(&format!("{base}{path}"))
        .call()
        .unwrap()
        .into_json()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn policies_are_enforced() {
    let tmp = tempfile::tempdir().unwrap();
    let base = start_server(&tmp.path().join("data")).await;
    let tmp_path = tmp.path().to_path_buf();
    tokio::task::spawn_blocking(move || {
        let admin = Client::new(&base).with_agent(Some("admin".into()));
        admin.create_repo("p", Some("main")).unwrap();
        let url = format!("{base}/repos/p");
        let a = Repository::init(&tmp_path.join("a")).unwrap();
        write(a.work_dir(), "f.txt", "1\n");
        commit_all(&a, "one", "agent-a");
        sync::add_remote(&a, "origin", &url).unwrap();
        sync::push(&a, "origin", "main", false, false).unwrap();

        let policy = Policy {
            protected_branches: vec!["main".into()],
            require_pull_request: true,
            require_approvals: 1,
            require_intent: true,
            ..Default::default()
        };
        admin.set_policy("p", &policy).unwrap();
        assert_eq!(admin.policy("p").unwrap(), policy);

        // Direct push to main is now refused.
        write(a.work_dir(), "f.txt", "2\n");
        commit_all(&a, "two", "agent-a");
        let err = sync::push(&a, "origin", "main", false, false).unwrap_err();
        assert!(matches!(err, agit_core::Error::PolicyViolation(_)), "{err}");

        // Feature branch + PR works; author approval does not count.
        ops::create_branch(&a, "feat", None).unwrap();
        sync::push(&a, "origin", "feat", false, false).unwrap();
        let ca = Client::new(&base).with_agent(Some("agent-a".into()));
        let pr = ca
            .create_pull(
                "p",
                &PullCreate {
                    title: "two".into(),
                    body: None,
                    intent: None,
                    task: None,
                    source: "feat".into(),
                    target: None,
                },
            )
            .unwrap();
        let err = ca
            .merge_pull("p", pr.number, &MergeRequest::default())
            .unwrap_err();
        assert!(err.to_string().contains("approval"), "{err}");
        ca.review_pull(
            "p",
            pr.number,
            &ReviewCreate {
                decision: ReviewDecision::Approve,
                body: String::new(),
            },
        )
        .unwrap();
        let err = ca
            .merge_pull("p", pr.number, &MergeRequest::default())
            .unwrap_err();
        assert!(err.to_string().contains("approval"), "{err}");
        let rv = Client::new(&base).with_agent(Some("reviewer".into()));
        rv.comment_pull(
            "p",
            pr.number,
            &CommentCreate {
                path: None,
                line: None,
                kind: CommentKind::Blocker,
                body: "wait".into(),
            },
        )
        .unwrap();
        rv.review_pull(
            "p",
            pr.number,
            &ReviewCreate {
                decision: ReviewDecision::Approve,
                body: String::new(),
            },
        )
        .unwrap();
        let err = ca
            .merge_pull("p", pr.number, &MergeRequest::default())
            .unwrap_err();
        assert!(err.to_string().contains("blocker"), "{err}");
        // Resolve the blocker, then merge succeeds (commits carry intent).
        let d = ca.pull("p", pr.number).unwrap();
        let cid = d.pr.comments[0].id;
        ureq::post(&format!(
            "{base}/repos/p/pulls/{}/comments/{cid}/resolve",
            pr.number
        ))
        .set("X-Agit-Agent", "reviewer")
        .call()
        .unwrap();
        ca.merge_pull("p", pr.number, &MergeRequest::default())
            .unwrap();

        // A commit without intent is rejected on a protected branch (via PR too).
        ops::switch(&a, "feat", false, false).unwrap();
        write(a.work_dir(), "g.txt", "x\n");
        worktree::add(&a, &[], false).unwrap();
        ops::commit(
            &a,
            CommitOptions {
                message: "no intent".into(),
                ..Default::default()
            },
        )
        .unwrap();
        sync::push(&a, "origin", "feat", false, false).unwrap();
        let pr2 = ca
            .create_pull(
                "p",
                &PullCreate {
                    title: "three".into(),
                    body: None,
                    intent: None,
                    task: None,
                    source: "feat".into(),
                    target: None,
                },
            )
            .unwrap();
        rv.review_pull(
            "p",
            pr2.number,
            &ReviewCreate {
                decision: ReviewDecision::Approve,
                body: String::new(),
            },
        )
        .unwrap();
        let err = ca
            .merge_pull("p", pr2.number, &MergeRequest::default())
            .unwrap_err();
        assert!(err.to_string().contains("intent"), "{err}");
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn webhooks_deliver_signed_events() {
    use axum::{routing::post, Router};
    type Received = Arc<std::sync::Mutex<Vec<(axum::http::HeaderMap, Vec<u8>)>>>;
    let received: Received = Arc::new(std::sync::Mutex::new(vec![]));
    let rx = received.clone();
    let app = Router::new().route(
        "/hook",
        post(
            move |headers: axum::http::HeaderMap, body: axum::body::Bytes| {
                let rx = rx.clone();
                async move {
                    rx.lock().unwrap().push((headers, body.to_vec()));
                    "ok"
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let hook_url = format!("http://{}/hook", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let tmp = tempfile::tempdir().unwrap();
    let base = start_server(&tmp.path().join("data")).await;
    let tmp_path = tmp.path().to_path_buf();
    let received2 = received.clone();
    tokio::task::spawn_blocking(move || {
        let c = Client::new(&base).with_agent(Some("ops".into()));
        c.create_repo("w", Some("main")).unwrap();
        let wh = c
            .create_webhook(
                "w",
                &WebhookCreate {
                    url: hook_url.clone(),
                    kinds: vec!["ref.*".into(), "pull.open".into()],
                    secret: Some("s3cret".into()),
                },
            )
            .unwrap();
        assert!(wh.has_secret);
        assert_eq!(c.webhooks("w").unwrap().len(), 1);

        let a = Repository::init(&tmp_path.join("a")).unwrap();
        write(a.work_dir(), "f.txt", "1\n");
        commit_all(&a, "one", "agent-a");
        sync::add_remote(&a, "origin", &format!("{base}/repos/w")).unwrap();
        sync::push(&a, "origin", "main", false, false).unwrap(); // emits ref.update
        c.claim_lease(
            "w",
            &LeaseCreate {
                holder: "x".into(),
                paths: vec!["f.txt".into()],
                mode: Default::default(),
                ttl_secs: Some(10),
                note: None,
            },
        )
        .unwrap(); // lease.claim: filtered out

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let n = received2.lock().unwrap().len();
            if n >= 1 || std::time::Instant::now() > deadline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
        let got = received2.lock().unwrap().clone();
        assert_eq!(
            got.len(),
            1,
            "expected exactly one matching delivery, got {}",
            got.len()
        );
        let (headers, body) = &got[0];
        assert_eq!(headers.get("x-agit-event").unwrap(), "ref.update");
        assert_eq!(headers.get("x-agit-repo").unwrap(), "w");
        let sig = headers
            .get("x-agit-signature")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert_eq!(sig, webhooks::sign("s3cret", body));
        let payload: serde_json::Value = serde_json::from_slice(body).unwrap();
        assert_eq!(payload["event"]["kind"], "ref.update");
        assert_eq!(payload["event"]["data"]["ref"], "refs/heads/main");

        c.delete_webhook("w", &wh.id).unwrap();
        assert!(c.webhooks("w").unwrap().is_empty());
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn signed_commits_and_key_registry() {
    use agit_core::sign::{Keypair, SignatureStatus};
    let tmp = tempfile::tempdir().unwrap();
    let base = start_server(&tmp.path().join("data")).await;
    let tmp_path = tmp.path().to_path_buf();
    tokio::task::spawn_blocking(move || {
        agit_core::sign::set_keys_dir(Some(tmp_path.join("keys")));
        let admin = Client::new(&base).with_agent(Some("admin".into()));
        admin.create_repo("s", Some("main")).unwrap();
        let url = format!("{base}/repos/s");
        let a = Repository::init(&tmp_path.join("a")).unwrap();
        write(a.work_dir(), "f.txt", "1\n");
        commit_all(&a, "one", "agent-a");
        sync::add_remote(&a, "origin", &url).unwrap();
        sync::push(&a, "origin", "main", false, false).unwrap();
        admin
            .set_policy(
                "s",
                &Policy {
                    protected_branches: vec!["main".into()],
                    require_signed: true,
                    ..Default::default()
                },
            )
            .unwrap();

        // Unsigned commit is refused on the protected branch.
        write(a.work_dir(), "f.txt", "2\n");
        commit_all(&a, "two", "agent-a");
        let err = sync::push(&a, "origin", "main", false, false).unwrap_err();
        assert!(err.to_string().contains("not signed"), "{err}");

        // Signed but unregistered key is refused too.
        let kp = Keypair::generate("agent-a");
        kp.save().unwrap();
        ops::reset(&a, "HEAD~1", ops::ResetMode::Soft).unwrap();
        let signed = ops::commit(
            &a,
            CommitOptions {
                message: "two signed".into(),
                intent: Some("i".into()),
                agent: Some(AgentInfo {
                    id: "agent-a".into(),
                    ..Default::default()
                }),
                sign_with: Some("agent-a".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let c = a.store().read_commit(&signed.id).unwrap();
        assert!(matches!(
            agit_core::sign::verify_commit(&c),
            SignatureStatus::Valid { .. }
        ));
        let err = sync::push(&a, "origin", "main", false, false).unwrap_err();
        assert!(err.to_string().contains("not registered"), "{err}");

        // Register the key for agent-a: push succeeds; detail shows trusted.
        let k = admin
            .register_key("s", "agent-a", &kp.public_key_hex())
            .unwrap();
        assert_eq!(k.key_id, kp.key_id());
        assert!(admin
            .register_key("s", "agent-b", &kp.public_key_hex())
            .is_err());
        sync::push(&a, "origin", "main", false, false).unwrap();
        let d = admin.commit_detail("s", "main").unwrap();
        let sig = d.signature.unwrap();
        assert!(sig.trusted);
        assert!(
            matches!(sig.status, SignatureStatus::Valid { ref signer, .. } if signer == "agent-a")
        );

        // A key registered for another agent does not make agent-b's commits trusted.
        write(a.work_dir(), "f.txt", "3\n");
        worktree::add(&a, &[], false).unwrap();
        ops::commit(
            &a,
            CommitOptions {
                message: "three".into(),
                agent: Some(AgentInfo {
                    id: "agent-b".into(),
                    ..Default::default()
                }),
                sign_with: Some("agent-a".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let err = sync::push(&a, "origin", "main", false, false).unwrap_err();
        assert!(
            err.to_string().contains("not registered for agent-b"),
            "{err}"
        );

        admin.delete_key("s", &k.key_id).unwrap();
        assert!(admin.keys("s").unwrap().is_empty());
        agit_core::sign::set_keys_dir(None);
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn knowledge_layer() {
    let tmp = tempfile::tempdir().unwrap();
    let base = start_server(&tmp.path().join("data")).await;
    let tmp_path = tmp.path().to_path_buf();
    tokio::task::spawn_blocking(move || {
        let c = Client::new(&base).with_agent(Some("agent-a".into()));
        c.create_repo("k", Some("main")).unwrap();
        let url = format!("{base}/repos/k");

        // One commit with a few files.
        let a = Repository::init(&tmp_path.join("a")).unwrap();
        write(a.work_dir(), "README.md", "# k\n");
        write(
            a.work_dir(),
            "src/auth/login.rs",
            &format!(
                "// login\n{}",
                "pub fn login() -> bool { true }\n".repeat(8)
            ),
        );
        write(a.work_dir(), "src/auth/session.rs", "pub struct Session;\n");
        write(
            a.work_dir(),
            "src/db/pool.rs",
            "pub const POOL: usize = 8;\n",
        );
        let c1 = commit_all(&a, "initial", "agent-a");
        sync::add_remote(&a, "origin", &url).unwrap();
        sync::push(&a, "origin", "main", false, false).unwrap();

        ureq::put(&format!("{base}/repos/k/knowledge/rules"))
            .set("X-Agit-Agent", "agent-a")
            .send_json(serde_json::json!([
                { "pattern": "src/auth/**", "tags": ["area:auth"] },
                { "pattern": "src/db/**", "tags": ["area:db"] }
            ]))
            .unwrap();

        // Notes default to valid_at = HEAD.
        let n1 = c
            .create_note(
                "k",
                &NoteCreate {
                    title: "Session tokens rotate nightly".into(),
                    body:
                        "The SessionStore calls rotate_token() every night; the gateway reads them."
                            .into(),
                    tags: vec!["area:auth".into(), "kind:gotcha".into()],
                    paths: vec!["src/auth/**".into()],
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(n1.valid_at, Some(c1.id));
        assert!(!n1.stale && !n1.draft);
        let n2 = c
            .create_note(
                "k",
                &NoteCreate {
                    title: "Connection pool sizing for the gateway".into(),
                    body: "The pool holds eight connections.".into(),
                    tags: vec!["area:db".into(), "kind:decision".into()],
                    paths: vec!["src/db/**".into()],
                    ..Default::default()
                },
            )
            .unwrap();
        let n3 = c
            .create_note(
                "k",
                &NoteCreate {
                    title: "Unrelated".into(),
                    body: "nothing to see".into(),
                    tags: vec!["kind:howto".into()],
                    commits: vec![c1.id],
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(c
            .create_note(
                "k",
                &NoteCreate {
                    title: "bad valid_at".into(),
                    valid_at: Some("00".repeat(32).parse().unwrap()),
                    ..Default::default()
                }
            )
            .is_err());

        // Full-text search: ranked, tokenized (camelCase, snake_case, tags).
        let hits = c.notes("k", "?q=rotate").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, n1.id);
        assert!(hits[0].score.unwrap() > 0.0);
        assert_eq!(c.notes("k", "?q=session%20store").unwrap()[0].id, n1.id);
        assert_eq!(c.notes("k", "?q=SessionStore").unwrap()[0].id, n1.id);
        assert_eq!(c.notes("k", "?q=area:auth").unwrap()[0].id, n1.id);
        let hits = c.notes("k", "?q=pool").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, n2.id);
        assert!(c.notes("k", "?q=pool&tags=area:auth").unwrap().is_empty());
        assert!(c.notes("k", "?tags=area:auth").unwrap()[0].score.is_none());
        // Title hits outrank body hits.
        let hits = c.notes("k", "?q=gateway").unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].id, n2.id);
        assert!(hits[0].score.unwrap() > hits[1].score.unwrap());
        // The index follows updates and deletes.
        c.patch_note(
            "k",
            &n2.id,
            &NotePatch {
                body: Some("The pool rotates idle connections.".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(c.notes("k", "?q=rotate").unwrap().len(), 2);
        assert_eq!(c.notes("k", "?q=nothing").unwrap().len(), 1);
        c.delete_note("k", &n3.id).unwrap();
        assert!(c.notes("k", "?q=nothing").unwrap().is_empty());
        assert!(c.note("k", &n3.id).is_err());

        // Linking filters.
        let n4 = c
            .create_note(
                "k",
                &NoteCreate {
                    title: "About the first commit".into(),
                    commits: vec![c1.id],
                    ..Default::default()
                },
            )
            .unwrap();
        let linked = c.notes("k", &format!("?commit={}", c1.id)).unwrap();
        assert_eq!(linked.len(), 1);
        assert_eq!(linked[0].id, n4.id);
        let detail: CommitDetail = client_get(&base, &format!("/repos/k/commits/{}", c1.id));
        assert!(detail.notes.iter().any(|n| n.id == n4.id));

        // Staleness: change a covered path, note n1 becomes stale.
        assert!(c.stale_notes("k", None).unwrap().is_empty());
        write(
            a.work_dir(),
            "src/auth/login.rs",
            "pub fn login() -> bool { false }\n",
        );
        let c2 = commit_all(&a, "flip login", "agent-a");
        sync::push(&a, "origin", "main", false, false).unwrap();
        let stale = c.stale_notes("k", None).unwrap();
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].note.id, n1.id);
        assert!(stale[0].note.stale);
        assert_eq!(stale[0].changed_paths, vec!["src/auth/login.rs"]);
        assert!(c.note("k", &n1.id).unwrap().stale);
        assert!(!c.note("k", &n2.id).unwrap().stale);
        let detail: CommitDetail = client_get(&base, &format!("/repos/k/commits/{}", c2.id));
        assert!(detail.notes.iter().any(|n| n.id == n1.id));
        assert!(!detail.notes.iter().any(|n| n.id == n2.id));
        // Context marks stale notes and lists fresh ones first.
        let fresh = c
            .create_note(
                "k",
                &NoteCreate {
                    title: "Login returns false for now".into(),
                    paths: vec!["src/auth/login.rs".into()],
                    ..Default::default()
                },
            )
            .unwrap();
        let ctx = c.context("k", "src/auth/login.rs").unwrap();
        assert_eq!(ctx.notes.len(), 2);
        assert_eq!(ctx.notes[0].id, fresh.id);
        assert!(!ctx.notes[0].stale);
        assert_eq!(ctx.notes[1].id, n1.id);
        assert!(ctx.notes[1].stale);
        assert_eq!(ctx.agents_active, vec!["agent-a"]);
        assert!(ctx.files.is_empty());
        // Validate against HEAD (and explicitly against an older rev).
        let v = c.validate_note("k", &n1.id, None).unwrap();
        assert_eq!(v.valid_at, Some(c2.id));
        assert!(!v.stale);
        assert!(c.stale_notes("k", None).unwrap().is_empty());
        // Against the older commit both notes written at c2 are stale.
        assert_eq!(
            c.stale_notes("k", Some(&c1.id.to_string())).unwrap().len(),
            2
        );
        let v = c
            .validate_note("k", &n1.id, Some(&c1.id.to_string()))
            .unwrap();
        assert_eq!(v.valid_at, Some(c1.id));
        assert!(v.stale);

        // Context pack: files within a budget, agents from leases and commits.
        c.claim_lease(
            "k",
            &LeaseCreate {
                holder: "agent-z".into(),
                paths: vec!["src/auth".into()],
                mode: Default::default(),
                ttl_secs: Some(60),
                note: None,
            },
        )
        .unwrap();
        let pack = c.context_pack("k", "src/auth/login.rs", None).unwrap();
        assert_eq!(pack.agents_active, vec!["agent-a", "agent-z"]);
        assert_eq!(pack.files.len(), 2);
        assert_eq!(pack.files[0].path, "src/auth/login.rs");
        assert!(pack.files[0].text.contains("false"));
        assert!(!pack.files[0].truncated);
        assert_eq!(pack.files[1].path, "src/auth/session.rs");
        assert!(c
            .context_pack("k", "src/auth/login.rs", Some(1))
            .unwrap()
            .files
            .is_empty());
        let mut without_files = pack.clone();
        without_files.files.clear();
        let used = serde_json::to_string(&without_files).unwrap().len();
        let tight = c
            .context_pack("k", "src/auth/login.rs", Some(used / 4 + 20))
            .unwrap();
        assert_eq!(tight.files.len(), 1);
        assert!(tight.files[0].truncated);
        let dir_pack = c.context_pack("k", "src/db", None).unwrap();
        assert_eq!(dir_pack.files.len(), 1);
        assert_eq!(dir_pack.files[0].path, "src/db/pool.rs");

        // Drafts from merges.
        let (b, _) = sync::clone(&url, &tmp_path.join("b")).unwrap();
        ops::switch(&b, "feature/db", true, false).unwrap();
        write(
            b.work_dir(),
            "src/db/pool.rs",
            "pub const POOL: usize = 16;\n",
        );
        write(b.work_dir(), "src/db/migrate.rs", "pub fn migrate() {}\n");
        write(b.work_dir(), "CHANGELOG.md", "- bigger pool\n");
        commit_all(&b, "bigger pool", "agent-b");
        sync::push(&b, "origin", "feature/db", false, false).unwrap();
        let bc = Client::new(&base).with_agent(Some("agent-b".into()));
        let pr = bc
            .create_pull(
                "k",
                &PullCreate {
                    title: "Double the pool".into(),
                    body: Some("More connections.".into()),
                    intent: Some("handle more load".into()),
                    task: Some("T-9".into()),
                    source: "feature/db".into(),
                    target: None,
                },
            )
            .unwrap();
        let rv = Client::new(&base).with_agent(Some("reviewer-1".into()));
        rv.review_pull(
            "k",
            pr.number,
            &ReviewCreate {
                decision: ReviewDecision::Approve,
                body: "fine".into(),
            },
        )
        .unwrap();
        let m = rv
            .merge_pull("k", pr.number, &MergeRequest::default())
            .unwrap();
        assert!(c
            .notes("k", &format!("?pull={}", pr.number))
            .unwrap()
            .is_empty());
        let drafts = c
            .notes("k", &format!("?drafts=1&pull={}", pr.number))
            .unwrap();
        assert_eq!(drafts.len(), 1);
        assert!(drafts[0].draft);
        let d = c.note("k", &drafts[0].id).unwrap();
        assert_eq!(d.title, "Double the pool");
        assert_eq!(d.author, "reviewer-1");
        assert_eq!(d.confidence, 0.4);
        assert_eq!(d.valid_at, Some(m.merged_commit));
        assert_eq!(d.commits, vec![m.merged_commit]);
        assert_eq!(d.pulls, vec![pr.number]);
        assert_eq!(d.tags, vec!["kind:change", "area:db"]);
        assert_eq!(d.paths, vec!["CHANGELOG.md", "src/db/**"]);
        for needle in [
            "Intent: handle more load",
            "Task: T-9",
            "- src/db/migrate.rs (A)",
            "- src/db/pool.rs (M)",
            "- reviewer-1: approve - fine",
        ] {
            assert!(d.body.contains(needle), "missing {needle:?} in {}", d.body);
        }
        // Drafts stay out of tag counts and context until accepted.
        assert!(!c.tags("k").unwrap().iter().any(|t| t.tag == "kind:change"));
        assert!(c
            .context("k", "src/db/pool.rs")
            .unwrap()
            .notes
            .iter()
            .all(|n| n.id != d.id));
        let detail: CommitDetail =
            client_get(&base, &format!("/repos/k/commits/{}", m.merged_commit));
        assert!(detail.notes.iter().any(|n| n.id == d.id));
        let accepted = c
            .accept_note(
                "k",
                &d.id,
                Some(&NotePatch {
                    confidence: Some(0.9),
                    ..Default::default()
                }),
            )
            .unwrap();
        assert!(!accepted.draft);
        assert_eq!(accepted.confidence, 0.9);
        assert_eq!(
            c.notes("k", &format!("?pull={}", pr.number)).unwrap().len(),
            1
        );
        assert!(c.tags("k").unwrap().iter().any(|t| t.tag == "kind:change"));
        assert_eq!(c.notes("k", "?q=double%20pool").unwrap()[0].id, d.id);

        let events = c.events("k", 0).unwrap();
        let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
        for k in [
            "note.create",
            "note.update",
            "note.delete",
            "note.validate",
            "note.draft",
            "note.accept",
        ] {
            assert!(kinds.contains(&k), "missing event {k} in {kinds:?}");
        }
    })
    .await
    .unwrap();
}

async fn start_server_with_token(data: &Path, token: &str) -> String {
    let st = Arc::new(state::AppState::open(data.to_path_buf(), Some(token.to_string())).unwrap());
    let app = api::router(st, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}/api/v1")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scoped_tokens_bind_identity_and_limit_access() {
    let tmp = tempfile::tempdir().unwrap();
    let base = start_server_with_token(&tmp.path().join("data"), "root-secret").await;
    let tmp_path = tmp.path().to_path_buf();
    tokio::task::spawn_blocking(move || {
        // No token: health is public, everything else is refused.
        let anon = Client::new(&base).with_token(Some("".into()));
        assert_eq!(
            ureq::get(&format!("{base}/health"))
                .call()
                .unwrap()
                .status(),
            200
        );
        let err = ureq::get(&format!("{base}/repos")).call().unwrap_err();
        assert!(matches!(err, ureq::Error::Status(401, _)));
        assert!(matches!(
            anon.list_repos().unwrap_err(),
            agit_core::Error::Unauthorized(_)
        ));

        let root = Client::new(&base).with_token(Some("root-secret".into()));
        assert!(root.whoami().unwrap().root);
        root.create_repo("t", Some("main")).unwrap();
        root.create_repo("other", Some("main")).unwrap();

        // A write token bound to agent-a on repo t.
        let created = root
            .create_token(&TokenCreate {
                name: "agent-a laptop".into(),
                agent: Some("agent-a".into()),
                scopes: vec![Scope {
                    repo: "t".into(),
                    access: Access::Write,
                }],
                ttl_secs: None,
            })
            .unwrap();
        assert!(created.token.starts_with("agit_"));
        let a = Client::new(&base)
            .with_token(Some(created.token.clone()))
            .with_agent(Some("someone-else".into()));
        let me = a.whoami().unwrap();
        assert_eq!(
            me.agent.as_deref(),
            Some("agent-a"),
            "bound identity must win over the header"
        );
        assert!(!me.root);
        assert_eq!(
            a.list_repos().unwrap().len(),
            1,
            "listing is filtered by scope"
        );

        // Acting as agent-a even though the header claims someone else.
        let l = a
            .claim_lease(
                "t",
                &LeaseCreate {
                    holder: "x".into(),
                    paths: vec!["src".into()],
                    mode: Default::default(),
                    ttl_secs: Some(60),
                    note: None,
                },
            )
            .unwrap();
        let ev = root.events("t", 0).unwrap();
        assert!(ev
            .iter()
            .any(|e| e.kind == "lease.claim" && e.actor.as_deref() == Some("agent-a")));
        a.release_lease("t", &l.id).unwrap();

        // Scope limits: no access to `other`, no admin on `t`.
        let r = a.repo("other");
        assert!(
            matches!(r, Err(agit_core::Error::Forbidden(_))),
            "expected forbidden, got {r:?}"
        );
        let r = a.set_policy("t", &Policy::default());
        assert!(
            matches!(r, Err(agit_core::Error::Forbidden(_))),
            "expected forbidden, got {r:?}"
        );
        assert!(matches!(
            a.create_token(&TokenCreate {
                name: "x".into(),
                agent: None,
                scopes: vec![Scope {
                    repo: "t".into(),
                    access: Access::Read
                }],
                ttl_secs: None
            }),
            Err(agit_core::Error::Forbidden(_))
        ));

        // Read-only token can read but not write.
        let ro = root
            .create_token(&TokenCreate {
                name: "reader".into(),
                agent: None,
                scopes: vec![Scope {
                    repo: "*".into(),
                    access: Access::Read,
                }],
                ttl_secs: Some(3600),
            })
            .unwrap();
        let r = Client::new(&base).with_token(Some(ro.token.clone()));
        match r.list_repos() {
            Ok(list) => assert_eq!(list.len(), 2, "reader token sees both repos"),
            Err(e) => panic!(
                "reader token rejected: {e:?} (tokens on server: {:?})",
                root.tokens()
            ),
        }
        assert!(matches!(
            r.claim_lease(
                "t",
                &LeaseCreate {
                    holder: "r".into(),
                    paths: vec!["a".into()],
                    mode: Default::default(),
                    ttl_secs: None,
                    note: None
                }
            ),
            Err(agit_core::Error::Forbidden(_))
        ));

        // Expired token is rejected; deleted token too.
        let exp = root
            .create_token(&TokenCreate {
                name: "short".into(),
                agent: None,
                scopes: vec![Scope {
                    repo: "*".into(),
                    access: Access::Read,
                }],
                ttl_secs: Some(0),
            })
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(Client::new(&base)
            .with_token(Some(exp.token))
            .list_repos()
            .is_err());
        assert_eq!(root.tokens().unwrap().len(), 3);
        root.delete_token(&created.info.id).unwrap();
        assert!(a.list_repos().is_err());

        // Audit log has entries.
        let audit = fs::read_to_string(tmp_path.join("data/audit.jsonl")).unwrap();
        assert!(audit.lines().count() >= 5);
        assert!(audit.contains("\"status\":403"));
        assert!(audit.contains("\"status\":401"));
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ssh_keys_open_sessions_and_scope_access() {
    use agit_core::sshkey::SshPrivateKey;
    use agit_protocol::sshauth;
    let tmp = tempfile::tempdir().unwrap();
    let tmp_path = tmp.path().to_path_buf();
    // Keep this test's sessions out of ~/.agit and never probe the developer's
    // own keys or ssh-agent: every client below names its key explicitly.
    sshauth::set_sessions_file(Some(tmp_path.join("sessions.json")));
    let gen = |name: &str| {
        let k =
            SshPrivateKey::parse(&agit_core::sign::Keypair::generate(name).secret_hex()).unwrap();
        let path = tmp_path.join(format!("{name}.key"));
        fs::write(&path, k.to_openssh().unwrap()).unwrap();
        (k, path)
    };
    let (admin_key, admin_path) = gen("admin");
    let (coder_key, coder_path) = gen("coder");
    let (_stranger_key, stranger_path) = gen("stranger");
    let mut admin_pub = admin_key.public().clone();
    admin_pub.comment = Some("ops laptop".into());
    // Closed server: no root token, one admin key.
    let st = Arc::new(
        state::AppState::open(tmp_path.join("data"), None)
            .unwrap()
            .with_admin_keys(vec![admin_pub]),
    );
    assert!(st.closed());
    let app = api::router(st, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let base = format!("http://{addr}/api/v1");

    tokio::task::spawn_blocking(move || {
        // Nothing works without credentials ...
        let err = ureq::get(&format!("{base}/repos")).call().unwrap_err();
        assert!(matches!(err, ureq::Error::Status(401, _)));
        // ... but a client with an admin key signs in on its own after the 401.
        let admin = Client::new(&base)
            .with_auto_login(true)
            .with_ssh_key(Some(admin_path.clone()));
        assert!(admin.list_repos().unwrap().is_empty());
        let me = admin.whoami().unwrap();
        assert_eq!(me.key.as_deref(), Some("admin"));
        assert!(me
            .scopes
            .iter()
            .any(|s| s.repo == "*" && s.access == Access::Admin));
        assert!(!me.root);
        admin.create_repo("t", Some("main")).unwrap();
        admin.create_repo("other", Some("main")).unwrap();
        // Admin sessions can hand out server-wide scopes.
        admin
            .create_token(&TokenCreate {
                name: "ci".into(),
                agent: None,
                scopes: vec![Scope {
                    repo: "*".into(),
                    access: Access::Read,
                }],
                ttl_secs: None,
            })
            .unwrap();

        // A stranger's key is refused with a message the client recognises.
        let stranger = Client::new(&base).with_auto_login(false);
        let err = sshauth::login_auto(&stranger, Some(&stranger_path), None).unwrap_err();
        assert!(matches!(err, agit_core::Error::Unauthorized(_)), "{err:?}");
        assert!(err.to_string().contains("not authorized"), "{err}");

        // Authorize coder-1 on repo t only.
        let key = admin
            .add_authorized_key(&AuthorizedKeyCreate {
                public_key: coder_key.public().to_openssh(),
                name: Some("coder laptop".into()),
                agent: Some("coder-1".into()),
                scopes: vec![Scope {
                    repo: "t".into(),
                    access: Access::Write,
                }],
            })
            .unwrap();
        assert_eq!(key.fingerprint, coder_key.public().fingerprint());
        assert_eq!(admin.authorized_keys().unwrap().len(), 1);
        let coder = Client::new(&base).with_auto_login(false);
        let (created, id) = coder
            .login_with_ssh_key(Some(&coder_path), Some(600))
            .unwrap();
        assert_eq!(id.public.fingerprint(), key.fingerprint);
        assert_eq!(created.info.key.as_deref(), Some(key.id.as_str()));
        let me = coder.whoami().unwrap();
        assert_eq!(me.agent.as_deref(), Some("coder-1"));
        assert_eq!(me.key.as_deref(), Some(key.id.as_str()));
        // Scopes limit what the key sees: only repository t.
        let names: Vec<String> = coder
            .list_repos()
            .unwrap()
            .into_iter()
            .map(|r| r.name)
            .collect();
        assert_eq!(names, vec!["t"]);
        assert!(matches!(
            coder.repo("other").unwrap_err(),
            agit_core::Error::Forbidden(_)
        ));
        assert!(coder.refs("t", "refs/heads/").is_ok());
        // Keys cannot be managed by a repository-scoped session.
        assert!(matches!(
            coder.authorized_keys().unwrap_err(),
            agit_core::Error::Forbidden(_)
        ));

        // The session is cached: a fresh client for the same server reuses it.
        let again = Client::new(&base).with_auto_login(false);
        assert_eq!(again.whoami().unwrap().agent.as_deref(), Some("coder-1"));

        // A challenge can be redeemed only once.
        let ch = coder.auth_challenge().unwrap();
        let sig = hex::encode(
            coder_key
                .sign(&agit_core::sshkey::auth_message(&ch.nonce))
                .unwrap(),
        );
        let req = SessionCreate {
            public_key: coder_key.public().to_openssh(),
            nonce: ch.nonce,
            signature: sig,
            ttl_secs: None,
        };
        coder.auth_session(&req).unwrap();
        assert!(matches!(
            coder.auth_session(&req).unwrap_err(),
            agit_core::Error::Forbidden(_)
        ));

        // Logout revokes the session token and forgets the cached session.
        coder.logout().unwrap();
        assert!(coder.token().is_none());
        assert!(sshauth::cached_session(&base).is_none());
        assert!(matches!(
            again.whoami().unwrap_err(),
            agit_core::Error::Unauthorized(_)
        ));

        // Deleting the key revokes every session opened with it.
        let coder2 = Client::new(&base).with_auto_login(false);
        coder2.login_with_ssh_key(Some(&coder_path), None).unwrap();
        assert!(coder2.whoami().is_ok());
        // Two sessions are still open for the key: the manually redeemed
        // challenge above and coder2's login.
        let r = admin.delete_authorized_key(&key.id).unwrap();
        assert_eq!(r["revoked_sessions"], 2);
        assert!(matches!(
            coder2.whoami().unwrap_err(),
            agit_core::Error::Unauthorized(_)
        ));
        assert!(admin.authorized_keys().unwrap().is_empty());
        sshauth::set_sessions_file(None);
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_knowledge_fallback_and_sync() {
    use agit_protocol::local_knowledge::LocalNotes;
    let tmp = tempfile::tempdir().unwrap();
    let base = start_server(&tmp.path().join("data")).await;
    let tmp_path = tmp.path().to_path_buf();
    tokio::task::spawn_blocking(move || {
        let c = Client::new(&base).with_agent(Some("agent-a".into()));
        c.create_repo("k2", Some("main")).unwrap();
        let a = Repository::init(&tmp_path.join("a")).unwrap();
        write(a.work_dir(), "src/x.rs", "x\n");
        commit_all(&a, "one", "agent-a");

        // Offline: notes land in .agit/knowledge and context works without a server.
        let local = LocalNotes::open(&a);
        let n = local
            .create(
                NoteCreate {
                    title: "local wisdom".into(),
                    body: "offline".into(),
                    tags: vec!["kind:gotcha".into()],
                    paths: vec!["src/**".into()],
                    ..Default::default()
                },
                "agent-a",
            )
            .unwrap();
        assert!(n.id.starts_with("n_local_"));
        assert_eq!(n.valid_at, a.head_commit().unwrap());
        local
            .set_rules(&[TagRule {
                pattern: "src/**".into(),
                tags: vec!["area:core".into()],
            }])
            .unwrap();
        let ctx = local.context("src/x.rs", 10).unwrap();
        assert_eq!(ctx.notes.len(), 1);
        assert_eq!(ctx.suggested_tags, vec!["area:core"]);
        assert_eq!(ctx.recent_commits.len(), 1);
        assert_eq!(ctx.agents_active, vec!["agent-a"]);
        assert_eq!(
            local
                .query(&["kind:gotcha".into()], None, Some("OFFLINE"), 10)
                .unwrap()
                .len(),
            1
        );

        // Sync pushes the local note (valid_at is dropped because the server
        // does not have the commit) and caches server notes locally.
        sync::add_remote(&a, "origin", &format!("{base}/repos/k2")).unwrap();
        c.create_note(
            "k2",
            &NoteCreate {
                title: "server wisdom".into(),
                body: "online".into(),
                tags: vec!["kind:decision".into()],
                ..Default::default()
            },
        )
        .unwrap();
        let r = local.sync(&c, "k2").unwrap();
        assert_eq!((r.pushed, r.pulled), (1, 2));
        let all = local.all().unwrap();
        assert_eq!(all.len(), 2);
        assert!(all.iter().all(|n| !n.id.starts_with("n_local_")));
        assert!(c
            .notes("k2", "?q=local")
            .unwrap()
            .iter()
            .any(|n| n.title == "local wisdom"));
        // Second sync pushes nothing new.
        let r = local.sync(&c, "k2").unwrap();
        assert_eq!(r.pushed, 0);
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn push_and_clone_larger_than_two_mib() {
    // Regression: object uploads were capped by axum's default 2 MiB body
    // limit, so every push of a real repository failed with a broken pipe.
    let tmp = tempfile::tempdir().unwrap();
    let base = start_server(&tmp.path().join("data")).await;
    let tmp_path = tmp.path().to_path_buf();
    tokio::task::spawn_blocking(move || {
        let url = format!("{base}/repos/big");
        let a = Repository::init(&tmp_path.join("a")).unwrap();
        // Pseudo-random, incompressible content: 300 files of 40 KiB (about
        // 12 MiB, several upload batches) plus one 9 MiB file sent on its own.
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut noise = |n: usize| -> Vec<u8> {
            (0..n)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    (x >> 24) as u8
                })
                .collect()
        };
        for i in 0..300 {
            let p = a.work_dir().join(format!("assets/tex{:03}.bin", i));
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, noise(40 * 1024)).unwrap();
        }
        let big = noise(9 * 1024 * 1024);
        fs::write(a.work_dir().join("level.pak"), &big).unwrap();
        let c = commit_all(&a, "assets", "agent-a");
        sync::add_remote(&a, "origin", &url).unwrap();
        let p = sync::push(&a, "origin", "main", false, true).unwrap();
        assert_eq!(p.new, c.id);
        assert!(p.objects_sent >= 302, "sent {}", p.objects_sent);

        let (b, _) = sync::clone(&url, &tmp_path.join("b")).unwrap();
        assert_eq!(fs::read(b.work_dir().join("level.pak")).unwrap(), big);
        assert_eq!(
            fs::read(b.work_dir().join("assets/tex299.bin"))
                .unwrap()
                .len(),
            40 * 1024
        );
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tree_with_last_commits() {
    let tmp = tempfile::tempdir().unwrap();
    let base = start_server(&tmp.path().join("data")).await;
    let tmp_path = tmp.path().to_path_buf();
    tokio::task::spawn_blocking(move || {
        let url = format!("{base}/repos/lc");
        let a = Repository::init(&tmp_path.join("a")).unwrap();
        write(a.work_dir(), "README.md", "# lc\n");
        write(a.work_dir(), "src/main.rs", "fn main() {}\n");
        let c1 = commit_all(&a, "initial", "agent-a");
        write(a.work_dir(), "src/main.rs", "fn main() { println!(); }\n");
        let c2 = commit_all(&a, "print", "agent-b");
        sync::add_remote(&a, "origin", &url).unwrap();
        sync::push(&a, "origin", "main", false, true).unwrap();
        let client = Client::new(&base);
        let plain: TreeResponse = client.get_json_public("/repos/lc/tree/main").unwrap();
        match plain {
            TreeResponse::Tree {
                entries,
                last_commits_truncated,
                ..
            } => {
                assert!(entries.iter().all(|e| e.last_commit.is_none()));
                assert!(last_commits_truncated.is_none());
            }
            _ => panic!("expected a tree"),
        }
        let rich: TreeResponse = client
            .get_json_public("/repos/lc/tree/main?last_commits=1")
            .unwrap();
        match rich {
            TreeResponse::Tree {
                entries,
                last_commits_truncated,
                ..
            } => {
                assert_eq!(last_commits_truncated, Some(false));
                let by: HashMap<_, _> = entries.iter().map(|e| (e.name.as_str(), e)).collect();
                let readme = by["README.md"].last_commit.as_ref().unwrap();
                assert_eq!(readme.id, c1.id);
                let src = by["src"].last_commit.as_ref().unwrap();
                assert_eq!(src.id, c2.id);
                assert_eq!(src.agent.as_deref(), Some("agent-b"));
                assert_eq!(src.summary, "print");
            }
            _ => panic!("expected a tree"),
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn leases_are_shared_team_wide_through_the_server() {
    // Two machines (two clones), two agents, one server.
    let tmp = tempfile::tempdir().unwrap();
    let base = start_server(&tmp.path().join("data")).await;
    let tmp_path = tmp.path().to_path_buf();
    tokio::task::spawn_blocking(move || {
        use agit_core::lease::{LeaseMode, LeaseRequest};
        use agit_protocol::leases;
        let url = format!("{base}/repos/team");
        let client = Client::new(&base);

        let seed = Repository::init(&tmp_path.join("seed")).unwrap();
        write(seed.work_dir(), "src/pay.rs", "fn pay() {}\n");
        write(seed.work_dir(), "src/cart.rs", "fn cart() {}\n");
        commit_all(&seed, "seed", "setup");
        sync::add_remote(&seed, "origin", &url).unwrap();
        sync::push(&seed, "origin", "main", false, true).unwrap();

        let (mut a, _) = sync::clone(&url, &tmp_path.join("alice")).unwrap();
        let (mut b, _) = sync::clone(&url, &tmp_path.join("bob")).unwrap();
        leases::attach(&mut a);
        leases::attach(&mut b);
        assert!(a.has_remote_leases() && b.has_remote_leases());

        // Alice claims on her machine: the lease lands on the server at once.
        let claimed = leases::claim(
            &a,
            LeaseRequest {
                holder: "alice".into(),
                paths: vec!["src/pay.rs".into()],
                mode: LeaseMode::Exclusive,
                ttl: Some(std::time::Duration::from_secs(600)),
                note: Some("refactor payments".into()),
                workspace: Some("main".into()),
            },
        )
        .unwrap();
        assert!(claimed.shared);
        assert_eq!(claimed.lease.remote.as_deref(), Some("origin"));
        let on_server = client.leases("team").unwrap();
        assert_eq!(on_server.len(), 1);
        assert_eq!(on_server[0].id, claimed.lease.id);
        assert_eq!(
            a.leases().get(&claimed.lease.id).unwrap().remote.as_deref(),
            Some("origin")
        );

        // Bob, on another machine, sees it ...
        let seen = leases::list(&b).unwrap();
        assert!(seen
            .iter()
            .any(|l| l.id == claimed.lease.id && l.holder == "alice"));
        // ... cannot claim it ...
        let err = leases::claim(
            &b,
            LeaseRequest {
                holder: "bob".into(),
                paths: vec!["src/pay.rs".into()],
                mode: LeaseMode::Exclusive,
                ttl: None,
                note: None,
                workspace: None,
            },
        )
        .unwrap_err();
        assert!(
            matches!(err, agit_core::Error::LeaseConflict { ref holder, .. } if holder == "alice"),
            "{err:?}"
        );
        // ... and cannot commit into it.
        assert!(matches!(
            b.enforce_leases("bob", &["src/pay.rs".to_string()]),
            Err(agit_core::Error::LeaseConflict { .. })
        ));
        write(b.work_dir(), "src/pay.rs", "fn pay() { bob }\n");
        worktree::add(&b, &[], false).unwrap();
        let err = ops::commit(
            &b,
            CommitOptions {
                message: "bob edits pay".into(),
                agent: Some(AgentInfo {
                    id: "bob".into(),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(
            matches!(err, agit_core::Error::LeaseConflict { .. }),
            "{err:?}"
        );
        // Other paths stay free for Bob.
        b.enforce_leases("bob", &["src/cart.rs".to_string()])
            .unwrap();

        // Forcing past the client check does not get it onto the server.
        ops::commit(
            &b,
            CommitOptions {
                message: "bob forces".into(),
                agent: Some(AgentInfo {
                    id: "bob".into(),
                    ..Default::default()
                }),
                force: true,
                ..Default::default()
            },
        )
        .unwrap();
        let err = sync::push(&b, "origin", "main", false, false).unwrap_err();
        assert!(
            matches!(err, agit_core::Error::LeaseConflict { ref holder, .. } if holder == "alice"),
            "{err:?}"
        );

        // Alice's own work into her lease is accepted.
        write(a.work_dir(), "src/pay.rs", "fn pay() { alice }\n");
        commit_all(&a, "alice refactors pay", "alice");
        sync::push(&a, "origin", "main", false, false).unwrap();

        // Releasing on her machine frees it everywhere.
        leases::release_all(&a, "alice").unwrap();
        assert!(client.leases("team").unwrap().is_empty());
        assert!(a.leases().list().unwrap().is_empty());
        assert!(!leases::list(&b)
            .unwrap()
            .iter()
            .any(|l| l.holder == "alice"));
        b.enforce_leases("bob", &["src/pay.rs".to_string()])
            .unwrap();
    })
    .await
    .unwrap();
}

#[test]
fn lease_claim_falls_back_to_local_when_the_server_is_down() {
    use agit_core::lease::{LeaseMode, LeaseRequest};
    use agit_protocol::leases;
    let tmp = tempfile::tempdir().unwrap();
    let mut repo = Repository::init(&tmp.path().join("r")).unwrap();
    write(repo.work_dir(), "a.txt", "a\n");
    commit_all(&repo, "seed", "me");
    sync::add_remote(&repo, "origin", "http://127.0.0.1:1/api/v1/repos/x").unwrap();
    leases::attach(&mut repo);
    let claimed = leases::claim(
        &repo,
        LeaseRequest {
            holder: "me".into(),
            paths: vec!["a.txt".into()],
            mode: LeaseMode::Exclusive,
            ttl: Some(std::time::Duration::from_secs(60)),
            note: None,
            workspace: None,
        },
    )
    .unwrap();
    assert!(!claimed.shared);
    assert!(claimed.lease.remote.is_none());
    // Local enforcement still works offline, and the dead server is not
    // retried on every check.
    assert!(matches!(
        repo.enforce_leases("other", &["a.txt".to_string()]),
        Err(agit_core::Error::LeaseConflict { .. })
    ));
    let t0 = std::time::Instant::now();
    repo.enforce_leases("me", &["a.txt".to_string()]).unwrap();
    assert!(t0.elapsed() < std::time::Duration::from_millis(500));
    assert_eq!(leases::list(&repo).unwrap().len(), 1);
}
