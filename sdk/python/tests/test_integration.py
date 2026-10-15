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
            assert "sdk-it" in ctx.agents_active  # holds a lease on the path
            assert ctx.files == []  # not requested
            pack = s.context_pack("src/lib.rs", budget=1000)
            assert pack.files == [] and pack.notes[0].id == note.id  # empty repository: nothing to pack

            # knowledge v2: ranked search, drafts, accept, validate, stale
            hits = client.notes(name, q="integration")
            assert [h.id for h in hits] == [note.id] and hits[0].score is not None and hits[0].score > 0
            assert hits[0].draft is False and hits[0].stale is False
            assert client.notes(name, pull=1) == []
            draft = client.create_note(name, "draft from sdk", "body", draft=True, pulls=[1], paths=["src/**"])
            assert draft.draft is True
            assert draft.id not in [n.id for n in client.notes(name, path="src/x.rs")]
            assert [n.id for n in client.notes(name, drafts=True, pull=1)] == [draft.id]
            assert client.knowledge_tags(name)[0].count == 1  # drafts do not count
            accepted = s.accept_note(draft, title="accepted from sdk")
            assert accepted.draft is False and accepted.title == "accepted from sdk"
            assert accepted.id in [n.id for n in client.notes(name, pull=1)]
            assert s.stale_notes() == []  # no commits yet, nothing can be stale
            assert client.delete_note(name, accepted.id) == {"deleted": accepted.id}

            # signing keys
            pk = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"  # RFC 8032 test key
            assert client.keys(name) == []
            key = client.register_key(name, "sdk-it", pk)
            assert key.agent == "sdk-it" and key.public_key == pk and key.added_by == "sdk-it" and key.key_id
            assert client.register_key(name, "sdk-it", pk).key_id == key.key_id
            with pytest.raises(AlreadyExists):
                client.register_key(name, "sdk-other", pk)
            with pytest.raises(AgitError) as ei:
                client.register_key(name, "sdk-it", "zz")
            assert ei.value.code == "invalid"
            assert [k.key_id for k in client.keys(name)] == [key.key_id]
            pol = client.policy(name)
            assert pol.require_signed is False
            pol.require_signed = True
            assert client.set_policy(name, pol).require_signed is True
            assert client.delete_key(name, key.key_id) == {"deleted": key.key_id}
            with pytest.raises(NotFound):
                client.delete_key(name, key.key_id)

            events = client.events(name, since=0)
            kinds = {e.kind for e in events}
            assert {"lease.claim", "note.create", "note.accept", "key.add", "key.remove"} <= kinds
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
