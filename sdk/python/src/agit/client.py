"""Blocking HTTP client for the agit server API (``docs/API.md``).

Only the standard library is used (``urllib.request`` + ``json``). The method
set mirrors ``crates/agit-protocol/src/client.rs``; every endpoint in API.md is
covered.

    >>> from agit import Client
    >>> c = Client("http://127.0.0.1:7878/api/v1", agent="coder-1")
    >>> c.health().status
    'ok'
"""

from __future__ import annotations

import json
import os
import socket
import urllib.error
import urllib.parse
import urllib.request
from typing import Any, Iterator, Mapping, Sequence

from .errors import AgitError, TransportError
from .types import (
    AgentActivity,
    BranchEntry,
    Comment,
    CommitDetail,
    CommitEntry,
    ContextResponse,
    DiffResponse,
    Event,
    Health,
    Lease,
    MergeResponse,
    Note,
    NoteSummary,
    Policy,
    PullDetail,
    PullRequest,
    RefEntry,
    RefUpdate,
    RefUpdateResponse,
    RepoSummary,
    TagCount,
    TagRule,
    as_list,
    expect_absent,
    expect_any,
    expect_is,
    Webhook,
)

__version__ = "0.1.0"

DEFAULT_BASE_URL = "http://127.0.0.1:7878/api/v1"
OBJECT_STREAM_CONTENT_TYPE = "application/x-agit-objects"
USER_AGENT = f"agit-sdk-python/{__version__}"

_UNSET: Any = object()


def _seg(value: Any) -> str:
    """Percent-encode one path segment (``/`` included, so ``refs/heads/x`` stays one segment)."""
    return urllib.parse.quote(str(value), safe="")


def _query(params: Mapping[str, Any] | None) -> str:
    """Encode query parameters, dropping ``None`` values. Lists are comma-joined."""
    if not params:
        return ""
    items: list[tuple[str, str]] = []
    for key, value in params.items():
        if value is None:
            continue
        if isinstance(value, bool):
            value = "true" if value else "false"
        elif isinstance(value, (list, tuple, set, frozenset)):
            if not value:
                continue
            value = ",".join(str(v) for v in value)
        items.append((key, str(value)))
    if not items:
        return ""
    return "?" + urllib.parse.urlencode(items, quote_via=urllib.parse.quote)


def _drop_none(d: Mapping[str, Any]) -> dict[str, Any]:
    return {k: v for k, v in d.items() if v is not None}


class Client:
    """Client for one agit server.

    Args:
        base_url: Server base including ``/api/v1``. ``None`` falls back to
            ``$AGIT_SERVER_URL`` and then to :data:`DEFAULT_BASE_URL`.
        token: Bearer token (default ``$AGIT_TOKEN``). Sent as ``Authorization``.
        agent: Acting agent id (default ``$AGIT_AGENT``). Sent as ``X-Agit-Agent``.
        model: Model name (default ``$AGIT_MODEL``). Sent as ``X-Agit-Model``.
        timeout: Socket timeout in seconds for ordinary requests.
    """

    def __init__(
        self,
        base_url: str | None = DEFAULT_BASE_URL,
        token: str | None = None,
        agent: str | None = None,
        model: str | None = None,
        timeout: float | None = 60,
        *,
        extra_headers: Mapping[str, str] | None = None,
        opener: urllib.request.OpenerDirector | None = None,
    ) -> None:
        if base_url is None:
            base_url = os.environ.get("AGIT_SERVER_URL") or DEFAULT_BASE_URL
        self.base_url = base_url.rstrip("/")
        self.token = token if token is not None else (os.environ.get("AGIT_TOKEN") or None)
        self.agent = agent if agent is not None else (os.environ.get("AGIT_AGENT") or None)
        self.model = model if model is not None else (os.environ.get("AGIT_MODEL") or None)
        self.timeout = timeout
        self.extra_headers = dict(extra_headers or {})
        self._opener = opener or urllib.request.build_opener()

    def __repr__(self) -> str:  # pragma: no cover - cosmetic
        return f"Client(base_url={self.base_url!r}, agent={self.agent!r})"

    # -- derived clients (mirror the Rust builder methods) ------------------

    def _copy(self, **changes: Any) -> "Client":
        c = Client.__new__(Client)
        c.__dict__.update(self.__dict__)
        c.__dict__.update(changes)
        return c

    def with_agent(self, agent: str | None) -> "Client":
        return self._copy(agent=agent) if agent is not None else self

    def with_token(self, token: str | None) -> "Client":
        return self._copy(token=token) if token is not None else self

    def with_model(self, model: str | None) -> "Client":
        return self._copy(model=model) if model is not None else self

    # -- transport ----------------------------------------------------------

    def _headers(self, extra: Mapping[str, str] | None = None) -> dict[str, str]:
        h = {"Accept": "application/json", "User-Agent": USER_AGENT}
        if self.token:
            h["Authorization"] = f"Bearer {self.token}"
        if self.agent:
            h["X-Agit-Agent"] = self.agent
        if self.model:
            h["X-Agit-Model"] = self.model
        h.update(self.extra_headers)
        if extra:
            h.update(extra)
        return h

    def url(self, path: str, params: Mapping[str, Any] | None = None) -> str:
        return f"{self.base_url}{path}{_query(params)}"

    def _open(
        self,
        method: str,
        path: str,
        *,
        params: Mapping[str, Any] | None = None,
        json_body: Any = _UNSET,
        data: bytes | None = None,
        headers: Mapping[str, str] | None = None,
        timeout: float | None = _UNSET,
    ):
        hdrs = dict(headers or {})
        body: bytes | None = None
        if json_body is not _UNSET:
            body = json.dumps(json_body, separators=(",", ":")).encode("utf-8")
            hdrs.setdefault("Content-Type", "application/json")
        elif data is not None:
            body = data
        req = urllib.request.Request(self.url(path, params), data=body, method=method, headers=self._headers(hdrs))
        if timeout is _UNSET:
            timeout = self.timeout
        try:
            return self._opener.open(req, timeout=timeout)
        except urllib.error.HTTPError as e:
            raw = e.read() if hasattr(e, "read") else b""
            raise AgitError.from_http(e.code, raw, reason=str(e.reason)) from None
        except (urllib.error.URLError, socket.timeout, TimeoutError, ConnectionError, OSError) as e:
            reason = getattr(e, "reason", e)
            raise TransportError(
                f"transport error talking to {self.base_url}: {reason}",
                code="transport",
                retryable=True,
            ) from e

    def request(
        self,
        method: str,
        path: str,
        *,
        params: Mapping[str, Any] | None = None,
        json_body: Any = _UNSET,
        data: bytes | None = None,
        headers: Mapping[str, str] | None = None,
    ) -> Any:
        """Perform a request and return the decoded JSON body (``None`` when empty)."""
        with self._open(method, path, params=params, json_body=json_body, data=data, headers=headers) as resp:
            raw = resp.read()
        if not raw:
            return None
        try:
            return json.loads(raw.decode("utf-8"))
        except ValueError as e:
            raise AgitError(f"bad json from server: {e}", code="bad_json") from None

    def request_bytes(
        self,
        method: str,
        path: str,
        *,
        params: Mapping[str, Any] | None = None,
        json_body: Any = _UNSET,
        data: bytes | None = None,
        headers: Mapping[str, str] | None = None,
    ) -> tuple[bytes, dict[str, str]]:
        """Perform a request and return ``(body, response headers)``."""
        with self._open(method, path, params=params, json_body=json_body, data=data, headers=headers) as resp:
            return resp.read(), {k.lower(): v for k, v in resp.headers.items()}

    def _get(self, path: str, params: Mapping[str, Any] | None = None) -> Any:
        return self.request("GET", path, params=params)

    def _post(self, path: str, body: Any = None, params: Mapping[str, Any] | None = None) -> Any:
        return self.request("POST", path, json_body=body if body is not None else {}, params=params)

    # -- health / repositories ---------------------------------------------

    def health(self) -> Health:
        return Health.from_dict(self._get("/health"))

    def list_repos(self) -> list[RepoSummary]:
        return RepoSummary.list_from(self._get("/repos"))

    def create_repo(self, name: str, default_branch: str | None = None) -> RepoSummary:
        body = _drop_none({"name": name, "default_branch": default_branch})
        return RepoSummary.from_dict(self._post("/repos", body))

    def repo(self, repo: str) -> RepoSummary:
        return RepoSummary.from_dict(self._get(f"/repos/{_seg(repo)}"))

    def delete_repo(self, repo: str) -> dict[str, Any]:
        return self.request("DELETE", f"/repos/{_seg(repo)}") or {}

    # -- refs ---------------------------------------------------------------

    def refs(self, repo: str, prefix: str | None = None) -> list[RefEntry]:
        return RefEntry.list_from(self._get(f"/repos/{_seg(repo)}/refs", {"prefix": prefix}))

    def update_refs(
        self,
        repo: str,
        updates: Sequence[RefUpdate | Mapping[str, Any]],
        message: str | None = None,
    ) -> RefUpdateResponse:
        """Atomic compare-and-swap of one or more refs.

        Each update is a :class:`RefUpdate` or a dict ``{"name", "expect", "new"}``;
        build ``expect`` with :func:`expect_is`, :func:`expect_absent` or
        :func:`expect_any`. Raises :class:`~agit.errors.RefConflict` (409) when
        a precondition fails and ``NotFound`` (412 ``object_not_found``) when a
        new value references an object the server does not have.
        """
        ups = [u.to_dict() if isinstance(u, RefUpdate) else dict(u) for u in updates]
        body: dict[str, Any] = {"updates": ups}
        if message is not None:
            body["message"] = message
        return RefUpdateResponse.from_dict(self._post(f"/repos/{_seg(repo)}/refs/update", body))

    def set_ref(self, repo: str, name: str, new: str | None, expect: Mapping[str, Any] | None = None,
                message: str | None = None) -> RefUpdateResponse:
        """Convenience: update a single ref (``expect`` defaults to ``expect_any()``)."""
        return self.update_refs(repo, [RefUpdate(name, dict(expect or expect_any()), new)], message)

    # -- objects ------------------------------------------------------------

    def missing(self, repo: str, ids: Sequence[str]) -> list[str]:
        r = self._post(f"/repos/{_seg(repo)}/objects/missing", {"ids": list(ids)})
        return list((r or {}).get("missing", []))

    def upload_objects(self, repo: str, stream: bytes) -> dict[str, Any]:
        """Upload an object stream (frames ``kind:u8 | len:u32be | payload``). Returns ``{stored, skipped}``."""
        return self.request(
            "POST",
            f"/repos/{_seg(repo)}/objects",
            data=bytes(stream),
            headers={"Content-Type": OBJECT_STREAM_CONTENT_TYPE},
        ) or {}

    def fetch_objects(self, repo: str, wants: Sequence[str], haves: Sequence[str] = ()) -> bytes:
        """Object stream of everything reachable from ``wants`` and not from ``haves``."""
        body, _ = self.request_bytes(
            "POST",
            f"/repos/{_seg(repo)}/fetch",
            json_body={"wants": list(wants), "haves": list(haves)},
            headers={"Accept": OBJECT_STREAM_CONTENT_TYPE},
        )
        return body

    def get_object(self, repo: str, object_id: str) -> tuple[str, bytes]:
        """Raw object payload. Returns ``(kind, payload)`` with kind from ``X-Agit-Kind``."""
        body, headers = self.request_bytes("GET", f"/repos/{_seg(repo)}/objects/{_seg(object_id)}",
                                           headers={"Accept": "*/*"})
        return headers.get("x-agit-kind", ""), body

    def object_json(self, repo: str, object_id: str) -> dict[str, Any]:
        return self._get(f"/repos/{_seg(repo)}/objects/{_seg(object_id)}/json") or {}

    # -- browsing -----------------------------------------------------------

    def commits(
        self,
        repo: str,
        rev: str | None = None,
        limit: int | None = None,
        agent: str | None = None,
        task: str | None = None,
        path: str | None = None,
        first_parent: bool = False,
    ) -> list[CommitEntry]:
        params = {"rev": rev, "limit": limit, "agent": agent, "task": task, "path": path,
                  "first_parent": "1" if first_parent else None}
        return CommitEntry.list_from(self._get(f"/repos/{_seg(repo)}/commits", params))

    def commit(self, repo: str, rev: str) -> CommitDetail:
        return CommitDetail.from_dict(self._get(f"/repos/{_seg(repo)}/commits/{_seg(rev)}"))

    def tree(self, repo: str, rev: str, path: str | None = None) -> dict[str, Any]:
        """Directory ``{kind:"tree", path, entries}`` or file ``{kind:"blob", path, id, size, binary, text?}``."""
        return self._get(f"/repos/{_seg(repo)}/tree/{_seg(rev)}", {"path": path}) or {}

    def diff(self, repo: str, from_: str | None, to: str) -> DiffResponse:
        return DiffResponse.from_dict(self._get(f"/repos/{_seg(repo)}/diff", {"from": from_, "to": to}))

    def branches(self, repo: str) -> list[BranchEntry]:
        return BranchEntry.list_from(self._get(f"/repos/{_seg(repo)}/branches"))

    def tags(self, repo: str) -> list[RefEntry]:
        """Repository tags (``GET /repos/{repo}/tags``). Knowledge tags: :meth:`knowledge_tags`."""
        return RefEntry.list_from(self._get(f"/repos/{_seg(repo)}/tags"))

    # -- events -------------------------------------------------------------

    def events(self, repo: str, since: int = 0, limit: int | None = None) -> list[Event]:
        return Event.list_from(self._get(f"/repos/{_seg(repo)}/events", {"since": since, "limit": limit}))

    def stream_events(self, repo: str, since: int | None = None, *, timeout: float | None = None) -> Iterator[Event]:
        """Follow ``GET /repos/{repo}/events/stream`` (Server-Sent Events).

        ``since=None`` yields only new events; ``since=N`` replays the backlog
        after sequence ``N`` first. The generator closes the connection when it
        is closed or garbage collected. ``timeout`` is the socket read timeout;
        the server sends a keep-alive comment every 15 s, so ``None`` (block
        forever) is safe.
        """
        resp = self._open(
            "GET",
            f"/repos/{_seg(repo)}/events/stream",
            params={"since": since},
            headers={"Accept": "text/event-stream", "Cache-Control": "no-cache"},
            timeout=timeout,
        )
        try:
            for payload in iter_sse(resp):
                try:
                    obj = json.loads(payload)
                except ValueError:
                    continue
                if isinstance(obj, dict):
                    yield Event.from_dict(obj)
        except (socket.timeout, TimeoutError) as e:
            raise TransportError("event stream timed out", code="transport", retryable=True) from e
        finally:
            try:
                resp.close()
            except Exception:  # pragma: no cover - best effort
                pass

    # -- leases -------------------------------------------------------------

    def leases(self, repo: str) -> list[Lease]:
        return Lease.list_from(self._get(f"/repos/{_seg(repo)}/leases"))

    def claim_lease(
        self,
        repo: str,
        holder: str | None,
        paths: Sequence[str] | str,
        mode: str = "exclusive",
        ttl_secs: int | None = None,
        note: str | None = None,
    ) -> Lease:
        """Claim a lease. ``holder=None`` uses the client's agent id. Raises :class:`LeaseConflict` (409)."""
        holder = holder or self.agent
        if not holder:
            raise ValueError("claim_lease needs a holder (or Client(agent=...) / $AGIT_AGENT)")
        body = _drop_none({"holder": holder, "paths": as_list(paths), "mode": mode, "ttl_secs": ttl_secs, "note": note})
        return Lease.from_dict(self._post(f"/repos/{_seg(repo)}/leases", body))

    def renew_lease(self, repo: str, lease_id: str, ttl_secs: int) -> Lease:
        return Lease.from_dict(self._post(f"/repos/{_seg(repo)}/leases/{_seg(lease_id)}/renew", {"ttl_secs": ttl_secs}))

    def release_lease(self, repo: str, lease_id: str) -> dict[str, Any]:
        return self.request("DELETE", f"/repos/{_seg(repo)}/leases/{_seg(lease_id)}") or {}

    def check_lease(self, repo: str, holder: str | None, paths: Sequence[str] | str) -> dict[str, Any]:
        """``{"allowed": true, ...}`` or raises :class:`LeaseConflict`."""
        holder = holder or self.agent
        if not holder:
            raise ValueError("check_lease needs a holder (or Client(agent=...) / $AGIT_AGENT)")
        return self._post(f"/repos/{_seg(repo)}/leases/check", {"holder": holder, "paths": as_list(paths)}) or {}

    # -- pull requests ------------------------------------------------------

    def pulls(self, repo: str, state: str | None = None) -> list[PullRequest]:
        return PullRequest.list_from(self._get(f"/repos/{_seg(repo)}/pulls", {"state": state}))

    def create_pull(
        self,
        repo: str,
        title: str,
        source: str,
        target: str | None = None,
        body: str | None = None,
        intent: str | None = None,
        task: str | None = None,
    ) -> PullRequest:
        payload = _drop_none({"title": title, "body": body, "intent": intent, "task": task,
                              "source": source, "target": target})
        return PullRequest.from_dict(self._post(f"/repos/{_seg(repo)}/pulls", payload))

    def pull(self, repo: str, number: int) -> PullDetail:
        return PullDetail.from_dict(self._get(f"/repos/{_seg(repo)}/pulls/{int(number)}"))

    def pull_diff(self, repo: str, number: int) -> DiffResponse:
        return DiffResponse.from_dict(self._get(f"/repos/{_seg(repo)}/pulls/{int(number)}/diff"))

    def update_pull(
        self,
        repo: str,
        number: int,
        title: str | None = None,
        body: str | None = None,
        intent: str | None = None,
        task: str | None = None,
    ) -> PullRequest:
        payload = _drop_none({"title": title, "body": body, "intent": intent, "task": task})
        return PullRequest.from_dict(self._post(f"/repos/{_seg(repo)}/pulls/{int(number)}/update", payload))

    def comment_pull(
        self,
        repo: str,
        number: int,
        body: str,
        path: str | None = None,
        line: int | None = None,
        kind: str = "note",
    ) -> Comment:
        payload = _drop_none({"path": path, "line": line, "kind": kind, "body": body})
        return Comment.from_dict(self._post(f"/repos/{_seg(repo)}/pulls/{int(number)}/comments", payload))

    def resolve_comment(self, repo: str, number: int, comment_id: int) -> PullRequest:
        return PullRequest.from_dict(
            self._post(f"/repos/{_seg(repo)}/pulls/{int(number)}/comments/{int(comment_id)}/resolve")
        )

    def review_pull(self, repo: str, number: int, decision: str, body: str = "") -> PullRequest:
        """``decision`` is ``approve``, ``request_changes`` or ``comment``."""
        return PullRequest.from_dict(
            self._post(f"/repos/{_seg(repo)}/pulls/{int(number)}/reviews", {"decision": decision, "body": body})
        )

    def set_check(self, repo: str, number: int, name: str, status: str, url: str | None = None) -> PullRequest:
        """``status`` is ``pending``, ``success`` or ``failure``."""
        payload = _drop_none({"name": name, "status": status, "url": url})
        return PullRequest.from_dict(self._post(f"/repos/{_seg(repo)}/pulls/{int(number)}/checks", payload))

    def merge_pull(self, repo: str, number: int, strategy: str = "merge", message: str | None = None) -> MergeResponse:
        """Merge on the server (``strategy`` ``merge`` or ``ff``). Raises :class:`MergeConflicts` (409)."""
        payload = _drop_none({"strategy": strategy, "message": message})
        return MergeResponse.from_dict(self._post(f"/repos/{_seg(repo)}/pulls/{int(number)}/merge", payload))

    def close_pull(self, repo: str, number: int) -> PullRequest:
        return PullRequest.from_dict(self._post(f"/repos/{_seg(repo)}/pulls/{int(number)}/close"))

    # -- policy -------------------------------------------------------------

    def policy(self, repo: str) -> Policy:
        """Repository policy (defaults when unset)."""
        return Policy.from_dict(self._get(f"/repos/{_seg(repo)}/policy"))

    def set_policy(self, repo: str, policy: Policy | Mapping[str, Any]) -> Policy:
        """``PUT`` the whole policy. Violations later surface as :class:`~agit.errors.PolicyViolation`."""
        body = policy.to_dict() if isinstance(policy, Policy) else dict(policy)
        return Policy.from_dict(self.request("PUT", f"/repos/{_seg(repo)}/policy", json_body=body))

    # -- webhooks -----------------------------------------------------------

    def webhooks(self, repo: str) -> list[Webhook]:
        return [Webhook.from_dict(w) for w in self._get(f"/repos/{_seg(repo)}/webhooks")]

    def create_webhook(
        self,
        repo: str,
        url: str,
        *,
        kinds: Sequence[str] = (),
        secret: str | None = None,
    ) -> Webhook:
        """Register a webhook. ``kinds`` are exact kinds or prefixes such as ``pull.*`` (empty = all)."""
        body: dict[str, Any] = {"url": url, "kinds": list(kinds)}
        if secret:
            body["secret"] = secret
        return Webhook.from_dict(self._post(f"/repos/{_seg(repo)}/webhooks", body))

    def delete_webhook(self, repo: str, webhook_id: str) -> dict[str, Any]:
        return self.request("DELETE", f"/repos/{_seg(repo)}/webhooks/{_seg(webhook_id)}")

    def test_webhook(self, repo: str, webhook_id: str) -> dict[str, Any]:
        """Queue a synthetic ``ping`` delivery."""
        return self._post(f"/repos/{_seg(repo)}/webhooks/{_seg(webhook_id)}/test")

    # -- knowledge ----------------------------------------------------------

    def notes(
        self,
        repo: str,
        tags: Sequence[str] | str | None = None,
        path: str | None = None,
        q: str | None = None,
        limit: int | None = None,
        include_superseded: bool = False,
    ) -> list[NoteSummary]:
        """Search notes. ``tags`` must all match; ``path`` matches notes whose ``paths`` cover it."""
        params = {
            "tags": as_list(tags) or None,
            "path": path,
            "q": q,
            "limit": limit,
            "include_superseded": "1" if include_superseded else None,
        }
        return NoteSummary.list_from(self._get(f"/repos/{_seg(repo)}/knowledge/notes", params))

    def note(self, repo: str, note_id: str) -> Note:
        return Note.from_dict(self._get(f"/repos/{_seg(repo)}/knowledge/notes/{_seg(note_id)}"))

    def create_note(
        self,
        repo: str,
        title: str,
        body: str = "",
        tags: Sequence[str] | str = (),
        paths: Sequence[str] | str = (),
        commits: Sequence[str] | str = (),
        pulls: Sequence[int] = (),
        confidence: float | None = None,
    ) -> Note:
        payload: dict[str, Any] = {
            "title": title,
            "body": body,
            "tags": as_list(tags),
            "paths": as_list(paths),
            "commits": as_list(commits),
            "pulls": [int(p) for p in pulls],
        }
        if confidence is not None:
            payload["confidence"] = confidence
        return Note.from_dict(self._post(f"/repos/{_seg(repo)}/knowledge/notes", payload))

    def update_note(
        self,
        repo: str,
        note_id: str,
        *,
        title: str | None = None,
        body: str | None = None,
        tags: Sequence[str] | None = None,
        paths: Sequence[str] | None = None,
        commits: Sequence[str] | None = None,
        pulls: Sequence[int] | None = None,
        confidence: float | None = None,
        superseded_by: str | None = _UNSET,
    ) -> Note:
        """``PATCH`` a note. Only the given fields change; ``superseded_by=None`` clears it."""
        payload = _drop_none({
            "title": title,
            "body": body,
            "tags": None if tags is None else as_list(tags),
            "paths": None if paths is None else as_list(paths),
            "commits": None if commits is None else as_list(commits),
            "pulls": None if pulls is None else [int(p) for p in pulls],
            "confidence": confidence,
        })
        if superseded_by is not _UNSET:
            payload["superseded_by"] = superseded_by
        return Note.from_dict(self.request("PATCH", f"/repos/{_seg(repo)}/knowledge/notes/{_seg(note_id)}", json_body=payload))

    def delete_note(self, repo: str, note_id: str) -> dict[str, Any]:
        return self.request("DELETE", f"/repos/{_seg(repo)}/knowledge/notes/{_seg(note_id)}") or {}

    def knowledge_tags(self, repo: str) -> list[TagCount]:
        """``GET /repos/{repo}/knowledge/tags`` -> ``[{tag, count}]``."""
        return TagCount.list_from(self._get(f"/repos/{_seg(repo)}/knowledge/tags"))

    note_tags = knowledge_tags

    def rules(self, repo: str) -> list[TagRule]:
        return TagRule.list_from(self._get(f"/repos/{_seg(repo)}/knowledge/rules"))

    def set_rules(self, repo: str, rules: Sequence[TagRule | Mapping[str, Any]]) -> list[TagRule]:
        body = [r.to_dict() if isinstance(r, TagRule) else {"pattern": r["pattern"], "tags": list(r.get("tags", []))}
                for r in rules]
        return TagRule.list_from(self.request("PUT", f"/repos/{_seg(repo)}/knowledge/rules", json_body=body))

    def context(self, repo: str, path: str, limit: int | None = None) -> ContextResponse:
        """Notes, recent commits, leases and open PRs relevant to ``path`` in one call."""
        return ContextResponse.from_dict(self._get(f"/repos/{_seg(repo)}/context", {"path": path, "limit": limit}))

    # -- activity -----------------------------------------------------------

    def agents(self, repo: str) -> list[AgentActivity]:
        return AgentActivity.list_from(self._get(f"/repos/{_seg(repo)}/agents"))


def iter_sse(resp: Any) -> Iterator[str]:
    """Yield the ``data`` payload of each Server-Sent Event read from ``resp``.

    Handles multi-line ``data:`` fields (joined with newlines), ignores ``id:``,
    ``event:``, ``retry:`` and comment lines, and dispatches on blank lines.
    """
    data_lines: list[str] = []
    while True:
        raw = resp.readline()
        if not raw:
            break
        line = raw.decode("utf-8", "replace").rstrip("\r\n")
        if line == "":
            if data_lines:
                yield "\n".join(data_lines)
                data_lines = []
            continue
        if line.startswith(":"):
            continue
        field_name, sep, value = line.partition(":")
        if not sep:
            field_name, value = line, ""
        if value.startswith(" "):
            value = value[1:]
        if field_name == "data":
            data_lines.append(value)
    if data_lines:
        yield "\n".join(data_lines)


__all__ = [
    "Client",
    "DEFAULT_BASE_URL",
    "OBJECT_STREAM_CONTENT_TYPE",
    "RefUpdate",
    "expect_absent",
    "expect_any",
    "expect_is",
    "iter_sse",
]
