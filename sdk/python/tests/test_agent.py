from __future__ import annotations

import pytest

from agit import AgentSession, AgitError, Client, LeaseConflict, NotFound

from conftest import ID_B


def test_session_requires_agent(server, monkeypatch):
    monkeypatch.delenv("AGIT_AGENT", raising=False)
    with pytest.raises(ValueError):
        AgentSession(Client(server.base_url), "demo")
    s = AgentSession(Client(server.base_url), "demo", "coder-9", model="m")
    assert s.agent_id == "coder-9" and s.client.agent == "coder-9" and s.client.model == "m"


def test_agent_loop_releases_leases_on_exit(server, client):
    with AgentSession(client, "demo", "coder-1") as s:
        ctx = s.context("src/auth/login.rs")
        assert ctx.notes and ctx.open_pulls

        lease = s.claim(["src/auth/**"], ttl_secs=900, note="DEMO-7")
        assert lease.holder == "coder-1" and server.last.json["ttl_secs"] == 900
        s.claim("docs/")  # default ttl
        assert server.last.json["ttl_secs"] == s.default_ttl_secs
        assert len(server.state.leases) == 2 and len(s.leases) == 2

        assert s.check(["src/auth/x.rs"]) is True
        assert s.check(["src/locked.rs"]) is False
        with pytest.raises(LeaseConflict):
            s.claim(["src/locked/other.rs"])

        s.renew_all(60)
        assert all(l.expires_ms == 100 + 60_000 for l in s.leases)

        pr = s.open_pr("Add login", source="refs/heads/ws/coder-1", task="DEMO-7", intent="sign in")
        assert pr.number == 2 and pr.task == "DEMO-7" and s.pulls == [pr]
        assert server.last.headers["x-agit-agent"] == "coder-1"

        note = s.remember("login validates email first", "details", tags=["area:auth"], paths=["src/auth/**"],
                          commits=[ID_B], pulls=[pr.number])
        assert note.tags == ["area:auth"] and note.pulls == [2] and s.notes == [note]
        assert server.last.json["title"] == "login validates email first"

        assert [e.kind for e in s.events(since=2)] == ["pull.open"]

    assert server.state.leases == {} and s.leases == []
    released = [r for r in server.requests if r.method == "DELETE"]
    assert len(released) == 2


def test_release_tolerates_expired_and_does_not_mask_errors(server, client):
    s = AgentSession(client, "demo")
    lease = s.claim(["src/x"])
    server.state.leases.clear()  # expired server-side
    assert s.release_all() == [lease.id] and s.leases == []

    s.claim(["src/y"])
    with pytest.raises(RuntimeError):
        with s:
            server.stop()  # releasing will fail with a transport error, but the user's error wins
            raise RuntimeError("boom")


def test_release_single(server, client):
    s = AgentSession(client, "demo")
    a = s.claim(["a"])
    b = s.claim(["b"])
    s.release(a)
    assert [l.id for l in s.leases] == [b.id]
    s.release(b.id)
    assert s.leases == [] and server.state.leases == {}
    s.release("l_missing")  # NotFound is swallowed
