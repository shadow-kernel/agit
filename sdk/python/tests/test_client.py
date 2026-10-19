from __future__ import annotations

import struct

import pytest

import agit
from agit import (
    AgitError,
    AuthorizedKey,
    CommitDetail,
    AlreadyExists,
    Client,
    Forbidden,
    Scope,
    TokenCreated,
    Unauthorized,
    WhoAmI,
    auth_message,
    ContextFile,
    ContextResponse,
    Event,
    KeyEntry,
    Lease,
    LeaseConflict,
    MergeConflicts,
    NotFound,
    Note,
    Policy,
    PolicyViolation,
    PullDetail,
    PullRequest,
    RefConflict,
    RefUpdate,
    SignatureInfo,
    StaleNote,
    TransportError,
    expect_absent,
    expect_any,
    expect_is,
)

from conftest import ID_A, ID_B, ID_C, NONCE, SSH_PUB


# -- construction, headers, env --------------------------------------------


def test_default_base_url_and_env_defaults(monkeypatch):
    monkeypatch.setenv("AGIT_TOKEN", "secret")
    monkeypatch.setenv("AGIT_AGENT", "env-agent")
    monkeypatch.setenv("AGIT_MODEL", "env-model")
    c = Client()
    assert c.base_url == "http://127.0.0.1:7878/api/v1"
    assert c.token == "secret" and c.agent == "env-agent" and c.model == "env-model"
    # explicit arguments win over the environment
    c2 = Client(token="t", agent="a", model="m")
    assert (c2.token, c2.agent, c2.model) == ("t", "a", "m")


def test_base_url_none_uses_server_url_env(monkeypatch):
    monkeypatch.setenv("AGIT_SERVER_URL", "http://example.test:1/api/v1/")
    assert Client(None).base_url == "http://example.test:1/api/v1"


def test_headers_are_sent(server, client):
    c = client.with_token("tok")
    assert c.health().status == "ok"
    req = server.last
    assert req.path == "/api/v1/health"
    assert req.headers["authorization"] == "Bearer tok"
    assert req.headers["x-agit-agent"] == "coder-1"
    assert req.headers["x-agit-model"] == "fable"
    assert req.headers["accept"] == "application/json"
    # the original client is untouched (with_* returns copies)
    assert client.token is None


def test_no_optional_headers_when_unset(server, monkeypatch):
    for var in ("AGIT_TOKEN", "AGIT_AGENT", "AGIT_MODEL"):
        monkeypatch.delenv(var, raising=False)
    Client(server.base_url).health()
    hdrs = server.last.headers
    assert "authorization" not in hdrs and "x-agit-agent" not in hdrs and "x-agit-model" not in hdrs


def test_bearer_token_enforced_by_server(server, client):
    server.state.token = "right"
    with pytest.raises(Unauthorized) as ei:
        client.list_repos()
    assert ei.value.code == "unauthorized" and ei.value.status == 401
    assert client.with_token("right").list_repos()[0].name == "demo"


# -- auth -------------------------------------------------------------------


def test_whoami_and_scopes(server, client):
    me = client.whoami()
    assert isinstance(me, WhoAmI) and me.principal == "tok_sess" and me.agent == "coder-1"
    assert me.key == "key_1" and not me.root
    assert me.holds("demo", "write") and me.holds("demo", "read")
    assert not me.holds("demo", "admin") and not me.holds("other", "read")


def test_login_with_key_signs_the_challenge_and_keeps_the_token(server, client):
    seen: list[bytes] = []

    def sign(message: bytes) -> bytes:
        seen.append(message)
        return bytes.fromhex("ab" * 64)

    created = client.login_with_key(SSH_PUB, sign, ttl_secs=600)
    assert seen == [auth_message(NONCE)] and seen[0].startswith(b"agit-auth-v1\n")
    assert isinstance(created, TokenCreated) and created.token == "agit_session" and created.key == "key_1"
    assert created.scopes == [Scope(repo="demo", access="write")]
    assert client.token == "agit_session"
    sess = server.requests[-1]
    assert sess.method == "POST" and sess.path == "/api/v1/auth/session"
    assert sess.json == {"public_key": SSH_PUB, "nonce": NONCE, "signature": "ab" * 64, "ttl_secs": 600}
    # The session token is sent from now on; logout revokes and forgets it.
    client.whoami()
    assert server.last.headers.get("authorization") == "Bearer agit_session"
    assert client.logout() == {"revoked": "tok_sess"}
    assert client.token is None


def test_unknown_key_is_forbidden(server, client):
    with pytest.raises(Forbidden) as ei:
        client.login_with_key("ssh-ed25519 AAAA other@host", lambda m: b"\0" * 64)
    assert "not authorized" in ei.value.message
    with pytest.raises(Forbidden):
        client.create_session(SSH_PUB, "stale-nonce", "ab" * 64)


def test_tokens_and_authorized_keys(server, client):
    tokens = client.tokens()
    assert [t.id for t in tokens] == ["tok_sess"] and tokens[0].key == "key_1"
    created = client.create_token("ci", [{"repo": "demo", "access": "read"}, Scope("*", "read")], agent="ci-bot", ttl_secs=60)
    assert created.token == "agit_new" and created.agent == "ci-bot"
    assert server.last.json == {"name": "ci", "agent": "ci-bot", "scopes": [{"repo": "demo", "access": "read"}, {"repo": "*", "access": "read"}], "ttl_secs": 60}
    assert client.delete_token("tok_new") == {"deleted": "tok_new"}

    keys = client.authorized_keys()
    assert len(keys) == 1 and isinstance(keys[0], AuthorizedKey)
    assert keys[0].fingerprint.startswith("SHA256:") and keys[0].scopes[0].repo == "demo"
    with pytest.raises(AlreadyExists):
        client.add_authorized_key(SSH_PUB, [Scope("demo", "write")])
    k = client.add_authorized_key("ssh-ed25519 AAAA new@host", [Scope("demo", "read")], name="new laptop", agent="reviewer-1")
    assert k.id == "key_2" and k.name == "new laptop" and k.agent == "reviewer-1"
    assert server.last.json == {"public_key": "ssh-ed25519 AAAA new@host", "name": "new laptop", "agent": "reviewer-1", "scopes": [{"repo": "demo", "access": "read"}]}
    assert client.delete_authorized_key("key_2") == {"deleted": "key_2", "revoked_sessions": 1}


def test_transport_error_is_retryable():
    c = Client("http://127.0.0.1:9/api/v1", timeout=1)
    with pytest.raises(TransportError) as ei:
        c.health()
    assert ei.value.retryable is True and ei.value.code == "transport"
    assert isinstance(ei.value, AgitError)


# -- repos ------------------------------------------------------------------


def test_repos(server, client):
    repos = client.list_repos()
    assert [r.name for r in repos] == ["demo"]
    created = client.create_repo("new", default_branch="trunk")
    assert created.name == "new" and created.default_branch == "trunk"
    assert server.last.json == {"name": "new", "default_branch": "trunk"}
    client.create_repo("plain")
    assert server.last.json == {"name": "plain"}  # None fields are not sent
    with pytest.raises(AlreadyExists) as ei:
        client.create_repo("new")
    assert ei.value.code == "already_exists" and ei.value.status == 409 and ei.value.retryable is False
    r = client.repo("demo")
    assert r.head == ID_A and r.objects == 3
    assert client.delete_repo("new") == {"deleted": True}
    with pytest.raises(NotFound) as ei:
        client.repo("new")
    assert ei.value.status == 404 and ei.value.code == "not_found"
    assert "not found" in ei.value.message


# -- refs -------------------------------------------------------------------


def test_refs_and_prefix(server, client):
    refs = client.refs("demo", prefix="refs/heads/")
    assert [r.name for r in refs] == ["refs/heads/main"] and refs[0].id == ID_A
    assert server.last.q("prefix") == "refs/heads/"
    assert len(client.refs("demo")) == 2
    assert "prefix" not in server.last.query


def test_update_refs_helpers_and_body(server, client):
    resp = client.update_refs(
        "demo",
        [RefUpdate("refs/heads/main", expect_is(ID_A), ID_B),
         {"name": "refs/heads/new", "expect": expect_absent(), "new": ID_B},
         RefUpdate("refs/heads/old", expect_any(), None)],
        message="land x",
    )
    sent = server.last.json
    assert sent["message"] == "land x"
    assert sent["updates"][0] == {"name": "refs/heads/main", "expect": {"is": ID_A}, "new": ID_B}
    assert sent["updates"][1]["expect"] == {"absent": True}
    assert sent["updates"][2] == {"name": "refs/heads/old", "expect": {"any": True}, "new": None}
    assert [u.name for u in resp.updated] == ["refs/heads/main", "refs/heads/new"]
    assert resp.deleted == ["refs/heads/old"]


def test_ref_conflict_maps_to_subclass(client):
    with pytest.raises(RefConflict) as ei:
        client.update_refs("demo", [RefUpdate("refs/heads/main", expect_is(ID_B), ID_C)])
    e = ei.value
    assert e.code == "ref_conflict" and e.retryable is True and e.status == 409
    assert e.name == "refs/heads/main" and e.expected == ID_B and e.actual == ID_A
    assert e.details["actual"] == ID_A


def test_object_not_found_is_not_found(client):
    with pytest.raises(NotFound) as ei:
        client.set_ref("demo", "refs/heads/main", ID_C, expect_is(ID_A))
    assert ei.value.code == "object_not_found" and ei.value.status == 412


# -- objects ----------------------------------------------------------------


def test_objects_roundtrip(server, client):
    assert client.missing("demo", [ID_A, ID_B]) == [ID_B]
    assert server.last.json == {"ids": [ID_A, ID_B]}

    frame = bytes([1]) + struct.pack(">I", 3) + b"abc"
    out = client.upload_objects("demo", frame * 2)
    assert out == {"stored": 2, "skipped": 0}
    assert server.last.headers["content-type"] == "application/x-agit-objects"
    assert server.last.body == frame * 2

    data = client.fetch_objects("demo", [ID_B], haves=[ID_A])
    assert server.last.json == {"wants": [ID_B], "haves": [ID_A]}
    assert isinstance(data, bytes) and data[0] == 1 and data[5:] == b"hello"

    kind, payload = client.get_object("demo", ID_C)
    assert kind == "blob" and payload == b"hello"


# -- browsing ---------------------------------------------------------------


def test_commits_query_encoding(server, client):
    commits = client.commits("demo", rev="main", limit=5, agent="coder-1", task="T 1", path="src/a b.rs")
    q = server.last
    assert q.path == "/api/v1/repos/demo/commits"
    assert q.q("rev") == "main" and q.q("limit") == "5" and q.q("agent") == "coder-1"
    assert q.q("task") == "T 1" and q.q("path") == "src/a b.rs"
    assert "first_parent" not in q.query
    assert commits[0].id == ID_B and commits[0].intent == "Provide x" and commits[0].agent_id == "coder-1"

    client.commits("demo")
    assert server.last.query == {}


def test_commit_tree_diff_branches_tags(server, client):
    d = client.commit("demo", "refs/heads/main")
    assert server.last.path == "/api/v1/repos/demo/commits/refs%2Fheads%2Fmain"
    assert d.id == ID_B and d.changes[0].letter == "A" and d.patch == "+x"

    t = client.tree("demo", "main")
    assert t["kind"] == "tree" and t["entries"][0]["name"] == "x.txt"
    b = client.tree("demo", "main", path="x.txt")
    assert b["kind"] == "blob" and b["text"] == "x\n"
    client.tree("demo", "main", last_commits=True, max_commits=50)
    assert server.last.q("last_commits") == "1" and server.last.q("max_commits") == "50"
    client.tree("demo", "main")
    assert "last_commits" not in server.last.query

    diff = client.diff("demo", None, "main")
    assert server.last.query == {"to": ["main"]}
    assert diff.stats[0].added == 1
    client.diff("demo", "v1", "main")
    assert server.last.q("from") == "v1"

    br = client.branches("demo")
    assert br[1].ahead == 1 and br[0].ahead is None
    assert client.tags("demo")[0].name == "v1"


# -- events -----------------------------------------------------------------


def test_events_list(server, client):
    evs = client.events("demo", since=1, limit=10)
    assert server.last.q("since") == "1" and server.last.q("limit") == "10"
    assert [e.seq for e in evs] == [2, 3]
    assert isinstance(evs[0], Event) and evs[0].kind == "ref.update" and evs[0].data["new"] == ID_A


def test_stream_events_parses_sse(server, client):
    events = list(client.stream_events("demo", since=1))
    assert server.last.q("since") == "1"
    assert server.last.headers["accept"] == "text/event-stream"
    assert [e.seq for e in events] == [2, 3, 99]
    assert events[0].actor == "coder-1"
    assert events[-1].kind == "multi.line"  # multi-line data: joined correctly


def test_stream_events_without_since_omits_param(server, client):
    gen = client.stream_events("demo")
    first = next(gen)
    gen.close()
    assert "since" not in server.last.query
    assert first.seq == 1


# -- leases -----------------------------------------------------------------


def test_leases(server, client):
    lease = client.claim_lease("demo", None, "src/auth/**", ttl_secs=600, note="T-1")
    assert isinstance(lease, Lease)
    assert server.last.json == {"holder": "coder-1", "paths": ["src/auth/**"], "mode": "exclusive",
                                "ttl_secs": 600, "note": "T-1"}
    assert lease.holder == "coder-1" and lease.expires_ms == 100 + 600_000
    assert [l.id for l in client.leases("demo")] == [lease.id]
    renewed = client.renew_lease("demo", lease.id, 1200)
    assert renewed.expires_ms == 100 + 1_200_000 and server.last.json == {"ttl_secs": 1200}
    assert client.check_lease("demo", "coder-1", ["src/auth/x.rs"])["allowed"] is True
    assert client.release_lease("demo", lease.id) == {"released": lease.id}
    with pytest.raises(NotFound) as ei:
        client.release_lease("demo", lease.id)
    assert ei.value.code == "lease_not_found"


def test_lease_conflict(client):
    with pytest.raises(LeaseConflict) as ei:
        client.claim_lease("demo", "coder-1", ["src/locked.rs"])
    e = ei.value
    assert e.status == 409 and e.retryable is False
    assert e.path == "src/locked.rs" and e.holder == "other-agent" and e.lease_id == "l_other"
    with pytest.raises(LeaseConflict):
        client.check_lease("demo", "coder-1", "src/locked.rs")


def test_claim_lease_needs_holder(server):
    with pytest.raises(ValueError):
        Client(server.base_url).claim_lease("demo", None, ["x"])


# -- pulls ------------------------------------------------------------------


def test_create_and_get_pull(server, client):
    pr = client.create_pull("demo", "Add y", "refs/heads/ws/coder-1", intent="why", task="T-2")
    assert server.last.json == {"title": "Add y", "intent": "why", "task": "T-2", "source": "refs/heads/ws/coder-1"}
    assert isinstance(pr, PullRequest)
    assert pr.number == 2 and pr.title == "Add y" and pr.target == "refs/heads/main"
    assert pr.author.id == "coder-1" and pr.author.model == "fable"
    assert pr.is_open

    detail = client.pull("demo", 1)
    assert isinstance(detail, PullDetail) and detail.mergeable is True
    assert detail.pr.number == 1 and detail.pr.reviews[0].decision == "approve"
    assert detail.pr.comments[0].kind == "question" and detail.pr.checks[0].status == "success"
    assert detail.commits[0].id == ID_B
    assert "future_field" not in detail.pr.__dict__  # unknown fields ignored...
    assert detail.pr.raw["future_field"] == {"ignored": True}  # ...but kept on .raw

    assert client.pull("demo", 2).conflicts[0].path == "x.txt"
    with pytest.raises(NotFound):
        client.pull("demo", 42)


def test_pull_listing_and_actions(server, client):
    assert len(client.pulls("demo", state="open")) == 1
    assert server.last.q("state") == "open"
    assert client.pulls("demo", state="closed") == []

    assert client.pull_diff("demo", 1).patch == ""
    c = client.comment_pull("demo", 1, "hm", path="x.txt", line=3, kind="suggestion")
    assert server.last.json == {"path": "x.txt", "line": 3, "kind": "suggestion", "body": "hm"}
    assert c.id == 7 and c.author == "coder-1"
    assert client.resolve_comment("demo", 1, 1).comments[0].resolved is True
    assert server.last.path.endswith("/pulls/1/comments/1/resolve")

    pr = client.review_pull("demo", 1, "approve", "ship it")
    assert server.last.json == {"decision": "approve", "body": "ship it"}
    assert pr.reviews[-1].reviewer == "coder-1"
    pr = client.set_check("demo", 1, "tests", "failure", url="http://ci/1")
    assert pr.checks[0].status == "failure" and pr.checks[0].url == "http://ci/1"
    pr = client.update_pull("demo", 1, title="New title")
    assert server.last.json == {"title": "New title"} and pr.title == "New title"

    merged = client.merge_pull("demo", 1, strategy="ff")
    assert server.last.json == {"strategy": "ff"}
    assert merged.merged_commit == ID_C and merged.pr.state == "merged"
    assert client.close_pull("demo", 1).state == "closed"
    assert server.last.body == b"{}"  # POST with an empty JSON body


def test_merge_conflicts(client):
    with pytest.raises(MergeConflicts) as ei:
        client.merge_pull("demo", 2)
    assert ei.value.count == 2 and ei.value.status == 409 and ei.value.code == "merge_conflicts"


# -- policy -----------------------------------------------------------------


def test_policy(server, client):
    pol = client.policy("demo")
    assert isinstance(pol, Policy) and pol.block_on_blockers is True and pol.protected_branches == []
    pol.protected_branches = ["main"]
    pol.require_pull_request = True
    pol.require_approvals = 1
    saved = client.set_policy("demo", pol)
    assert server.last.method == "PUT" and server.last.path == "/api/v1/repos/demo/policy"
    assert server.last.json["protected_branches"] == ["main"] and server.last.json["require_approvals"] == 1
    assert set(server.last.json) == set(Policy().to_dict())  # the whole document is sent
    assert saved.require_pull_request is True and saved.require_signed is False
    pol.require_signed = True
    assert client.set_policy("demo", pol).require_signed is True
    assert server.last.json["require_signed"] is True
    client.set_policy("demo", {"protected_branches": ["main"]})  # plain dicts are accepted as-is
    assert server.last.json == {"protected_branches": ["main"]}
    with pytest.raises(PolicyViolation) as ei:
        client.set_ref("demo", "refs/heads/main", None)
    assert ei.value.code == "policy_violation" and ei.value.status == 403 and ei.value.retryable is False


# -- knowledge --------------------------------------------------------------


def test_notes(server, client):
    note = client.create_note("demo", "Title", "Body", tags=["area:auth", "kind:gotcha"], paths="src/auth/**",
                              commits=[ID_B], pulls=[1], confidence=0.8)
    assert server.last.json == {"title": "Title", "body": "Body", "tags": ["area:auth", "kind:gotcha"],
                                "paths": ["src/auth/**"], "commits": [ID_B], "pulls": [1], "confidence": 0.8}
    assert isinstance(note, Note) and note.id == "n_2" and note.author == "coder-1" and note.confidence == 0.8

    summaries = client.notes("demo", tags=["area:auth", "kind:gotcha"], path="src/auth/login.rs", q="Body", limit=5)
    q = server.last
    assert q.q("tags") == "area:auth,kind:gotcha" and q.q("path") == "src/auth/login.rs"
    assert q.q("q") == "Body" and q.q("limit") == "5" and "include_superseded" not in q.query
    assert [s.id for s in summaries] == ["n_2"] and summaries[0].excerpt == "Body"
    assert not hasattr(summaries[0], "body")

    client.notes("demo")
    assert server.last.query == {}

    full = client.note("demo", "n_1")
    assert full.body == "Long body text" and full.pulls == [1]

    patched = client.update_note("demo", "n_2", body="Better", superseded_by=None)
    assert server.last.method == "PATCH"
    assert server.last.json == {"body": "Better", "superseded_by": None}
    assert patched.body == "Better"
    client.update_note("demo", "n_2", tags=["a"])
    assert server.last.json == {"tags": ["a"]}

    assert client.delete_note("demo", "n_2") == {"deleted": "n_2"}
    with pytest.raises(NotFound):
        client.note("demo", "n_2")


def test_note_fields_v2_and_ranked_search(server, client):
    note = client.note("demo", "n_1")
    assert note.valid_at == ID_A and note.draft is False and note.stale is False

    # q= returns ranked summaries with a score; filters are post-filters on the ranking
    hits = client.notes("demo", q="importer", commit=ID_B, pull=1, drafts=True, include_superseded=True)
    q = server.last
    assert q.q("q") == "importer" and q.q("commit") == ID_B and q.q("pull") == "1"
    assert q.q("drafts") == "1" and q.q("include_superseded") == "1"
    assert [h.id for h in hits] == ["n_1"] and hits[0].score == 1.5 and hits[0].valid_at == ID_A
    assert hits[0].draft is False and hits[0].stale is False
    assert client.notes("demo", q="zzz-no-match") == []
    plain = client.notes("demo")
    assert plain[0].score is None and "drafts" not in server.last.query and "commit" not in server.last.query
    with pytest.raises(NotFound):
        client.notes("demo", commit="nope")


def test_create_note_valid_at_and_draft(server, client):
    n = client.create_note("demo", "Pinned", valid_at=ID_B)
    assert server.last.json["valid_at"] == ID_B and "draft" not in server.last.json
    assert n.valid_at == ID_B and n.draft is False
    client.create_note("demo", "Default")  # valid_at defaults to HEAD server-side; nothing is sent
    assert "valid_at" not in server.last.json
    with pytest.raises(NotFound) as ei:
        client.create_note("demo", "Bad", valid_at=ID_C)
    assert ei.value.code == "object_not_found" and ei.value.status == 412

    d = client.create_note("demo", "Draft", draft=True, pulls=[1])
    assert server.last.json["draft"] is True and d.draft is True
    assert d.id not in [x.id for x in client.notes("demo")]  # hidden from listings...
    drafts = client.notes("demo", drafts=True, pull=1)
    assert d.id in [x.id for x in drafts] and all(isinstance(x.draft, bool) for x in drafts)
    assert next(x for x in drafts if x.id == d.id).draft is True


def test_accept_and_validate_note(server, client):
    d = client.create_note("demo", "Draft", "body", draft=True)
    accepted = client.accept_note("demo", d.id)
    assert server.last.method == "POST" and server.last.path.endswith(f"/knowledge/notes/{d.id}/accept")
    assert server.last.json == {}  # no patch: plain accept
    assert accepted.draft is False and accepted.id == d.id

    d2 = client.create_note("demo", "Draft 2", draft=True)
    edited = client.accept_note("demo", d2.id, title="Better title", tags=["kind:change"], superseded_by=None)
    assert server.last.json == {"title": "Better title", "tags": ["kind:change"], "superseded_by": None}
    assert edited.title == "Better title" and edited.draft is False

    before = client.note("demo", d.id).updated_ms
    v = client.validate_note("demo", d.id)
    assert server.last.path.endswith(f"/knowledge/notes/{d.id}/validate") and server.last.json == {}
    assert v.valid_at == ID_A and v.stale is False and v.updated_ms == before + 1
    v2 = client.validate_note("demo", d.id, rev=ID_B)
    assert server.last.json == {"rev": ID_B} and v2.valid_at == ID_B
    with pytest.raises(NotFound):
        client.validate_note("demo", d.id, rev="nope")
    with pytest.raises(NotFound):
        client.accept_note("demo", "n_missing")


def test_stale_notes(server, client):
    assert client.stale_notes("demo") == []
    assert server.last.path == "/api/v1/repos/demo/knowledge/stale" and server.last.query == {}
    stale = client.stale_notes("demo", rev=ID_B, drafts=True)
    assert server.last.q("rev") == ID_B and server.last.q("drafts") == "1"
    assert len(stale) == 1 and isinstance(stale[0], StaleNote)
    s = stale[0]
    assert s.id == "n_1" and s.stale is True and s.changed_paths == ["x.txt"]
    assert s.excerpt == "Long body text" and s.valid_at == ID_A and not hasattr(s, "body")
    with pytest.raises(NotFound):
        client.stale_notes("demo", rev="nope")


def test_context_pack_and_active_agents(server, client):
    ctx = client.context("demo", "src/auth/login.rs")
    assert "include" not in server.last.query and "budget" not in server.last.query
    assert ctx.agents_active == ["coder-1", "reviewer-1"] and ctx.files == []
    assert [n.id for n in ctx.fresh_notes] == ["n_1"] and [n.id for n in ctx.stale_notes] == ["n_old"]

    pack = client.context_pack("demo", "src/auth/login.rs", budget=4000, limit=2)
    q = server.last
    assert q.q("include") == "files" and q.q("budget") == "4000" and q.q("limit") == "2"
    assert [f.path for f in pack.files] == ["src/auth/login.rs", "src/auth/sibling.rs"]
    assert isinstance(pack.files[0], ContextFile) and pack.files[0].truncated is False
    assert pack.files[0].text.startswith("fn main()")

    tight = client.context("demo", "src/auth/login.rs", include_files=True, budget=5)
    assert len(tight.files) == 1 and tight.files[0].truncated is True and len(tight.files[0].text) == 20


# -- signing keys -----------------------------------------------------------


def test_keys_roundtrip_and_commit_signature(server, client):
    assert client.keys("demo") == []
    pk = "D75A980182B10AB7D54BFED3C964073A0EE172F3DAA62325AF021A68F707511A"
    key = client.register_key("demo", "coder-1", pk)
    assert server.last.method == "POST" and server.last.path == "/api/v1/repos/demo/keys"
    assert server.last.json == {"agent": "coder-1", "public_key": pk}
    assert isinstance(key, KeyEntry) and key.agent == "coder-1" and key.key_id == "k_d75a9801"
    assert key.public_key == pk.lower() and key.added_by == "coder-1" and key.added_ms == 1000
    assert client.register_key("demo", "coder-1", pk) == key  # idempotent for the same agent
    with pytest.raises(AlreadyExists) as ei:
        client.register_key("demo", "other-agent", pk)
    assert ei.value.status == 409
    with pytest.raises(AgitError) as ei:
        client.register_key("demo", "coder-1", "not-hex")
    assert ei.value.code == "invalid"
    assert [k.key_id for k in client.keys("demo")] == [key.key_id]

    server.state.keys[0]["key_id"] = "k_1"  # the commit fixture is signed with k_1
    d = client.commit("demo", "main")
    assert isinstance(d.signature, SignatureInfo)
    assert d.signature.status == "valid" and d.signature.key_id == "k_1" and d.signature.signer == "coder-1"
    assert d.signature.trusted is True and d.signature.is_valid and d.signature.is_trusted
    assert [n.id for n in d.notes] == ["n_1"] and d.notes[0].excerpt == "Long body text"

    assert client.delete_key("demo", "k_1") == {"deleted": "k_1"}
    assert server.last.method == "DELETE" and server.last.path == "/api/v1/repos/demo/keys/k_1"
    with pytest.raises(NotFound):
        client.delete_key("demo", "k_1")
    assert client.commit("demo", "main").signature.trusted is False  # valid but no longer registered

    unsigned = client.commit("demo", "unsigned").signature
    assert unsigned.status == "unsigned" and unsigned.key_id is None and not unsigned.is_valid
    forged = client.commit("demo", "forged").signature
    assert forged.status == "invalid" and forged.reason == "signature does not verify" and not forged.is_trusted
    assert CommitDetail.from_dict({"id": ID_A}).signature is None  # older servers: field absent


def test_tags_rules_context_agents(server, client):
    tags = client.knowledge_tags("demo")
    assert server.last.path == "/api/v1/repos/demo/knowledge/tags"
    assert tags[0].tag == "area:import" and tags[0].count == 1

    assert client.rules("demo")[0].pattern == "src/auth/**"
    rules = client.set_rules("demo", [{"pattern": "web/**", "tags": ["area:web"]}])
    assert server.last.method == "PUT" and server.last.json == [{"pattern": "web/**", "tags": ["area:web"]}]
    assert rules[0].tags == ["area:web"]

    ctx = client.context("demo", "src/auth/login.rs", limit=3)
    assert server.last.q("path") == "src/auth/login.rs" and server.last.q("limit") == "3"
    assert isinstance(ctx, ContextResponse)
    assert ctx.path == "src/auth/login.rs"
    assert ctx.notes[0].id == "n_1" and ctx.recent_commits[0].id == ID_B
    assert isinstance(ctx.leases[0], Lease) and ctx.open_pulls[0].number == 1
    assert ctx.suggested_tags == ["area:import"]
    with pytest.raises(AgitError) as ei:
        client.context("demo", "")
    assert ei.value.code == "invalid" and ei.value.status == 400

    agents = client.agents("demo")
    assert agents[0].id == "coder-1" and agents[0].commits == 3


# -- misc -------------------------------------------------------------------


def test_repo_names_are_url_encoded(server, client):
    server.state.repos["a b"] = {"name": "a b", "default_branch": "main", "head": None, "created_ms": 0}
    assert client.repo("a b").name == "a b"
    assert server.last.path == "/api/v1/repos/a%20b"


def test_error_from_non_json_body():
    e = AgitError.from_http(502, b"<html>bad gateway</html>")
    assert e.code == "http_error" and e.status == 502 and e.retryable is True
    e2 = AgitError.from_http(404, b"")
    assert isinstance(e2, NotFound)


def test_lenient_dataclass_parsing():
    pr = PullRequest.from_dict({"number": 3, "unknown": 1})
    assert pr.number == 3 and pr.title == "" and pr.reviews == [] and pr.author.id == ""
    assert Event.from_dict(None).seq == 0
    assert agit.__version__ == "0.1.0"
