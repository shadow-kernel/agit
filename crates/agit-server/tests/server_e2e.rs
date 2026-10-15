//! Server + client end-to-end: push, clone, pull, pull requests, knowledge.

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
        std::env::set_var("AGIT_KEYS_DIR", tmp_path.join("keys"));
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
        std::env::remove_var("AGIT_KEYS_DIR");
    })
    .await
    .unwrap();
}
