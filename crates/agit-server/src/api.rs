//! HTTP routes (docs/API.md).

use std::collections::{BTreeSet, HashMap};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use agit_core::diff::{change_stats, diff_trees, is_binary, render_patch};
use agit_core::events::{now_ms, Event};
use agit_core::lease::{pattern_covers, LeaseRequest};
use agit_core::object::Object;
use agit_core::ops::{log_store, LogOptions};
use agit_core::refs::{HEADS_PREFIX, TAGS_PREFIX};
use agit_core::store::ObjectStore;
use agit_core::{EntryMode, Error, ObjectId};
use agit_protocol::stream;
use agit_protocol::types::*;
use agit_protocol::walk;
use axum::body::Bytes;
use axum::extract::{FromRequestParts, Path, Query, Request, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use futures::stream::{Stream, StreamExt};
use tower_http::cors::{Any, CorsLayer};

use crate::error::{ApiErr, ApiResult};
use crate::knowledge::{NoteQuery, NoteStore};
use crate::pulls::PullStore;
use crate::state::{AppState, RepoHandle};

type S = State<Arc<AppState>>;

/// Acting agent from `X-Agit-Agent` (falls back to `anonymous`).
pub struct Agent(pub String);

impl<St: Send + Sync> FromRequestParts<St> for Agent {
    type Rejection = Infallible;
    async fn from_request_parts(parts: &mut Parts, _: &St) -> Result<Self, Infallible> {
        Ok(Agent(
            parts
                .headers
                .get("x-agit-agent")
                .and_then(|v| v.to_str().ok())
                .filter(|s| !s.is_empty())
                .unwrap_or("anonymous")
                .to_string(),
        ))
    }
}

fn actor_of(headers: &HeaderMap) -> Actor {
    Actor {
        id: headers
            .get("x-agit-agent")
            .and_then(|v| v.to_str().ok())
            .filter(|s| !s.is_empty())
            .unwrap_or("anonymous")
            .to_string(),
        model: headers
            .get("x-agit-model")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string),
    }
}

/// `?key=1` or `?key=true`.
fn flag(q: &HashMap<String, String>, key: &str) -> bool {
    q.get(key).is_some_and(|v| v == "1" || v == "true")
}

async fn auth(State(state): S, req: Request, next: Next) -> Response {
    if let Some(token) = &state.token {
        let ok = req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .is_some_and(|t| t == token);
        if !ok {
            return ApiErr::Forbidden("missing or invalid bearer token".into()).into_response();
        }
    }
    next.run(req).await
}

pub fn router(state: Arc<AppState>, web_dir: Option<&std::path::Path>) -> Router {
    let api = Router::new()
        .route("/health", get(health))
        .route("/repos", get(list_repos).post(create_repo))
        .route("/repos/{repo}", get(get_repo).delete(delete_repo))
        .route("/repos/{repo}/refs", get(list_refs))
        .route("/repos/{repo}/refs/update", post(update_refs))
        .route("/repos/{repo}/branches", get(list_branches))
        .route("/repos/{repo}/tags", get(list_tags))
        .route("/repos/{repo}/objects", post(upload_objects))
        .route("/repos/{repo}/objects/missing", post(missing_objects))
        .route("/repos/{repo}/objects/{id}", get(get_object))
        .route("/repos/{repo}/objects/{id}/json", get(get_object_json))
        .route("/repos/{repo}/fetch", post(fetch_objects))
        .route("/repos/{repo}/commits", get(list_commits))
        .route("/repos/{repo}/commits/{rev}", get(get_commit))
        .route("/repos/{repo}/tree/{rev}", get(get_tree))
        .route("/repos/{repo}/diff", get(get_diff))
        .route("/repos/{repo}/events", get(list_events))
        .route("/repos/{repo}/events/stream", get(stream_events))
        .route("/repos/{repo}/leases", get(list_leases).post(create_lease))
        .route("/repos/{repo}/leases/check", post(check_lease))
        .route("/repos/{repo}/leases/{id}", delete(release_lease))
        .route("/repos/{repo}/leases/{id}/renew", post(renew_lease))
        .route("/repos/{repo}/pulls", get(list_pulls).post(create_pull))
        .route("/repos/{repo}/pulls/{n}", get(get_pull))
        .route("/repos/{repo}/pulls/{n}/diff", get(pull_diff))
        .route("/repos/{repo}/pulls/{n}/update", post(update_pull))
        .route("/repos/{repo}/pulls/{n}/comments", post(comment_pull))
        .route(
            "/repos/{repo}/pulls/{n}/comments/{id}/resolve",
            post(resolve_comment),
        )
        .route("/repos/{repo}/pulls/{n}/reviews", post(review_pull))
        .route("/repos/{repo}/pulls/{n}/checks", post(check_pull))
        .route("/repos/{repo}/pulls/{n}/merge", post(merge_pull))
        .route("/repos/{repo}/pulls/{n}/close", post(close_pull))
        .route(
            "/repos/{repo}/knowledge/notes",
            get(list_notes).post(create_note),
        )
        .route(
            "/repos/{repo}/knowledge/notes/{id}",
            get(get_note).patch(patch_note).delete(delete_note),
        )
        .route(
            "/repos/{repo}/knowledge/notes/{id}/validate",
            post(validate_note),
        )
        .route(
            "/repos/{repo}/knowledge/notes/{id}/accept",
            post(accept_note),
        )
        .route("/repos/{repo}/knowledge/stale", get(stale_notes))
        .route("/repos/{repo}/knowledge/tags", get(list_tags_kb))
        .route(
            "/repos/{repo}/knowledge/rules",
            get(get_rules).put(put_rules),
        )
        .route("/repos/{repo}/policy", get(get_policy).put(put_policy))
        .route(
            "/repos/{repo}/webhooks",
            get(list_webhooks).post(create_webhook),
        )
        .route("/repos/{repo}/webhooks/{id}", delete(delete_webhook))
        .route("/repos/{repo}/webhooks/{id}/test", post(test_webhook))
        .route("/repos/{repo}/context", get(get_context))
        .route("/repos/{repo}/agents", get(list_agents))
        .layer(middleware::from_fn_with_state(state.clone(), auth))
        .with_state(state);
    let mut root = Router::new().nest("/api/v1", api);
    if let Some(dir) = web_dir {
        // Single-page app: unknown paths fall back to index.html.
        let index = dir.join("index.html");
        root = root.fallback_service(
            tower_http::services::ServeDir::new(dir)
                .fallback(tower_http::services::ServeFile::new(index)),
        );
    }
    root.layer(
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any)
            .expose_headers(Any),
    )
    .layer(tower_http::trace::TraceLayer::new_for_http())
}

// ---------------------------------------------------------------------------
// repos
// ---------------------------------------------------------------------------

async fn health() -> Json<Health> {
    Json(Health {
        status: "ok".into(),
        version: env!("CARGO_PKG_VERSION").into(),
    })
}

async fn list_repos(State(st): S) -> ApiResult<Json<Vec<RepoSummary>>> {
    let mut out = Vec::new();
    for r in st.list() {
        out.push(r.summary()?);
    }
    Ok(Json(out))
}

async fn create_repo(
    State(st): S,
    Json(req): Json<CreateRepo>,
) -> ApiResult<(StatusCode, Json<RepoSummary>)> {
    let r = st.create(&req.name, req.default_branch.as_deref())?;
    Ok((StatusCode::CREATED, Json(r.summary()?)))
}

async fn get_repo(State(st): S, Path(repo): Path<String>) -> ApiResult<Json<RepoSummary>> {
    let r = st.get(&repo)?;
    let mut s = r.summary()?;
    s.objects = r.store.iter_ids().map(|v| v.len()).unwrap_or(0);
    Ok(Json(s))
}

async fn delete_repo(State(st): S, Path(repo): Path<String>) -> ApiResult<Json<serde_json::Value>> {
    st.delete(&repo)?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}

// ---------------------------------------------------------------------------
// refs
// ---------------------------------------------------------------------------

async fn list_refs(
    State(st): S,
    Path(repo): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<Json<Vec<RefEntry>>> {
    let r = st.get(&repo)?;
    let prefix = q.get("prefix").cloned().unwrap_or_else(|| "refs/".into());
    Ok(Json(
        r.refs
            .list(&prefix)?
            .into_iter()
            .map(|(name, id)| RefEntry { name, id })
            .collect(),
    ))
}

async fn update_refs(
    State(st): S,
    Path(repo): Path<String>,
    Agent(agent): Agent,
    Json(req): Json<RefUpdateRequest>,
) -> ApiResult<Json<RefUpdateResponse>> {
    let r = st.get(&repo)?;
    for u in &req.updates {
        if let Some(id) = u.new {
            if !r.store.contains(&id) {
                return Err(Error::ObjectNotFound(id).into());
            }
        }
    }
    let olds: Vec<Option<ObjectId>> = req
        .updates
        .iter()
        .map(|u| r.refs.read(&u.name))
        .collect::<Result<_, _>>()?;
    {
        let ps = crate::policy::PolicyStore::new(&r);
        for (u, old) in req.updates.iter().zip(olds.iter()) {
            ps.check_push(&u.name, *old, u.new, &agent)?;
        }
    }
    let updates: Vec<(String, agit_core::refs::Expect, Option<ObjectId>)> = req
        .updates
        .iter()
        .map(|u| (u.name.clone(), u.expect.to_core(), u.new))
        .collect();
    r.refs.update_many(&updates)?;
    let mut resp = RefUpdateResponse {
        updated: vec![],
        deleted: vec![],
    };
    for (u, old) in req.updates.iter().zip(olds) {
        r.emit(
            "ref.update",
            Some(&agent),
            serde_json::json!({ "ref": u.name, "old": old, "new": u.new, "message": req.message }),
        )?;
        match u.new {
            Some(id) => resp.updated.push(RefEntry {
                name: u.name.clone(),
                id,
            }),
            None => resp.deleted.push(u.name.clone()),
        }
    }
    Ok(Json(resp))
}

fn ahead_behind(r: &RepoHandle, a: &ObjectId, b: &ObjectId) -> (usize, usize) {
    let ca = walk::commit_closure(&r.store, &[*a], Some(5000)).unwrap_or_default();
    let cb = walk::commit_closure(&r.store, &[*b], Some(5000)).unwrap_or_default();
    (ca.difference(&cb).count(), cb.difference(&ca).count())
}

async fn list_branches(
    State(st): S,
    Path(repo): Path<String>,
) -> ApiResult<Json<Vec<BranchEntry>>> {
    let r = st.get(&repo)?;
    let head = r.head()?;
    let mut out = Vec::new();
    for (name, id) in r.refs.list(HEADS_PREFIX)? {
        let (ahead, behind) = match head {
            Some(h) if h != id => {
                let (a, b) = ahead_behind(&r, &id, &h);
                (Some(a), Some(b))
            }
            Some(_) => (Some(0), Some(0)),
            None => (None, None),
        };
        out.push(BranchEntry {
            name: name[HEADS_PREFIX.len()..].to_string(),
            id,
            ahead,
            behind,
        });
    }
    Ok(Json(out))
}

async fn list_tags(State(st): S, Path(repo): Path<String>) -> ApiResult<Json<Vec<RefEntry>>> {
    let r = st.get(&repo)?;
    Ok(Json(
        r.refs
            .list(TAGS_PREFIX)?
            .into_iter()
            .map(|(name, id)| RefEntry {
                name: name[TAGS_PREFIX.len()..].to_string(),
                id,
            })
            .collect(),
    ))
}

// ---------------------------------------------------------------------------
// objects
// ---------------------------------------------------------------------------

async fn missing_objects(
    State(st): S,
    Path(repo): Path<String>,
    Json(req): Json<IdList>,
) -> ApiResult<Json<MissingResponse>> {
    let r = st.get(&repo)?;
    Ok(Json(MissingResponse {
        missing: req
            .ids
            .into_iter()
            .filter(|id| !r.store.contains(id))
            .collect(),
    }))
}

async fn upload_objects(
    State(st): S,
    Path(repo): Path<String>,
    body: Bytes,
) -> ApiResult<Json<StoreResponse>> {
    let r = st.get(&repo)?;
    let (stored, skipped) = tokio::task::spawn_blocking(move || stream::import(&r.store, &body))
        .await
        .map_err(|e| ApiErr::BadRequest(e.to_string()))??;
    Ok(Json(StoreResponse { stored, skipped }))
}

async fn get_object(State(st): S, Path((repo, id)): Path<(String, String)>) -> ApiResult<Response> {
    let r = st.get(&repo)?;
    let oid: ObjectId = id.parse()?;
    let (kind, payload) = r.store.read_raw(&oid)?;
    Ok((
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            ),
            (
                header::HeaderName::from_static("x-agit-kind"),
                HeaderValue::from_static(kind.name()),
            ),
        ],
        payload,
    )
        .into_response())
}

async fn get_object_json(
    State(st): S,
    Path((repo, id)): Path<(String, String)>,
) -> ApiResult<Json<serde_json::Value>> {
    let r = st.get(&repo)?;
    let oid = r.resolve(&id)?;
    Ok(Json(match r.store.read(&oid)? {
        Object::Blob(b) => {
            serde_json::json!({ "id": oid, "kind": "blob", "size": b.len(), "binary": is_binary(&b), "text": if is_binary(&b) { None } else { Some(String::from_utf8_lossy(&b).to_string()) } })
        }
        Object::Tree(t) => {
            serde_json::json!({ "id": oid, "kind": "tree", "size": t.len(), "tree": t.entries() })
        }
        Object::Commit(c) => serde_json::json!({ "id": oid, "kind": "commit", "commit": c }),
    }))
}

async fn fetch_objects(
    State(st): S,
    Path(repo): Path<String>,
    Json(req): Json<FetchRequest>,
) -> ApiResult<Response> {
    let r = st.get(&repo)?;
    let bytes = tokio::task::spawn_blocking(move || -> agit_core::Result<Vec<u8>> {
        let haves: Vec<ObjectId> = req
            .haves
            .into_iter()
            .filter(|h| r.store.contains(h))
            .collect();
        for w in &req.wants {
            if !r.store.contains(w) {
                return Err(Error::ObjectNotFound(*w));
            }
        }
        let ids = walk::objects_to_send(&r.store, &req.wants, &haves)?;
        stream::export(&r.store, &ids)
    })
    .await
    .map_err(|e| ApiErr::BadRequest(e.to_string()))??;
    Ok((
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static(stream::CONTENT_TYPE),
        )],
        bytes,
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// browsing
// ---------------------------------------------------------------------------

async fn list_commits(
    State(st): S,
    Path(repo): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<Json<Vec<CommitEntry>>> {
    let r = st.get(&repo)?;
    let rev = q.get("rev").cloned().unwrap_or_else(|| "HEAD".into());
    let start = match r.resolve(&rev) {
        Ok(id) => vec![id],
        Err(Error::UnknownRevision(_)) if rev == "HEAD" => vec![],
        Err(e) => return Err(e.into()),
    };
    let opts = LogOptions {
        limit: Some(
            q.get("limit")
                .and_then(|l| l.parse().ok())
                .unwrap_or(50)
                .min(1000),
        ),
        agent: q.get("agent").cloned().filter(|s| !s.is_empty()),
        task: q.get("task").cloned().filter(|s| !s.is_empty()),
        path: q
            .get("path")
            .cloned()
            .filter(|s| !s.is_empty())
            .map(|p| p.trim_matches('/').to_string()),
        first_parent: q
            .get("first_parent")
            .is_some_and(|v| v == "1" || v == "true"),
    };
    let entries = tokio::task::spawn_blocking(move || log_store(&r.store, &start, &opts))
        .await
        .map_err(|e| ApiErr::BadRequest(e.to_string()))??;
    Ok(Json(
        entries
            .into_iter()
            .map(|e| CommitEntry {
                id: e.id,
                commit: e.commit,
            })
            .collect(),
    ))
}

async fn get_commit(
    State(st): S,
    Path((repo, rev)): Path<(String, String)>,
) -> ApiResult<Json<CommitDetail>> {
    let r = st.get(&repo)?;
    let id = r.resolve(&rev)?;
    let c = r.store.read_commit(&id)?;
    let parent_tree = match c.parents.first() {
        Some(p) => Some(r.store.read_commit(p)?.tree),
        None => None,
    };
    let changes = diff_trees(&r.store, parent_tree.as_ref(), Some(&c.tree))?;
    let patch = render_patch(&r.store, &changes, 3)?;
    let notes = NoteStore::new(&r).for_commit(id, &changes, 10)?;
    Ok(Json(CommitDetail {
        id,
        commit: c,
        changes,
        patch,
        notes,
    }))
}

async fn get_tree(
    State(st): S,
    Path((repo, rev)): Path<(String, String)>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<Json<TreeResponse>> {
    let r = st.get(&repo)?;
    let id = r.resolve(&rev)?;
    let commit = r.store.read_commit(&id)?;
    let path = q
        .get("path")
        .map(|p| p.trim_matches('/').to_string())
        .unwrap_or_default();
    let mut cur = commit.tree;
    let mut cur_mode = agit_core::EntryMode::Dir;
    let mut size = 0u64;
    if !path.is_empty() {
        for part in path.split('/') {
            let t = r.store.read_tree(&cur)?;
            let e = t
                .get(part)
                .ok_or_else(|| ApiErr::NotFound(format!("path not found: {path}")))?;
            cur = e.id;
            cur_mode = e.mode;
            size = e.size;
        }
    }
    if cur_mode.is_dir() {
        let t = r.store.read_tree(&cur)?;
        Ok(Json(TreeResponse::Tree {
            path,
            id: cur,
            entries: t
                .entries()
                .iter()
                .map(|e| TreeEntryJson {
                    name: e.name.clone(),
                    mode: e.mode,
                    id: e.id,
                    size: e.size,
                })
                .collect(),
        }))
    } else {
        let b = r.store.read_blob(&cur)?;
        let binary = is_binary(&b);
        Ok(Json(TreeResponse::Blob {
            path,
            id: cur,
            size: size.max(b.len() as u64),
            binary,
            text: if binary {
                None
            } else {
                Some(String::from_utf8_lossy(&b).to_string())
            },
        }))
    }
}

fn diff_response(
    r: &RepoHandle,
    from: Option<&ObjectId>,
    to: &ObjectId,
) -> ApiResult<DiffResponse> {
    let from_tree = match from {
        Some(f) => Some(r.store.read_commit(f)?.tree),
        None => None,
    };
    let to_tree = r.store.read_commit(to)?.tree;
    let changes = diff_trees(&r.store, from_tree.as_ref(), Some(&to_tree))?;
    let patch = render_patch(&r.store, &changes, 3)?;
    let stats = change_stats(&r.store, &changes)
        .into_iter()
        .map(|(path, added, removed)| FileStat {
            path,
            added,
            removed,
        })
        .collect();
    Ok(DiffResponse {
        changes,
        patch,
        stats,
    })
}

async fn get_diff(
    State(st): S,
    Path(repo): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<Json<DiffResponse>> {
    let r = st.get(&repo)?;
    let to = r.resolve(
        q.get("to")
            .ok_or_else(|| ApiErr::BadRequest("missing `to`".into()))?,
    )?;
    let from = match q.get("from") {
        Some(f) => Some(r.resolve(f)?),
        None => None,
    };
    Ok(Json(diff_response(&r, from.as_ref(), &to)?))
}

// ---------------------------------------------------------------------------
// events
// ---------------------------------------------------------------------------

async fn list_events(
    State(st): S,
    Path(repo): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<Json<Vec<Event>>> {
    let r = st.get(&repo)?;
    let since = q.get("since").and_then(|s| s.parse().ok()).unwrap_or(0);
    let limit = q.get("limit").and_then(|s| s.parse().ok()).unwrap_or(500);
    let mut events = r.events.read_after(since)?;
    if events.len() > limit {
        events = events.split_off(events.len() - limit);
    }
    Ok(Json(events))
}

async fn stream_events(
    State(st): S,
    Path(repo): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>> {
    let r = st.get(&repo)?;
    let since = q
        .get("since")
        .and_then(|s| s.parse().ok())
        .unwrap_or(u64::MAX);
    let rx = r.tx.subscribe();
    let backlog = if since == u64::MAX {
        vec![]
    } else {
        r.events.read_after(since)?
    };
    let last_seq = backlog.last().map(|e| e.seq).unwrap_or(0);
    let live = tokio_stream::wrappers::BroadcastStream::new(rx).filter_map(move |ev| async move {
        match ev {
            Ok(e) if e.seq > last_seq => Some(e),
            _ => None,
        }
    });
    let all = futures::stream::iter(backlog).chain(live).map(|e| {
        Ok(SseEvent::default()
            .id(e.seq.to_string())
            .data(serde_json::to_string(&e).unwrap_or_default()))
    });
    Ok(Sse::new(all).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

// ---------------------------------------------------------------------------
// leases
// ---------------------------------------------------------------------------

async fn list_leases(State(st): S, Path(repo): Path<String>) -> ApiResult<Json<Vec<LeaseJson>>> {
    Ok(Json(st.get(&repo)?.leases.list()?))
}

async fn create_lease(
    State(st): S,
    Path(repo): Path<String>,
    Agent(agent): Agent,
    Json(req): Json<LeaseCreate>,
) -> ApiResult<(StatusCode, Json<LeaseJson>)> {
    let r = st.get(&repo)?;
    let lease = r.leases.claim(LeaseRequest {
        holder: req.holder.clone(),
        paths: req.paths,
        mode: req.mode,
        ttl: Some(Duration::from_secs(req.ttl_secs.unwrap_or(3600))),
        note: req.note,
        workspace: None,
    })?;
    r.emit("lease.claim", Some(&agent), serde_json::to_value(&lease)?)?;
    Ok((StatusCode::CREATED, Json(lease)))
}

async fn check_lease(
    State(st): S,
    Path(repo): Path<String>,
    Json(req): Json<LeaseCheck>,
) -> ApiResult<Json<serde_json::Value>> {
    let r = st.get(&repo)?;
    agit_core::ops::enforce_leases(&r.leases, &req.holder, &req.paths)?;
    Ok(Json(
        serde_json::json!({ "allowed": true, "holder": req.holder, "paths": req.paths }),
    ))
}

async fn release_lease(
    State(st): S,
    Path((repo, id)): Path<(String, String)>,
    Agent(agent): Agent,
) -> ApiResult<Json<serde_json::Value>> {
    let r = st.get(&repo)?;
    let l = r.leases.release(&id)?;
    r.emit(
        "lease.release",
        Some(&agent),
        serde_json::json!({ "id": l.id, "holder": l.holder, "paths": l.paths }),
    )?;
    Ok(Json(serde_json::json!({ "released": l.id })))
}

async fn renew_lease(
    State(st): S,
    Path((repo, id)): Path<(String, String)>,
    Agent(agent): Agent,
    Json(req): Json<LeaseRenew>,
) -> ApiResult<Json<LeaseJson>> {
    let r = st.get(&repo)?;
    let l = r.leases.renew(&id, Duration::from_secs(req.ttl_secs))?;
    r.emit(
        "lease.renew",
        Some(&agent),
        serde_json::json!({ "id": l.id, "expires_ms": l.expires_ms }),
    )?;
    Ok(Json(l))
}

// ---------------------------------------------------------------------------
// pull requests
// ---------------------------------------------------------------------------

async fn list_pulls(
    State(st): S,
    Path(repo): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<Json<Vec<PullRequest>>> {
    let r = st.get(&repo)?;
    let state = match q.get("state").map(String::as_str) {
        Some("open") => Some(PullState::Open),
        Some("merged") => Some(PullState::Merged),
        Some("closed") => Some(PullState::Closed),
        _ => None,
    };
    Ok(Json(PullStore::new(&r).list(state)?))
}

async fn create_pull(
    State(st): S,
    Path(repo): Path<String>,
    headers: HeaderMap,
    Json(req): Json<PullCreate>,
) -> ApiResult<(StatusCode, Json<PullRequest>)> {
    let r = st.get(&repo)?;
    Ok((
        StatusCode::CREATED,
        Json(PullStore::new(&r).create(req, actor_of(&headers))?),
    ))
}

async fn get_pull(
    State(st): S,
    Path((repo, n)): Path<(String, u64)>,
) -> ApiResult<Json<PullDetail>> {
    let r = st.get(&repo)?;
    let d = tokio::task::spawn_blocking(move || PullStore::new(&r).detail(n))
        .await
        .map_err(|e| ApiErr::BadRequest(e.to_string()))??;
    Ok(Json(d))
}

async fn pull_diff(
    State(st): S,
    Path((repo, n)): Path<(String, u64)>,
) -> ApiResult<Json<DiffResponse>> {
    let r = st.get(&repo)?;
    let ps = PullStore::new(&r);
    let pr = ps.load(n)?;
    let changes = ps.changes(&pr)?;
    let patch = render_patch(&r.store, &changes, 3)?;
    let stats = change_stats(&r.store, &changes)
        .into_iter()
        .map(|(path, added, removed)| FileStat {
            path,
            added,
            removed,
        })
        .collect();
    Ok(Json(DiffResponse {
        changes,
        patch,
        stats,
    }))
}

async fn update_pull(
    State(st): S,
    Path((repo, n)): Path<(String, u64)>,
    Agent(agent): Agent,
    Json(req): Json<PullUpdate>,
) -> ApiResult<Json<PullRequest>> {
    let r = st.get(&repo)?;
    Ok(Json(PullStore::new(&r).update(n, req, &agent)?))
}

async fn comment_pull(
    State(st): S,
    Path((repo, n)): Path<(String, u64)>,
    Agent(agent): Agent,
    Json(req): Json<CommentCreate>,
) -> ApiResult<(StatusCode, Json<Comment>)> {
    let r = st.get(&repo)?;
    Ok((
        StatusCode::CREATED,
        Json(PullStore::new(&r).comment(n, req, &agent)?),
    ))
}

async fn resolve_comment(
    State(st): S,
    Path((repo, n, id)): Path<(String, u64, u64)>,
    Agent(agent): Agent,
) -> ApiResult<Json<PullRequest>> {
    let r = st.get(&repo)?;
    Ok(Json(PullStore::new(&r).resolve_comment(n, id, &agent)?))
}

async fn review_pull(
    State(st): S,
    Path((repo, n)): Path<(String, u64)>,
    Agent(agent): Agent,
    Json(req): Json<ReviewCreate>,
) -> ApiResult<Json<PullRequest>> {
    let r = st.get(&repo)?;
    Ok(Json(PullStore::new(&r).review(n, req, &agent)?))
}

async fn check_pull(
    State(st): S,
    Path((repo, n)): Path<(String, u64)>,
    Agent(agent): Agent,
    Json(req): Json<CheckCreate>,
) -> ApiResult<Json<PullRequest>> {
    let r = st.get(&repo)?;
    Ok(Json(PullStore::new(&r).check(n, req, &agent)?))
}

async fn merge_pull(
    State(st): S,
    Path((repo, n)): Path<(String, u64)>,
    headers: HeaderMap,
    body: Option<Json<MergeRequest>>,
) -> ApiResult<Json<MergeResponse>> {
    let r = st.get(&repo)?;
    let req = body.map(|b| b.0).unwrap_or_default();
    let actor = actor_of(&headers);
    let resp = tokio::task::spawn_blocking(move || PullStore::new(&r).merge(n, req, &actor))
        .await
        .map_err(|e| ApiErr::BadRequest(e.to_string()))??;
    Ok(Json(resp))
}

async fn close_pull(
    State(st): S,
    Path((repo, n)): Path<(String, u64)>,
    Agent(agent): Agent,
) -> ApiResult<Json<PullRequest>> {
    let r = st.get(&repo)?;
    Ok(Json(PullStore::new(&r).close(n, &agent)?))
}

// ---------------------------------------------------------------------------
// knowledge
// ---------------------------------------------------------------------------

async fn list_notes(
    State(st): S,
    Path(repo): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<Json<Vec<NoteSummary>>> {
    let r = st.get(&repo)?;
    let query = NoteQuery {
        tags: q
            .get("tags")
            .map(|t| {
                t.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        path: q.get("path").cloned().filter(|s| !s.is_empty()),
        q: q.get("q").cloned().filter(|s| !s.is_empty()),
        commit: match q.get("commit").filter(|s| !s.is_empty()) {
            Some(c) => Some(r.resolve(c)?),
            None => None,
        },
        pull: q.get("pull").and_then(|p| p.parse().ok()),
        limit: q.get("limit").and_then(|l| l.parse().ok()).unwrap_or(100),
        include_superseded: flag(&q, "include_superseded"),
        include_drafts: flag(&q, "drafts"),
    };
    let out = tokio::task::spawn_blocking(move || NoteStore::new(&r).query(&query))
        .await
        .map_err(|e| ApiErr::BadRequest(e.to_string()))??;
    Ok(Json(out))
}

async fn create_note(
    State(st): S,
    Path(repo): Path<String>,
    Agent(agent): Agent,
    Json(req): Json<NoteCreate>,
) -> ApiResult<(StatusCode, Json<Note>)> {
    let r = st.get(&repo)?;
    Ok((
        StatusCode::CREATED,
        Json(NoteStore::new(&r).create(req, &agent)?),
    ))
}

async fn get_note(State(st): S, Path((repo, id)): Path<(String, String)>) -> ApiResult<Json<Note>> {
    let r = st.get(&repo)?;
    Ok(Json(NoteStore::new(&r).get(&id)?))
}

async fn patch_note(
    State(st): S,
    Path((repo, id)): Path<(String, String)>,
    Agent(agent): Agent,
    Json(req): Json<NotePatch>,
) -> ApiResult<Json<Note>> {
    let r = st.get(&repo)?;
    Ok(Json(NoteStore::new(&r).patch(&id, req, &agent)?))
}

async fn delete_note(
    State(st): S,
    Path((repo, id)): Path<(String, String)>,
    Agent(agent): Agent,
) -> ApiResult<Json<serde_json::Value>> {
    let r = st.get(&repo)?;
    NoteStore::new(&r).delete(&id, &agent)?;
    Ok(Json(serde_json::json!({ "deleted": id })))
}

async fn validate_note(
    State(st): S,
    Path((repo, id)): Path<(String, String)>,
    Agent(agent): Agent,
    body: Option<Json<ValidateRequest>>,
) -> ApiResult<Json<Note>> {
    let r = st.get(&repo)?;
    let rev = body.and_then(|b| b.0.rev).filter(|s| !s.is_empty());
    Ok(Json(NoteStore::new(&r).validate(
        &id,
        rev.as_deref(),
        &agent,
    )?))
}

async fn accept_note(
    State(st): S,
    Path((repo, id)): Path<(String, String)>,
    Agent(agent): Agent,
    body: Option<Json<NotePatch>>,
) -> ApiResult<Json<Note>> {
    let r = st.get(&repo)?;
    Ok(Json(NoteStore::new(&r).accept(
        &id,
        body.map(|b| b.0),
        &agent,
    )?))
}

async fn stale_notes(
    State(st): S,
    Path(repo): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<Json<Vec<StaleNote>>> {
    let r = st.get(&repo)?;
    let rev = q.get("rev").cloned().filter(|s| !s.is_empty());
    let drafts = flag(&q, "drafts");
    let out = tokio::task::spawn_blocking(move || NoteStore::new(&r).stale(rev.as_deref(), drafts))
        .await
        .map_err(|e| ApiErr::BadRequest(e.to_string()))??;
    Ok(Json(out))
}

async fn list_tags_kb(State(st): S, Path(repo): Path<String>) -> ApiResult<Json<Vec<TagCount>>> {
    let r = st.get(&repo)?;
    Ok(Json(NoteStore::new(&r).tags()?))
}

async fn get_rules(State(st): S, Path(repo): Path<String>) -> ApiResult<Json<Vec<TagRule>>> {
    let r = st.get(&repo)?;
    Ok(Json(NoteStore::new(&r).rules()?))
}

async fn put_rules(
    State(st): S,
    Path(repo): Path<String>,
    Agent(agent): Agent,
    Json(rules): Json<Vec<TagRule>>,
) -> ApiResult<Json<Vec<TagRule>>> {
    let r = st.get(&repo)?;
    Ok(Json(NoteStore::new(&r).set_rules(rules, &agent)?))
}

/// Default context budget in approximate tokens (4 characters per token).
const CONTEXT_DEFAULT_BUDGET: usize = 6000;
const CONTEXT_MAX_SIBLINGS: usize = 5;
const ACTIVE_WINDOW_MS: i64 = 24 * 3600 * 1000;

async fn get_context(
    State(st): S,
    Path(repo): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<Json<ContextResponse>> {
    let r = st.get(&repo)?;
    let path = q
        .get("path")
        .map(|p| p.trim_matches('/').to_string())
        .ok_or_else(|| ApiErr::BadRequest("missing `path`".into()))?;
    let limit = q.get("limit").and_then(|l| l.parse().ok()).unwrap_or(10);
    let budget: usize = q
        .get("budget")
        .and_then(|b| b.parse().ok())
        .unwrap_or(CONTEXT_DEFAULT_BUDGET);
    let include_files = q
        .get("include")
        .is_some_and(|v| v.split(',').any(|x| x.trim() == "files"));
    let resp = tokio::task::spawn_blocking(move || -> ApiResult<ContextResponse> {
        let ns = NoteStore::new(&r);
        let mut notes = ns.query(&NoteQuery {
            path: Some(path.clone()),
            limit,
            ..Default::default()
        })?;
        // Fresh notes first; the query order (newest update first) is kept within each group.
        notes.sort_by_key(|n| n.stale);
        let suggested_tags = ns.suggested_tags(&path)?;
        let head = r.head()?;
        let log = match head {
            Some(h) => log_store(
                &r.store,
                &[h],
                &LogOptions {
                    limit: Some(limit.max(100)),
                    path: Some(path.clone()),
                    ..Default::default()
                },
            )?,
            None => vec![],
        };
        let leases: Vec<_> = r
            .leases
            .list()?
            .into_iter()
            .filter(|l| {
                l.paths
                    .iter()
                    .any(|p| pattern_covers(p, &path) || pattern_covers(&path, p))
            })
            .collect();
        let since = now_ms() - ACTIVE_WINDOW_MS;
        let mut active: BTreeSet<String> = leases.iter().map(|l| l.holder.clone()).collect();
        for e in log.iter().filter(|e| e.commit.time_ms >= since) {
            active.insert(match &e.commit.agent {
                Some(a) => a.id.clone(),
                None => e.commit.author.name.clone(),
            });
        }
        let recent_commits = log
            .into_iter()
            .take(limit)
            .map(|e| CommitEntry {
                id: e.id,
                commit: e.commit,
            })
            .collect();
        let open_pulls = PullStore::new(&r).open_touching(&path)?;
        let mut resp = ContextResponse {
            path: path.clone(),
            notes,
            suggested_tags,
            recent_commits,
            leases,
            open_pulls,
            agents_active: active.into_iter().collect(),
            files: vec![],
        };
        if include_files {
            if let Some(h) = head {
                let used = serde_json::to_string(&resp)?.len();
                let remaining = (budget * 4).saturating_sub(used);
                resp.files = context_files(&r, h, &path, remaining)?;
            }
        }
        Ok(resp)
    })
    .await
    .map_err(|e| ApiErr::BadRequest(e.to_string()))??;
    Ok(Json(resp))
}

fn lookup_path(
    store: &ObjectStore,
    root: ObjectId,
    path: &str,
) -> ApiResult<Option<(EntryMode, ObjectId)>> {
    let mut cur = (EntryMode::Dir, root);
    if path.is_empty() {
        return Ok(Some(cur));
    }
    for part in path.split('/') {
        if !cur.0.is_dir() {
            return Ok(None);
        }
        let t = store.read_tree(&cur.1)?;
        match t.get(part) {
            Some(e) => cur = (e.mode, e.id),
            None => return Ok(None),
        }
    }
    Ok(Some(cur))
}

/// The file at `path` (when it is one) plus up to five sibling files in the
/// same directory, most recently touched first, trimmed to `budget_chars`.
/// Later files are cut first; a file that does not fit at all is dropped.
fn context_files(
    r: &RepoHandle,
    head: ObjectId,
    path: &str,
    budget_chars: usize,
) -> ApiResult<Vec<ContextFile>> {
    let store = &r.store;
    let root = store.read_commit(&head)?.tree;
    let (dir, target) = match lookup_path(store, root, path)? {
        Some((mode, _)) if mode.is_dir() => (path.to_string(), None),
        Some((mode, id)) if !mode.is_submodule() => (
            path.rsplit_once('/')
                .map(|(d, _)| d)
                .unwrap_or("")
                .to_string(),
            Some((path.to_string(), id)),
        ),
        _ => return Ok(vec![]),
    };
    let mut candidates: Vec<(String, ObjectId)> = Vec::new();
    if let Some(t) = &target {
        candidates.push(t.clone());
    }
    let dir_tree = match lookup_path(store, root, &dir)? {
        Some((mode, id)) if mode.is_dir() => store.read_tree(&id)?,
        _ => return Ok(vec![]),
    };
    let mut siblings: Vec<(String, ObjectId)> = dir_tree
        .entries()
        .iter()
        .filter(|e| !e.mode.is_dir() && !e.mode.is_submodule())
        .map(|e| {
            let p = if dir.is_empty() {
                e.name.clone()
            } else {
                format!("{dir}/{}", e.name)
            };
            (p, e.id)
        })
        .filter(|(p, _)| Some(p.as_str()) != target.as_ref().map(|t| t.0.as_str()))
        .collect();
    // Most recently touched first: walk recent commits touching the directory.
    let mut recency: HashMap<String, usize> = HashMap::new();
    let log = log_store(
        store,
        &[head],
        &LogOptions {
            limit: Some(100),
            path: if dir.is_empty() {
                None
            } else {
                Some(dir.clone())
            },
            ..Default::default()
        },
    )?;
    for (rank, e) in log.iter().enumerate() {
        let parent_tree = match e.commit.parents.first() {
            Some(p) => Some(store.read_commit(p)?.tree),
            None => None,
        };
        for c in diff_trees(store, parent_tree.as_ref(), Some(&e.commit.tree))? {
            recency.entry(c.path).or_insert(rank);
        }
    }
    siblings.sort_by(|a, b| {
        let ra = recency.get(&a.0).copied().unwrap_or(usize::MAX);
        let rb = recency.get(&b.0).copied().unwrap_or(usize::MAX);
        ra.cmp(&rb).then_with(|| a.0.cmp(&b.0))
    });
    candidates.extend(siblings.into_iter().take(CONTEXT_MAX_SIBLINGS));

    let mut out = Vec::new();
    let mut remaining = budget_chars;
    for (p, id) in candidates {
        if remaining == 0 {
            break;
        }
        let bytes = store.read_blob(&id)?;
        if is_binary(&bytes) {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        // Per-file overhead for the JSON wrapper and the path.
        let overhead = p.len() + 40;
        if remaining <= overhead {
            break;
        }
        let room = remaining - overhead;
        let total = text.chars().count();
        let take = total.min(room);
        let cut: String = text.chars().take(take).collect();
        remaining -= overhead + take;
        out.push(ContextFile {
            path: p,
            text: cut,
            truncated: take < total,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// policy
// ---------------------------------------------------------------------------

async fn get_policy(State(st): S, Path(repo): Path<String>) -> ApiResult<Json<Policy>> {
    let r = st.get(&repo)?;
    Ok(Json(crate::policy::PolicyStore::new(&r).get()?))
}

async fn put_policy(
    State(st): S,
    Path(repo): Path<String>,
    Agent(agent): Agent,
    Json(p): Json<Policy>,
) -> ApiResult<Json<Policy>> {
    let r = st.get(&repo)?;
    Ok(Json(crate::policy::PolicyStore::new(&r).set(p, &agent)?))
}

// ---------------------------------------------------------------------------
// webhooks
// ---------------------------------------------------------------------------

async fn list_webhooks(State(st): S, Path(repo): Path<String>) -> ApiResult<Json<Vec<Webhook>>> {
    let r = st.get(&repo)?;
    Ok(Json(crate::webhooks::WebhookStore::new(&r).list()?))
}

async fn create_webhook(
    State(st): S,
    Path(repo): Path<String>,
    Agent(agent): Agent,
    Json(req): Json<WebhookCreate>,
) -> ApiResult<(StatusCode, Json<Webhook>)> {
    let r = st.get(&repo)?;
    Ok((
        StatusCode::CREATED,
        Json(crate::webhooks::WebhookStore::new(&r).create(req, &agent)?),
    ))
}

async fn delete_webhook(
    State(st): S,
    Path((repo, id)): Path<(String, String)>,
    Agent(agent): Agent,
) -> ApiResult<Json<serde_json::Value>> {
    let r = st.get(&repo)?;
    crate::webhooks::WebhookStore::new(&r).delete(&id, &agent)?;
    Ok(Json(serde_json::json!({ "deleted": id })))
}

async fn test_webhook(
    State(st): S,
    Path((repo, id)): Path<(String, String)>,
) -> ApiResult<Json<serde_json::Value>> {
    let r = st.get(&repo)?;
    crate::webhooks::WebhookStore::new(&r).test(&id)?;
    Ok(Json(serde_json::json!({ "queued": true })))
}

// ---------------------------------------------------------------------------
// agents
// ---------------------------------------------------------------------------

async fn list_agents(
    State(st): S,
    Path(repo): Path<String>,
) -> ApiResult<Json<Vec<AgentActivity>>> {
    let r = st.get(&repo)?;
    let out = tokio::task::spawn_blocking(move || -> ApiResult<Vec<AgentActivity>> {
        let mut m: HashMap<String, AgentActivity> = HashMap::new();
        let mut roots: Vec<ObjectId> = r
            .refs
            .list(HEADS_PREFIX)?
            .into_iter()
            .map(|(_, id)| id)
            .collect();
        roots.dedup();
        for e in log_store(
            &r.store,
            &roots,
            &LogOptions {
                limit: Some(5000),
                ..Default::default()
            },
        )? {
            if let Some(a) = &e.commit.agent {
                let entry = m.entry(a.id.clone()).or_insert_with(|| AgentActivity {
                    id: a.id.clone(),
                    model: a.model.clone(),
                    commits: 0,
                    last_seen_ms: 0,
                    open_pulls: 0,
                    leases: 0,
                });
                entry.commits += 1;
                entry.last_seen_ms = entry.last_seen_ms.max(e.commit.time_ms);
                if entry.model.is_none() {
                    entry.model = a.model.clone();
                }
            }
        }
        for l in r.leases.list()? {
            let entry = m.entry(l.holder.clone()).or_insert_with(|| AgentActivity {
                id: l.holder.clone(),
                model: None,
                commits: 0,
                last_seen_ms: 0,
                open_pulls: 0,
                leases: 0,
            });
            entry.leases += 1;
            entry.last_seen_ms = entry.last_seen_ms.max(l.created_ms);
        }
        for (author, n) in PullStore::new(&r).open_by_author()? {
            let entry = m.entry(author.clone()).or_insert_with(|| AgentActivity {
                id: author,
                model: None,
                commits: 0,
                last_seen_ms: 0,
                open_pulls: 0,
                leases: 0,
            });
            entry.open_pulls = n;
        }
        let mut v: Vec<AgentActivity> = m.into_values().collect();
        v.sort_by_key(|a| std::cmp::Reverse(a.last_seen_ms));
        Ok(v)
    })
    .await
    .map_err(|e| ApiErr::BadRequest(e.to_string()))??;
    Ok(Json(out))
}
