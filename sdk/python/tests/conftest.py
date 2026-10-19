"""A fake agit server (stdlib ``http.server``) with canned responses.

It implements enough of docs/API.md to exercise the client: headers, query
encoding, JSON bodies, the error envelope and SSE. Every request is recorded on
``server.requests`` so tests can assert on what was sent.
"""

from __future__ import annotations

import json
import os
import struct
import threading
import urllib.parse
from dataclasses import dataclass, field
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any

import pytest

ID_A = "a" * 64
ID_B = "b" * 64
ID_C = "c" * 64

PR = {
    "number": 1,
    "title": "Add x",
    "body": "",
    "intent": "Provide x for the importer",
    "task": "DEMO-7",
    "source": "refs/heads/ws/coder-1",
    "target": "refs/heads/main",
    "source_id": ID_B,
    "target_id": ID_A,
    "base_id": ID_A,
    "author": {"id": "coder-1", "model": "fable"},
    "state": "open",
    "created_ms": 1,
    "updated_ms": 2,
    "merged_commit": None,
    "reviews": [{"reviewer": "reviewer-1", "decision": "approve", "body": "lgtm", "time_ms": 3}],
    "comments": [
        {"id": 1, "author": "reviewer-1", "path": "x.txt", "line": 1, "kind": "question", "body": "why?",
         "resolved": False, "time_ms": 3}
    ],
    "checks": [{"name": "tests", "status": "success", "url": None, "time_ms": 4}],
    "future_field": {"ignored": True},
}

NOTE = {
    "id": "n_1",
    "title": "x.txt is consumed by the importer",
    "body": "Long body text",
    "tags": ["kind:decision", "area:import"],
    "paths": ["x.txt"],
    "commits": [ID_B],
    "pulls": [1],
    "author": "coder-1",
    "confidence": 0.9,
    "created_ms": 10,
    "updated_ms": 11,
    "superseded_by": None,
    "valid_at": ID_A,
    "stale": False,
}

KEY = {
    "agent": "coder-1",
    "key_id": "k_1",
    "public_key": "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
    "added_ms": 1000,
    "added_by": "admin",
}


def summary(note: dict, **extra: Any) -> dict:
    """Note summary as the server builds it: no body, an excerpt, draft/stale always present."""
    out = {k: v for k, v in note.items() if k != "body"}
    out["excerpt"] = note.get("body", "")[:200]
    out.setdefault("draft", False)
    out.setdefault("stale", False)
    out.update(extra)
    return out

LEASE = {
    "id": "l_1",
    "holder": "coder-1",
    "paths": ["src/auth/**"],
    "mode": "exclusive",
    "created_ms": 100,
    "expires_ms": 100 + 1800 * 1000,
    "note": "DEMO-7",
}

COMMIT = {
    "id": ID_B,
    "tree": ID_C,
    "parents": [ID_A],
    "author": {"name": "coder-1", "email": ""},
    "time_ms": 5,
    "message": "Add x",
    "intent": "Provide x",
    "task": "DEMO-7",
    "agent": {"id": "coder-1", "model": "fable"},
}

EVENTS = [
    {"seq": 1, "time_ms": 1, "kind": "repo.create", "actor": None, "data": {"name": "demo"}},
    {"seq": 2, "time_ms": 2, "kind": "ref.update", "actor": "coder-1",
     "data": {"ref": "refs/heads/main", "old": None, "new": ID_A}},
    {"seq": 3, "time_ms": 3, "kind": "pull.open", "actor": "coder-1", "data": {"number": 1}},
]


def error(code: str, message: str, retryable: bool = False, details: dict | None = None) -> dict:
    err: dict[str, Any] = {"code": code, "message": message, "retryable": retryable}
    if details is not None:
        err["details"] = details
    return {"ok": False, "error": err}


@dataclass
class Recorded:
    method: str
    path: str
    query: dict[str, list[str]]
    headers: dict[str, str]
    body: bytes

    @property
    def json(self) -> Any:
        return json.loads(self.body) if self.body else None

    def q(self, key: str) -> str | None:
        v = self.query.get(key)
        return v[0] if v else None


POLICY = {
    "protected_branches": [], "require_pull_request": False, "require_approvals": 0, "require_intent": False,
    "require_agent": False, "require_checks": False, "block_on_blockers": True, "allowed_merge_agents": [],
    "allowed_push_agents": [], "max_files_per_commit": 0, "require_signed": False,
}


NONCE = "n-" + "1" * 62
SSH_PUB = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGkmTwaq4yTAX4vyc3zq/dMzitl+O9gXb9V8+ZxCfNQD test@agit"
SSH_SIG_HEX = "ab" * 64
SESSION_TOKEN = {"id": "tok_sess", "name": "session: laptop", "agent": "coder-1",
                 "scopes": [{"repo": "demo", "access": "write"}], "created_ms": 1, "expires_ms": 29_000,
                 "created_by": "key_1", "key": "key_1"}
WHOAMI = {"principal": "tok_sess", "agent": "coder-1", "root": False,
          "scopes": [{"repo": "demo", "access": "write"}], "key": "key_1"}
AUTHORIZED_KEY = {"id": "key_1", "name": "laptop", "fingerprint": "SHA256:NeEsyNdzgZA8xAHH2Nw7ehNrSS7b6gYEjQl2qlIHrqM",
                  "key_id": "0123456789abcdef", "public_key": SSH_PUB, "agent": "coder-1",
                  "scopes": [{"repo": "demo", "access": "write"}], "created_ms": 1, "created_by": "root",
                  "last_used_ms": None}


@dataclass
class FakeState:
    requests: list[Recorded] = field(default_factory=list)
    policy: dict = field(default_factory=lambda: dict(POLICY))
    webhooks: list = field(default_factory=list)
    repos: dict[str, dict] = field(default_factory=dict)
    notes: dict[str, dict] = field(default_factory=dict)
    keys: list[dict] = field(default_factory=list)
    leases: dict[str, dict] = field(default_factory=dict)
    next_note: int = 1
    next_lease: int = 1
    token: str | None = None


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    state: FakeState  # set by the fixture

    def log_message(self, *_: Any) -> None:  # silence
        pass

    # -- plumbing -----------------------------------------------------------

    def _record(self) -> Recorded:
        parsed = urllib.parse.urlsplit(self.path)
        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length) if length else b""
        rec = Recorded(
            self.command,
            parsed.path,
            urllib.parse.parse_qs(parsed.query, keep_blank_values=True),
            {k.lower(): v for k, v in self.headers.items()},
            body,
        )
        self.state.requests.append(rec)
        return rec

    def _send(self, status: int, payload: Any = None, *, raw: bytes | None = None, ctype: str = "application/json",
              extra: dict[str, str] | None = None) -> None:
        data = raw if raw is not None else (json.dumps(payload).encode() if payload is not None else b"")
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        for k, v in (extra or {}).items():
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(data)

    def _err(self, status: int, code: str, message: str, retryable: bool = False, details: dict | None = None) -> None:
        self._send(status, error(code, message, retryable, details))

    def _authorized(self, rec: Recorded) -> bool:
        if self.state.token is None:
            return True
        return rec.headers.get("authorization") == f"Bearer {self.state.token}"

    def do_GET(self) -> None:
        self._dispatch()

    def do_POST(self) -> None:
        self._dispatch()

    def do_PATCH(self) -> None:
        self._dispatch()

    def do_PUT(self) -> None:
        self._dispatch()

    def do_DELETE(self) -> None:
        self._dispatch()

    # -- routes -------------------------------------------------------------

    def _dispatch(self) -> None:
        rec = self._record()
        if not rec.path.startswith("/api/v1/"):
            return self._err(404, "not_found", "no such route")
        parts = [urllib.parse.unquote(p) for p in rec.path[len("/api/v1/"):].split("/")]
        m = rec.method
        st = self.state

        # SSH sign-in is public: a challenge, then a signed session request.
        if parts == ["auth", "challenge"] and m == "POST":
            return self._send(200, {"nonce": NONCE, "expires_ms": 120_000})
        if parts == ["auth", "session"] and m == "POST":
            body = rec.json
            if body.get("nonce") != NONCE:
                return self._err(403, "forbidden", "challenge is unknown, already used or expired")
            if body.get("public_key") != SSH_PUB:
                return self._err(403, "forbidden", "ssh key SHA256:x is not authorized on this server")
            if body.get("signature") != SSH_SIG_HEX:
                return self._err(403, "forbidden", "signature does not match the challenge")
            return self._send(201, {**SESSION_TOKEN, "token": "agit_session",
                                    "expires_ms": 1_000 + int(body.get("ttl_secs") or 28_800) * 1000})

        if not self._authorized(rec):
            return self._err(401, "unauthorized", "missing bearer token")

        if parts[0] == "auth":
            if parts == ["auth", "whoami"] and m == "GET":
                return self._send(200, WHOAMI)
            if parts == ["auth", "logout"] and m == "POST":
                return self._send(200, {"revoked": WHOAMI["principal"]})
            if parts == ["auth", "tokens"]:
                if m == "GET":
                    return self._send(200, [SESSION_TOKEN])
                if m == "POST":
                    body = rec.json
                    if not body.get("scopes"):
                        return self._err(400, "invalid", "at least one scope is required")
                    return self._send(201, {"id": "tok_new", "name": body["name"], "agent": body.get("agent"),
                                            "scopes": body["scopes"], "created_ms": 2, "created_by": "root",
                                            "expires_ms": body.get("ttl_secs") and 2 + body["ttl_secs"] * 1000,
                                            "token": "agit_new"})
            if len(parts) == 3 and parts[1] == "tokens" and m == "DELETE":
                return self._send(200, {"deleted": parts[2]})
            if parts == ["auth", "keys"]:
                if m == "GET":
                    return self._send(200, [AUTHORIZED_KEY])
                if m == "POST":
                    body = rec.json
                    if body.get("public_key") == AUTHORIZED_KEY["public_key"]:
                        return self._err(409, "already_exists", "key is already authorized as key_1")
                    return self._send(201, {**AUTHORIZED_KEY, "id": "key_2", "name": body.get("name") or "SHA256:y",
                                            "agent": body.get("agent"), "scopes": body["scopes"], "fingerprint": "SHA256:y",
                                            "public_key": body["public_key"]})
            if len(parts) == 3 and parts[1] == "keys" and m == "DELETE":
                return self._send(200, {"deleted": parts[2], "revoked_sessions": 1})

        if parts == ["health"]:
            return self._send(200, {"status": "ok", "version": "0.1.0"})

        if parts == ["repos"]:
            if m == "GET":
                return self._send(200, list(st.repos.values()))
            if m == "POST":
                body = rec.json
                name = body["name"]
                if name in st.repos:
                    return self._err(409, "already_exists", f"repository {name} already exists")
                st.repos[name] = {"name": name, "default_branch": body.get("default_branch") or "main",
                                  "head": None, "created_ms": 1}
                return self._send(201, st.repos[name])

        if len(parts) >= 2 and parts[0] == "repos":
            repo = parts[1]
            rest = parts[2:]
            if repo not in st.repos:
                return self._err(404, "not_found", f"repository {repo} not found")
            if not rest:
                if m == "GET":
                    return self._send(200, {**st.repos[repo], "branches": 1, "tags": 0, "objects": 3})
                if m == "DELETE":
                    del st.repos[repo]
                    return self._send(200, {"deleted": True})

            if rest == ["refs"] and m == "GET":
                refs = [{"name": "refs/heads/main", "id": ID_A}, {"name": "refs/tags/v1", "id": ID_B}]
                prefix = rec.q("prefix") or "refs/"
                return self._send(200, [r for r in refs if r["name"].startswith(prefix)])

            if rest == ["refs", "update"] and m == "POST":
                updates = rec.json["updates"]
                for u in updates:
                    exp = u["expect"]
                    if u["new"] is None and u["name"].split("/")[-1] in st.policy["protected_branches"]:
                        return self._err(403, "policy_violation", f"{u['name']} is protected")
                    if "is" in exp and exp["is"] != ID_A:
                        return self._err(409, "ref_conflict", "reference changed concurrently", True,
                                         {"name": u["name"], "expected": exp["is"], "actual": ID_A})
                    if u["new"] == ID_C:
                        return self._err(412, "object_not_found", f"object {ID_C} not found")
                return self._send(200, {
                    "updated": [{"name": u["name"], "id": u["new"]} for u in updates if u["new"]],
                    "deleted": [u["name"] for u in updates if not u["new"]],
                })

            if rest == ["objects", "missing"] and m == "POST":
                return self._send(200, {"missing": [i for i in rec.json["ids"] if i != ID_A]})

            if rest == ["objects"] and m == "POST":
                if rec.headers.get("content-type") != "application/x-agit-objects":
                    return self._err(400, "invalid", "wrong content type")
                count, off = 0, 0
                while off + 5 <= len(rec.body):
                    (ln,) = struct.unpack(">I", rec.body[off + 1:off + 5])
                    off += 5 + ln
                    count += 1
                return self._send(200, {"stored": count, "skipped": 0})

            if rest == ["fetch"] and m == "POST":
                payload = b"hello"
                frame = bytes([1]) + struct.pack(">I", len(payload)) + payload
                return self._send(200, raw=frame, ctype="application/x-agit-objects")

            if len(rest) == 2 and rest[0] == "objects" and m == "GET":
                return self._send(200, raw=b"hello", ctype="application/octet-stream", extra={"X-Agit-Kind": "blob"})

            if rest == ["commits"] and m == "GET":
                return self._send(200, [COMMIT])
            if len(rest) == 2 and rest[0] == "commits" and m == "GET":
                if rest[1] == "unsigned":
                    signature: dict[str, Any] = {"status": "unsigned", "trusted": False}
                elif rest[1] == "forged":
                    signature = {"status": "invalid", "reason": "signature does not verify", "trusted": False}
                else:
                    signature = {"status": "valid", "key_id": "k_1", "signer": "coder-1",
                                 "public_key": KEY["public_key"], "trusted": any(k["key_id"] == "k_1" for k in st.keys)}
                return self._send(200, {"id": ID_B, "commit": {k: v for k, v in COMMIT.items() if k != "id"},
                                        "changes": [{"path": "x.txt", "kind": "added", "new": {"id": ID_C, "mode": "file"}}],
                                        "patch": "+x", "signature": signature,
                                        "notes": [summary(NOTE)] + [summary(n) for n in st.notes.values()
                                                                     if ID_B in n.get("commits", [])]})
            if len(rest) == 2 and rest[0] == "tree" and m == "GET":
                if rec.q("path"):
                    return self._send(200, {"kind": "blob", "path": rec.q("path"), "id": ID_C, "size": 2,
                                            "binary": False, "text": "x\n"})
                return self._send(200, {"kind": "tree", "path": "", "id": ID_C,
                                        "entries": [{"name": "x.txt", "mode": "file", "id": ID_C, "size": 2}]})
            if rest == ["diff"] and m == "GET":
                return self._send(200, {"changes": [], "patch": "", "stats": [{"path": "x.txt", "added": 1, "removed": 0}]})
            if rest == ["branches"] and m == "GET":
                return self._send(200, [{"name": "main", "id": ID_A}, {"name": "ws/coder-1", "id": ID_B, "ahead": 1, "behind": 0}])
            if rest == ["tags"] and m == "GET":
                return self._send(200, [{"name": "v1", "id": ID_B}])

            if rest == ["events"] and m == "GET":
                since = int(rec.q("since") or 0)
                limit = int(rec.q("limit") or 500)
                return self._send(200, [e for e in EVENTS if e["seq"] > since][:limit])

            if rest == ["events", "stream"] and m == "GET":
                since = int(rec.q("since") or 0)
                chunks = [b": keep-alive\n\n"]
                for e in EVENTS:
                    if e["seq"] > since:
                        chunks.append(f"id: {e['seq']}\nevent: {e['kind']}\ndata: {json.dumps(e)}\n\n".encode())
                # one multi-line data event to exercise the parser
                chunks.append(b"data: {\"seq\": 99,\ndata:  \"kind\": \"multi.line\", \"time_ms\": 9, \"data\": null}\n\n")
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Cache-Control", "no-cache")
                self.send_header("Connection", "close")
                self.end_headers()
                for c in chunks:
                    self.wfile.write(c)
                    self.wfile.flush()
                self.close_connection = True
                return None

            if rest == ["leases"]:
                if m == "GET":
                    return self._send(200, list(st.leases.values()))
                if m == "POST":
                    body = rec.json
                    for p in body["paths"]:
                        if p.startswith("src/locked"):
                            return self._err(409, "lease_conflict", f"{p} is leased by other-agent", False,
                                             {"path": p, "holder": "other-agent", "lease_id": "l_other"})
                    lid = f"l_{st.next_lease}"
                    st.next_lease += 1
                    lease = {**LEASE, "id": lid, "holder": body["holder"], "paths": body["paths"],
                             "mode": body.get("mode", "exclusive"), "note": body.get("note"),
                             "expires_ms": 100 + (body.get("ttl_secs") or 0) * 1000}
                    st.leases[lid] = lease
                    return self._send(201, lease)
            if rest == ["leases", "check"] and m == "POST":
                for p in rec.json["paths"]:
                    if p.startswith("src/locked"):
                        return self._err(409, "lease_conflict", "leased", False,
                                         {"path": p, "holder": "other-agent", "lease_id": "l_other"})
                return self._send(200, {"allowed": True, "holder": rec.json["holder"], "paths": rec.json["paths"]})
            if len(rest) == 2 and rest[0] == "leases" and m == "DELETE":
                if rest[1] not in st.leases:
                    return self._err(404, "lease_not_found", f"lease {rest[1]} not found")
                del st.leases[rest[1]]
                return self._send(200, {"released": rest[1]})
            if len(rest) == 3 and rest[0] == "leases" and rest[2] == "renew" and m == "POST":
                if rest[1] not in st.leases:
                    return self._err(404, "lease_not_found", f"lease {rest[1]} not found")
                st.leases[rest[1]]["expires_ms"] = 100 + rec.json["ttl_secs"] * 1000
                return self._send(200, st.leases[rest[1]])

            if rest == ["pulls"]:
                if m == "GET":
                    return self._send(200, [PR] if rec.q("state") in (None, "open") else [])
                if m == "POST":
                    body = rec.json
                    pr = {**PR, "number": 2, "title": body["title"], "body": body.get("body", ""),
                          "intent": body.get("intent"), "task": body.get("task"), "source": body["source"],
                          "target": body.get("target") or "refs/heads/main",
                          "author": {"id": rec.headers.get("x-agit-agent", "anonymous"),
                                     "model": rec.headers.get("x-agit-model")},
                          "reviews": [], "comments": [], "checks": []}
                    return self._send(201, pr)
            if len(rest) >= 2 and rest[0] == "pulls":
                n = int(rest[1])
                if n not in (1, 2):
                    return self._err(404, "not_found", f"pull request {n} not found")
                tail = rest[2:]
                if not tail and m == "GET":
                    return self._send(200, {"pr": PR, "commits": [COMMIT], "changes": [], "mergeable": n == 1,
                                            "conflicts": [] if n == 1 else [{"path": "x.txt", "kind": "content"}]})
                if tail == ["diff"]:
                    return self._send(200, {"changes": [], "patch": "", "stats": []})
                if tail == ["comments"] and m == "POST":
                    return self._send(201, {"id": 7, "author": rec.headers.get("x-agit-agent", "anonymous"),
                                            "kind": rec.json.get("kind", "note"), "body": rec.json["body"],
                                            "path": rec.json.get("path"), "line": rec.json.get("line"),
                                            "resolved": False, "time_ms": 1})
                if len(tail) == 3 and tail[0] == "comments" and tail[2] == "resolve":
                    return self._send(200, {**PR, "comments": [{**PR["comments"][0], "resolved": True}]})
                if tail == ["reviews"]:
                    return self._send(200, {**PR, "reviews": PR["reviews"] + [
                        {"reviewer": rec.headers.get("x-agit-agent", "anonymous"), "decision": rec.json["decision"],
                         "body": rec.json.get("body", ""), "time_ms": 9}]})
                if tail == ["checks"]:
                    return self._send(200, {**PR, "checks": [{"name": rec.json["name"], "status": rec.json["status"],
                                                              "url": rec.json.get("url"), "time_ms": 9}]})
                if tail == ["merge"]:
                    if n == 2:
                        return self._err(409, "merge_conflicts", "merge stopped with 2 conflicts", False, {"count": 2})
                    return self._send(200, {"pr": {**PR, "state": "merged", "merged_commit": ID_C}, "merged_commit": ID_C})
                if tail == ["close"]:
                    return self._send(200, {**PR, "state": "closed"})
                if tail == ["update"]:
                    return self._send(200, {**PR, **{k: v for k, v in rec.json.items()}})

            if rest == ["webhooks"]:
                if m == "GET":
                    return self._send(200, st.webhooks)
                if m == "POST":
                    if not str(rec.json.get("url", "")).startswith(("http://", "https://")):
                        return self._err(400, "invalid", "webhook url must be http(s)")
                    wh = {"id": f"wh_{len(st.webhooks) + 1}", "url": rec.json["url"], "kinds": rec.json.get("kinds", []),
                          "active": True, "has_secret": bool(rec.json.get("secret")), "created_ms": 1700000000000}
                    st.webhooks.append(wh)
                    return self._send(201, wh)
            if len(rest) == 2 and rest[0] == "webhooks" and m == "DELETE":
                before = len(st.webhooks)
                st.webhooks = [w for w in st.webhooks if w["id"] != rest[1]]
                if len(st.webhooks) == before:
                    return self._err(404, "not_found", f"webhook {rest[1]}")
                return self._send(200, {"deleted": rest[1]})
            if len(rest) == 3 and rest[0] == "webhooks" and rest[2] == "test" and m == "POST":
                return self._send(200, {"queued": True})

            if rest == ["keys"]:
                if m == "GET":
                    return self._send(200, st.keys)
                if m == "POST":
                    body = rec.json
                    pk = str(body.get("public_key", "")).strip().lower()
                    if len(pk) != 64 or any(c not in "0123456789abcdef" for c in pk):
                        return self._err(400, "invalid", "public key must be 32 bytes of hex")
                    key_id = "k_" + pk[:8]
                    for k in st.keys:
                        if k["key_id"] == key_id:
                            if k["agent"] == body["agent"]:
                                return self._send(201, k)
                            return self._err(409, "already_exists", f"key {key_id} is registered for {k['agent']}")
                    entry = {"agent": body["agent"], "key_id": key_id, "public_key": pk, "added_ms": 1000,
                             "added_by": rec.headers.get("x-agit-agent", "anonymous")}
                    st.keys.append(entry)
                    return self._send(201, entry)
            if len(rest) == 2 and rest[0] == "keys" and m == "DELETE":
                before = len(st.keys)
                st.keys = [k for k in st.keys if k["key_id"] != rest[1]]
                if len(st.keys) == before:
                    return self._err(404, "not_found", f"key {rest[1]} not found")
                return self._send(200, {"deleted": rest[1]})

            if rest == ["policy"]:
                if m == "GET":
                    return self._send(200, st.policy)
                if m == "PUT":
                    st.policy = {**POLICY, **rec.json}
                    return self._send(200, st.policy)

            if rest == ["knowledge", "notes"]:
                if m == "GET":
                    if rec.q("commit") == "nope":
                        return self._err(404, "unknown_revision", "unknown revision nope")
                    out = []
                    for note in list(st.notes.values()) + [NOTE]:
                        if rec.q("tags") and not set(rec.q("tags").split(",")) <= set(note["tags"]):
                            continue
                        if rec.q("path") and not any(note_path.split("/**")[0] in rec.q("path") for note_path in note["paths"]):
                            continue
                        if rec.q("commit") and rec.q("commit") not in note.get("commits", []):
                            continue
                        if rec.q("pull") and int(rec.q("pull")) not in note.get("pulls", []):
                            continue
                        if note.get("draft") and rec.q("drafts") not in ("1", "true"):
                            continue
                        if note.get("superseded_by") and rec.q("include_superseded") not in ("1", "true"):
                            continue
                        q = rec.q("q")
                        if q:
                            hay = " ".join([note["title"], note["body"], *note["tags"], *note["paths"]]).lower()
                            if q.lower() not in hay:
                                continue
                            out.append(summary(note, score=1.5 if q.lower() in note["title"].lower() else 0.5))
                        else:
                            out.append(summary(note))
                    if rec.q("q"):
                        out.sort(key=lambda n: -n["score"])
                    return self._send(200, out[: int(rec.q("limit") or 100)])
                if m == "POST":
                    body = rec.json
                    if body.get("valid_at") == ID_C:
                        return self._err(412, "object_not_found", f"object {ID_C} not found")
                    nid = f"n_{st.next_note + 1}"
                    st.next_note += 1
                    note = {**NOTE, "pulls": [], "commits": [], **body, "id": nid,
                            "author": rec.headers.get("x-agit-agent", "anonymous")}
                    note.setdefault("confidence", 1.0)
                    note.setdefault("valid_at", ID_A)
                    note["stale"] = False
                    if not note.get("draft"):
                        note.pop("draft", None)  # the server omits `draft` when false
                    st.notes[nid] = note
                    return self._send(201, note)
            if rest == ["knowledge", "stale"] and m == "GET":
                rev = rec.q("rev") or "HEAD"
                if rev == "nope":
                    return self._err(404, "unknown_revision", "unknown revision nope")
                out = []
                for note in [NOTE] + list(st.notes.values()):
                    if note.get("superseded_by"):
                        continue
                    if note.get("draft") and rec.q("drafts") not in ("1", "true"):
                        continue
                    if note.get("valid_at") in (ID_A, None) and rev != ID_B:
                        continue  # valid at HEAD: nothing changed
                    out.append(summary(note, stale=True, changed_paths=[p.replace("**", "login.rs") for p in note["paths"]]))
                return self._send(200, out)
            if len(rest) == 3 and rest[:2] == ["knowledge", "notes"]:
                nid = rest[2]
                note = st.notes.get(nid) or (NOTE if nid == "n_1" else None)
                if note is None:
                    return self._err(404, "not_found", f"note {nid} not found")
                if m == "GET":
                    return self._send(200, note)
                if m == "PATCH":
                    note = {**note, **rec.json}
                    st.notes[nid] = note
                    return self._send(200, note)
                if m == "DELETE":
                    st.notes.pop(nid, None)
                    return self._send(200, {"deleted": nid})
            if len(rest) == 4 and rest[:2] == ["knowledge", "notes"] and m == "POST":
                nid, action = rest[2], rest[3]
                note = st.notes.get(nid) or (NOTE if nid == "n_1" else None)
                if note is None:
                    return self._err(404, "not_found", f"note {nid} not found")
                body = rec.json or {}
                if action == "validate":
                    rev = body.get("rev") or "HEAD"
                    if rev == "nope":
                        return self._err(404, "unknown_revision", "unknown revision nope")
                    note = {**note, "valid_at": ID_A if rev == "HEAD" else rev, "stale": False,
                            "updated_ms": note["updated_ms"] + 1}
                    st.notes[nid] = note
                    return self._send(200, note)
                if action == "accept":
                    note = {**note, **body, "updated_ms": note["updated_ms"] + 1}
                    note.pop("draft", None)
                    st.notes[nid] = note
                    return self._send(200, note)
            if rest == ["knowledge", "tags"]:
                return self._send(200, [{"tag": "area:import", "count": 1}, {"tag": "kind:decision", "count": 1}])
            if rest == ["knowledge", "rules"]:
                if m == "GET":
                    return self._send(200, [{"pattern": "src/auth/**", "tags": ["area:auth"]}])
                if m == "PUT":
                    return self._send(200, rec.json)
            if rest == ["context"] and m == "GET":
                if not rec.q("path"):
                    return self._err(400, "invalid", "missing `path`")
                path = rec.q("path")
                ctx = {
                    "path": path,
                    "notes": [summary(NOTE, excerpt="Long"), summary(NOTE, id="n_old", stale=True, excerpt="Old")],
                    "suggested_tags": ["area:import"],
                    "recent_commits": [COMMIT],
                    "leases": [LEASE],
                    "open_pulls": [PR],
                    "agents_active": ["coder-1", "reviewer-1"],
                }
                if "files" in (rec.q("include") or "").split(","):
                    budget = int(rec.q("budget") or 6000)
                    text = "fn main() {}\n" * 10
                    files = [{"path": path, "text": text, "truncated": False},
                             {"path": path.rsplit("/", 1)[0] + "/sibling.rs", "text": "// sib\n", "truncated": False}]
                    if budget * 4 < len(text):
                        files = [{"path": path, "text": text[: budget * 4], "truncated": True}]
                    ctx["files"] = files
                return self._send(200, ctx)
            if rest == ["agents"]:
                return self._send(200, [{"id": "coder-1", "model": "fable", "commits": 3, "last_seen_ms": 5,
                                         "open_pulls": 1, "leases": 1}])

        return self._err(404, "not_found", f"no route for {m} {rec.path}")


class FakeServer:
    def __init__(self) -> None:
        self.state = FakeState()
        handler = type("BoundHandler", (Handler,), {"state": self.state})
        self.httpd = ThreadingHTTPServer(("127.0.0.1", 0), handler)
        self.httpd.daemon_threads = True
        self.thread = threading.Thread(target=self.httpd.serve_forever, kwargs={"poll_interval": 0.02}, daemon=True)
        self.thread.start()
        host, port = self.httpd.server_address[:2]
        self.base_url = f"http://{host}:{port}/api/v1"
        self.state.repos["demo"] = {"name": "demo", "default_branch": "main", "head": ID_A, "created_ms": 1}

    @property
    def requests(self) -> list[Recorded]:
        return self.state.requests

    @property
    def last(self) -> Recorded:
        return self.state.requests[-1]

    def stop(self) -> None:
        self.httpd.shutdown()
        self.httpd.server_close()
        self.thread.join(timeout=5)


@pytest.fixture
def server():
    s = FakeServer()
    try:
        yield s
    finally:
        s.stop()


@pytest.fixture
def client(server, monkeypatch):
    for var in ("AGIT_TOKEN", "AGIT_AGENT", "AGIT_MODEL", "AGIT_SERVER_URL"):
        monkeypatch.delenv(var, raising=False)
    from agit import Client

    return Client(server.base_url, agent="coder-1", model="fable", timeout=5)


@pytest.fixture
def server_url() -> str:
    url = os.environ.get("AGIT_SERVER_URL")
    if not url:
        pytest.skip("AGIT_SERVER_URL not set")
    return url
