"""End-to-end against a real ``agit-server``. Runs only when ``AGIT_SERVER_URL`` is set, e.g.

    agit-server --data-dir /tmp/agit-data --listen 127.0.0.1:7878 &
    AGIT_SERVER_URL=http://127.0.0.1:7878/api/v1 pytest -m integration
"""

from __future__ import annotations

import time
import uuid

import pytest

from agit import AgentSession, AgitError, AlreadyExists, Client, LeaseConflict, NotFound

pytestmark = pytest.mark.integration


def test_roundtrip_against_real_server(server_url):
    client = Client(server_url, agent="sdk-it", model="pytest")
    assert client.health().status == "ok"

    name = f"sdk-it-{uuid.uuid4().hex[:8]}"
    repo = client.create_repo(name)
    assert repo.name == name and repo.default_branch == "main"
    with pytest.raises(AlreadyExists):
        client.create_repo(name)
    try:
        assert any(r.name == name for r in client.list_repos())
        assert client.refs(name) == []
        assert client.branches(name) == []

        with AgentSession(client, name, "sdk-it") as s:
            lease = s.claim(["src/**"], ttl_secs=60, note="integration")
            assert lease.holder == "sdk-it"
            other = Client(server_url, agent="sdk-other")
            with pytest.raises(LeaseConflict) as ei:
                other.claim_lease(name, "sdk-other", ["src/main.rs"])
            assert ei.value.holder == "sdk-it"
            assert other.check_lease(name, "sdk-other", ["docs/x"])["allowed"] is True
            me = [a for a in client.agents(name) if a.id == "sdk-it"]  # active leases count as activity
            assert me and me[0].leases == 1

            note = s.remember("integration note", "body", tags=["kind:test"], paths=["src/**"])
            assert client.note(name, note.id).title == "integration note"
            assert [n.id for n in client.notes(name, tags=["kind:test"])] == [note.id]
            assert [n.id for n in client.notes(name, path="src/lib.rs")] == [note.id]
            assert client.knowledge_tags(name)[0].tag == "kind:test"
            rules = client.set_rules(name, [{"pattern": "src/**", "tags": ["area:core"]}])
            assert rules[0].tags == ["area:core"]

            ctx = s.context("src/lib.rs")
            assert ctx.path == "src/lib.rs"
            assert [n.id for n in ctx.notes] == [note.id]
            assert [l.id for l in ctx.leases] == [lease.id]
            assert "area:core" in ctx.suggested_tags

            events = client.events(name, since=0)
            kinds = {e.kind for e in events}
            assert {"lease.claim", "note.create"} <= kinds
            last_seq = events[-1].seq

            gen = client.stream_events(name, since=last_seq - 1, timeout=10)
            assert next(gen).seq == last_seq
            gen.close()

        assert client.leases(name) == []
        assert not any(a.id == "sdk-it" for a in client.agents(name))  # no commits, lease released
        # The server currently answers 400 `invalid` for an unknown PR number; API.md only
        # promises the envelope, so accept either code but require a typed error.
        with pytest.raises(AgitError) as ei:
            client.pull(name, 999)
        assert ei.value.code in ("not_found", "invalid") and ei.value.status in (400, 404)
    finally:
        assert client.delete_repo(name) == {"deleted": True}
    time.sleep(0.05)
    with pytest.raises(NotFound):
        client.repo(name)
