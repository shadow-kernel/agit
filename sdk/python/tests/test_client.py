from __future__ import annotations

import struct

import pytest

import agit
from agit import (
    AgitError,
    AlreadyExists,
    Client,
    ContextResponse,
    Event,
    Lease,
    LeaseConflict,
    MergeConflicts,
    NotFound,
    Note,
    PullDetail,
    PullRequest,
    RefConflict,
    RefUpdate,
    TransportError,
    expect_absent,
    expect_any,
    expect_is,
)

from conftest import ID_A, ID_B, ID_C


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
    with pytest.raises(AgitError) as ei:
        client.list_repos()
    assert ei.value.code == "forbidden" and ei.value.status == 403
    assert client.with_token("right").list_repos()[0].name == "demo"


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
