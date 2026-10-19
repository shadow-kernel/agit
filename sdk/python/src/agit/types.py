"""Dataclasses for the JSON objects of the agit HTTP API (docs/API.md).

Field names match ``crates/agit-protocol/src/types.rs`` exactly. Parsing is
lenient: unknown fields are ignored and missing fields get a neutral default,
so additions on the server never break an older SDK. The complete payload is
kept on ``.raw`` for anything the dataclass does not model.
"""

from __future__ import annotations

import dataclasses
from dataclasses import dataclass, field
from typing import Any, Callable, ClassVar, Mapping, Sequence, TypeVar

T = TypeVar("T", bound="Model")

# -- enums as plain string constants (the wire format is lowercase strings) ---

PULL_STATES = ("open", "merged", "closed")
REVIEW_DECISIONS = ("approve", "request_changes", "comment")
COMMENT_KINDS = ("question", "suggestion", "blocker", "note")
CHECK_STATUSES = ("pending", "success", "failure")
MERGE_STRATEGIES = ("merge", "ff")
LEASE_MODES = ("exclusive", "shared")
CHANGE_KINDS = ("added", "modified", "deleted", "type_changed", "renamed")


class Model:
    """Mixin: lenient ``from_dict`` for dataclasses."""

    #: Per-field converters for nested objects. Values are callables taking the
    #: raw JSON value and returning the parsed value.
    _parsers: ClassVar[dict[str, Callable[[Any], Any]]] = {}

    @classmethod
    def from_dict(cls: type[T], data: Mapping[str, Any] | None) -> T:
        if data is None:
            data = {}
        if not isinstance(data, Mapping):
            raise TypeError(f"{cls.__name__}.from_dict expects a mapping, got {type(data).__name__}")
        kwargs: dict[str, Any] = {}
        for f in dataclasses.fields(cls):  # type: ignore[arg-type]
            if f.name == "raw":
                continue
            if f.name in data:
                value = data[f.name]
                parser = cls._parsers.get(f.name)
                if parser is not None and value is not None:
                    value = parser(value)
                kwargs[f.name] = value
        kwargs["raw"] = dict(data)
        return cls(**kwargs)  # type: ignore[call-arg]

    @classmethod
    def list_from(cls: type[T], data: Any) -> list[T]:
        if not data:
            return []
        return [cls.from_dict(item) for item in data]

    def to_dict(self) -> dict[str, Any]:
        """The payload as received from the server (not re-serialised)."""
        return dict(getattr(self, "raw", {}))


def _list_of(klass: type[T]) -> Callable[[Any], list[T]]:
    return klass.list_from


def _one(klass: type[T]) -> Callable[[Any], T]:
    return klass.from_dict


# -- health / repos ----------------------------------------------------------


@dataclass
class Health(Model):
    status: str = ""
    version: str = ""
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)


@dataclass
class RepoSummary(Model):
    name: str = ""
    default_branch: str = "main"
    head: str | None = None
    created_ms: int = 0
    branches: int = 0
    tags: int = 0
    objects: int = 0
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)


# -- refs --------------------------------------------------------------------


@dataclass
class RefEntry(Model):
    name: str = ""
    id: str = ""
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)


@dataclass
class BranchEntry(Model):
    name: str = ""
    id: str = ""
    ahead: int | None = None
    behind: int | None = None
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)


def expect_is(object_id: str) -> dict[str, Any]:
    """CAS precondition: the ref must currently point at ``object_id``."""
    return {"is": object_id}


def expect_absent() -> dict[str, Any]:
    """CAS precondition: the ref must not exist (create)."""
    return {"absent": True}


def expect_any() -> dict[str, Any]:
    """No precondition (unconditional update; use sparingly)."""
    return {"any": True}


@dataclass
class RefUpdate:
    """One entry of ``POST /repos/{repo}/refs/update``. ``new=None`` deletes."""

    name: str
    expect: dict[str, Any]
    new: str | None = None

    def to_dict(self) -> dict[str, Any]:
        return {"name": self.name, "expect": dict(self.expect), "new": self.new}


@dataclass
class RefUpdateResponse(Model):
    updated: list[RefEntry] = field(default_factory=list)
    deleted: list[str] = field(default_factory=list)
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)
    _parsers = {"updated": _list_of(RefEntry)}


# -- commits / diffs ---------------------------------------------------------


@dataclass
class Change(Model):
    """One changed path. ``old``/``new`` are ``{"id", "mode"}`` sides (dicts).

    ``kind: "renamed"`` (with ``from`` and ``similarity`` on ``.raw``) only appears when
    rename detection is applied; see :attr:`renamed_from`.
    """

    path: str = ""
    kind: str = ""
    old: dict[str, Any] | None = None
    new: dict[str, Any] | None = None
    similarity: int | None = None
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)

    @property
    def letter(self) -> str:
        return {"added": "A", "modified": "M", "deleted": "D", "type_changed": "T", "renamed": "R"}.get(self.kind, "?")

    @property
    def renamed_from(self) -> str | None:
        """Previous path for ``kind == "renamed"`` (the wire field ``from`` is a Python keyword)."""
        return self.raw.get("from")


@dataclass
class CommitEntry(Model):
    """``{id, ...commit}`` as returned by ``GET /repos/{repo}/commits``."""

    id: str = ""
    tree: str = ""
    parents: list[str] = field(default_factory=list)
    author: dict[str, Any] = field(default_factory=dict)
    time_ms: int = 0
    message: str = ""
    intent: str | None = None
    task: str | None = None
    agent: dict[str, Any] | None = None
    checkpoint: bool = False
    meta: dict[str, Any] = field(default_factory=dict)
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)

    @property
    def agent_id(self) -> str | None:
        return (self.agent or {}).get("id")


SIGNATURE_STATUSES = ("unsigned", "valid", "invalid")


@dataclass
class SignatureInfo(Model):
    """Commit signature verdict (``CommitDetail.signature``).

    ``status`` is ``unsigned``, ``valid`` or ``invalid``. ``key_id``, ``signer`` and
    ``public_key`` are set for ``valid``; ``reason`` for ``invalid``. ``trusted`` is
    ``True`` when the key is registered for the signer in the repository (``/keys``).
    """

    status: str = "unsigned"
    key_id: str | None = None
    signer: str | None = None
    public_key: str | None = None
    reason: str | None = None
    trusted: bool = False
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)

    @property
    def is_valid(self) -> bool:
        return self.status == "valid"

    @property
    def is_trusted(self) -> bool:
        """Valid signature with a key registered for the signer."""
        return self.is_valid and self.trusted


@dataclass
class KeyEntry(Model):
    """A public signing key registered for an agent (``GET /repos/{repo}/keys``)."""

    agent: str = ""
    key_id: str = ""
    public_key: str = ""
    added_ms: int = 0
    added_by: str = ""
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)


@dataclass
class CommitDetail(Model):
    """``GET /repos/{repo}/commits/{rev}``: ``{id, commit, changes, patch, signature?, notes}``."""

    id: str = ""
    commit: dict[str, Any] = field(default_factory=dict)
    changes: list[Change] = field(default_factory=list)
    patch: str = ""
    #: Signature verdict; ``None`` when the server did not report one.
    signature: SignatureInfo | None = None
    #: Up to 10 notes linking this commit (drafts included) or covering a changed path.
    notes: list[NoteSummary] = field(default_factory=list)
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)
    _parsers = {
        "changes": _list_of(Change),
        "signature": _one(SignatureInfo),
        "notes": lambda v: NoteSummary.list_from(v),
    }


@dataclass
class FileStat(Model):
    path: str = ""
    added: int = 0
    removed: int = 0
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)


@dataclass
class DiffResponse(Model):
    changes: list[Change] = field(default_factory=list)
    patch: str = ""
    stats: list[FileStat] = field(default_factory=list)
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)
    _parsers = {"changes": _list_of(Change), "stats": _list_of(FileStat)}


# -- leases ------------------------------------------------------------------


@dataclass
class Lease(Model):
    id: str = ""
    holder: str = ""
    paths: list[str] = field(default_factory=list)
    mode: str = "exclusive"
    created_ms: int = 0
    expires_ms: int | None = None
    note: str | None = None
    workspace: str | None = None
    #: CLI only: the remote the lease is registered on (shared with the team); None = this machine only.
    remote: str | None = None
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)

    def is_expired_at(self, now_ms: int) -> bool:
        return self.expires_ms is not None and self.expires_ms <= now_ms


# -- events ------------------------------------------------------------------


@dataclass
class Event(Model):
    seq: int = 0
    time_ms: int = 0
    kind: str = ""
    workspace: str | None = None
    actor: str | None = None
    data: Any = None
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)


# -- pull requests -----------------------------------------------------------


@dataclass
class Actor(Model):
    id: str = ""
    model: str | None = None
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)


@dataclass
class Review(Model):
    reviewer: str = ""
    decision: str = "comment"
    body: str = ""
    time_ms: int = 0
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)


@dataclass
class Comment(Model):
    id: int = 0
    author: str = ""
    path: str | None = None
    line: int | None = None
    kind: str = "note"
    body: str = ""
    resolved: bool = False
    time_ms: int = 0
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)


@dataclass
class Check(Model):
    name: str = ""
    status: str = "pending"
    url: str | None = None
    time_ms: int = 0
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)


@dataclass
class PullRequest(Model):
    number: int = 0
    title: str = ""
    body: str = ""
    intent: str | None = None
    task: str | None = None
    source: str = ""
    target: str = ""
    source_id: str = ""
    target_id: str = ""
    base_id: str | None = None
    author: Actor = field(default_factory=Actor)
    state: str = "open"
    created_ms: int = 0
    updated_ms: int = 0
    merged_commit: str | None = None
    reviews: list[Review] = field(default_factory=list)
    comments: list[Comment] = field(default_factory=list)
    checks: list[Check] = field(default_factory=list)
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)
    _parsers = {
        "author": _one(Actor),
        "reviews": _list_of(Review),
        "comments": _list_of(Comment),
        "checks": _list_of(Check),
    }

    @property
    def is_open(self) -> bool:
        return self.state == "open"


@dataclass
class Conflict(Model):
    path: str = ""
    kind: str = ""
    base: str | None = None
    ours: str | None = None
    theirs: str | None = None
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)


@dataclass
class PullDetail(Model):
    """``GET /repos/{repo}/pulls/{n}``."""

    pr: PullRequest = field(default_factory=PullRequest)
    commits: list[CommitEntry] = field(default_factory=list)
    changes: list[Change] = field(default_factory=list)
    mergeable: bool = False
    conflicts: list[Conflict] = field(default_factory=list)
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)
    _parsers = {
        "pr": _one(PullRequest),
        "commits": _list_of(CommitEntry),
        "changes": _list_of(Change),
        "conflicts": _list_of(Conflict),
    }


@dataclass
class MergeResponse(Model):
    pr: PullRequest = field(default_factory=PullRequest)
    merged_commit: str = ""
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)
    _parsers = {"pr": _one(PullRequest)}


# -- knowledge ---------------------------------------------------------------


@dataclass
class Note(Model):
    id: str = ""
    title: str = ""
    body: str = ""
    tags: list[str] = field(default_factory=list)
    paths: list[str] = field(default_factory=list)
    commits: list[str] = field(default_factory=list)
    pulls: list[int] = field(default_factory=list)
    author: str = ""
    confidence: float = 1.0
    created_ms: int = 0
    updated_ms: int = 0
    superseded_by: str | None = None
    #: Commit the note was written or last validated against (``None`` when unknown).
    valid_at: str | None = None
    #: Machine-generated note (e.g. from a merged PR) that has not been accepted yet.
    draft: bool = False
    #: Computed on read: a path covered by ``paths`` changed between ``valid_at`` and HEAD.
    stale: bool = False
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)


@dataclass
class NoteSummary(Model):
    """A note without its body, plus a 200-char ``excerpt``, ``valid_at``, ``draft``, ``stale``
    and, for ``q=`` searches, the BM25 ``score``."""

    id: str = ""
    title: str = ""
    tags: list[str] = field(default_factory=list)
    paths: list[str] = field(default_factory=list)
    author: str = ""
    confidence: float = 1.0
    updated_ms: int = 0
    excerpt: str = ""
    superseded_by: str | None = None
    valid_at: str | None = None
    draft: bool = False
    stale: bool = False
    #: BM25 relevance; only present for full-text (``q=``) queries.
    score: float | None = None
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)


@dataclass
class StaleNote(NoteSummary):
    """``GET /repos/{repo}/knowledge/stale``: a summary (``stale=True``) plus the covered paths
    that changed between ``valid_at`` and the requested revision."""

    changed_paths: list[str] = field(default_factory=list)


@dataclass
class TagCount(Model):
    tag: str = ""
    count: int = 0
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)


@dataclass
class Webhook(Model):
    """Event delivery target. Payloads are signed with ``X-Agit-Signature: blake3=<hex>`` when a secret is set."""

    id: str = ""
    url: str = ""
    kinds: list[str] = field(default_factory=list)
    active: bool = True
    has_secret: bool = False
    created_ms: int = 0
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)


@dataclass
class TagRule(Model):
    pattern: str = ""
    tags: list[str] = field(default_factory=list)
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)

    def to_dict(self) -> dict[str, Any]:
        return {"pattern": self.pattern, "tags": list(self.tags)}


@dataclass
class ContextFile(Model):
    """One file of a context pack (``include=files``). ``truncated`` marks a cut made to fit the budget."""

    path: str = ""
    text: str = ""
    truncated: bool = False
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)


@dataclass
class ContextResponse(Model):
    """``GET /repos/{repo}/context?path=``: what to read before touching a path.

    ``notes`` lists fresh notes first, then stale ones (``stale=True``); drafts are
    excluded. ``agents_active`` are agents holding a lease on the path or who committed
    to it in the last 24 hours. ``files`` is only filled with ``include=files``
    (:meth:`~agit.Client.context_pack`).
    """

    path: str = ""
    notes: list[NoteSummary] = field(default_factory=list)
    suggested_tags: list[str] = field(default_factory=list)
    recent_commits: list[CommitEntry] = field(default_factory=list)
    leases: list[Lease] = field(default_factory=list)
    open_pulls: list[PullRequest] = field(default_factory=list)
    agents_active: list[str] = field(default_factory=list)
    files: list[ContextFile] = field(default_factory=list)
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)
    _parsers = {
        "notes": _list_of(NoteSummary),
        "recent_commits": _list_of(CommitEntry),
        "leases": _list_of(Lease),
        "open_pulls": _list_of(PullRequest),
        "files": _list_of(ContextFile),
    }

    @property
    def fresh_notes(self) -> list[NoteSummary]:
        return [n for n in self.notes if not n.stale]

    @property
    def stale_notes(self) -> list[NoteSummary]:
        return [n for n in self.notes if n.stale]


# -- policy ------------------------------------------------------------------


@dataclass
class Policy(Model):
    """Per-repository rules enforced at ``refs/update`` and ``pulls/{n}/merge`` (403 ``policy_violation``)."""

    protected_branches: list[str] = field(default_factory=list)
    require_pull_request: bool = False
    require_approvals: int = 0
    require_intent: bool = False
    require_agent: bool = False
    require_checks: bool = False
    block_on_blockers: bool = True
    allowed_merge_agents: list[str] = field(default_factory=list)
    allowed_push_agents: list[str] = field(default_factory=list)
    max_files_per_commit: int = 0
    #: Commits reaching a protected branch need a valid signature with a key registered for the signer.
    require_signed: bool = False
    #: Pushes may not bring another agent's commits into paths under an active exclusive lease.
    enforce_leases: bool = True
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)

    def to_dict(self) -> dict[str, Any]:
        """The full policy body for ``PUT /repos/{repo}/policy``."""
        return {
            "protected_branches": list(self.protected_branches),
            "require_pull_request": self.require_pull_request,
            "require_approvals": self.require_approvals,
            "require_intent": self.require_intent,
            "require_agent": self.require_agent,
            "require_checks": self.require_checks,
            "block_on_blockers": self.block_on_blockers,
            "allowed_merge_agents": list(self.allowed_merge_agents),
            "allowed_push_agents": list(self.allowed_push_agents),
            "max_files_per_commit": self.max_files_per_commit,
            "require_signed": self.require_signed,
            "enforce_leases": self.enforce_leases,
        }


# -- activity ----------------------------------------------------------------


@dataclass
class AgentActivity(Model):
    id: str = ""
    model: str | None = None
    commits: int = 0
    last_seen_ms: int = 0
    open_pulls: int = 0
    leases: int = 0
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)


def as_list(items: Sequence[str] | str | None) -> list[str]:
    """Accept a single string or an iterable of strings."""
    if items is None:
        return []
    if isinstance(items, str):
        return [items]
    return [str(i) for i in items]


__all__ = [
    "Actor",
    "AgentActivity",
    "BranchEntry",
    "CHANGE_KINDS",
    "CHECK_STATUSES",
    "COMMENT_KINDS",
    "Change",
    "Check",
    "CommitDetail",
    "CommitEntry",
    "Comment",
    "Conflict",
    "ContextFile",
    "ContextResponse",
    "DiffResponse",
    "Event",
    "FileStat",
    "Health",
    "KeyEntry",
    "LEASE_MODES",
    "Lease",
    "MERGE_STRATEGIES",
    "MergeResponse",
    "Model",
    "Note",
    "NoteSummary",
    "PULL_STATES",
    "Policy",
    "PullDetail",
    "PullRequest",
    "REVIEW_DECISIONS",
    "RefEntry",
    "RefUpdate",
    "RefUpdateResponse",
    "RepoSummary",
    "Review",
    "SIGNATURE_STATUSES",
    "SignatureInfo",
    "StaleNote",
    "TagCount",
    "TagRule",
    "as_list",
    "expect_absent",
    "expect_any",
    "expect_is",
]


# -- auth --------------------------------------------------------------------


@dataclass
class Scope(Model):
    """``{repo, access}``: ``repo`` is a repository name or ``*``; ``access`` is ``read``, ``write`` or ``admin``."""

    repo: str = "*"
    access: str = "read"
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)

    def to_dict(self) -> dict[str, Any]:
        return {"repo": self.repo, "access": self.access}


@dataclass
class TokenInfo(Model):
    """A scoped bearer token (``GET /auth/tokens``); hashes only, never the plaintext."""

    id: str = ""
    name: str = ""
    agent: str | None = None
    scopes: list[Scope] = field(default_factory=list)
    created_ms: int = 0
    expires_ms: int | None = None
    created_by: str = ""
    last_used_ms: int | None = None
    #: Session tokens: the authorized key (``/auth/keys`` id, or ``admin``) that signed in.
    key: str | None = None
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)

    _parsers: ClassVar[dict[str, Callable[[Any], Any]]] = {"scopes": _list_of(Scope)}


@dataclass
class TokenCreated(TokenInfo):
    """``POST /auth/tokens`` / ``POST /auth/session``: the plaintext ``token`` is returned once."""

    token: str = ""


@dataclass
class WhoAmI(Model):
    """``GET /auth/whoami``."""

    #: ``root``, ``anonymous`` or the token id.
    principal: str = ""
    agent: str | None = None
    root: bool = False
    scopes: list[Scope] = field(default_factory=list)
    #: The authorized key behind this session, when signed in with one.
    key: str | None = None
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)

    _parsers: ClassVar[dict[str, Callable[[Any], Any]]] = {"scopes": _list_of(Scope)}

    def holds(self, repo: str, access: str) -> bool:
        """Does this principal have ``access`` (``read``/``write``/``admin``) on ``repo`` (``*`` for server-wide)?"""
        if self.root:
            return True
        rank = {"read": 0, "write": 1, "admin": 2}
        need = rank.get(access, 0)
        return any((s.repo == "*" or s.repo == repo) and rank.get(s.access, 0) >= need for s in self.scopes)


@dataclass
class AuthorizedKey(Model):
    """An SSH public key allowed to sign in (``GET /auth/keys``)."""

    id: str = ""
    name: str = ""
    #: ``SHA256:…``, as ``ssh-keygen -lf`` prints it.
    fingerprint: str = ""
    key_id: str = ""
    #: ``ssh-ed25519 <base64> [comment]``.
    public_key: str = ""
    #: Sessions opened with this key always act as this agent.
    agent: str | None = None
    scopes: list[Scope] = field(default_factory=list)
    created_ms: int = 0
    created_by: str = ""
    last_used_ms: int | None = None
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)

    _parsers: ClassVar[dict[str, Callable[[Any], Any]]] = {"scopes": _list_of(Scope)}


@dataclass
class Challenge(Model):
    """``POST /auth/challenge``: a single-use nonce to sign with an SSH key."""

    nonce: str = ""
    expires_ms: int = 0
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)


#: Prefix of the bytes a client signs to redeem a challenge: ``agit-auth-v1\n<nonce>``.
AUTH_MESSAGE_PREFIX = b"agit-auth-v1\n"


def auth_message(nonce: str) -> bytes:
    """The exact bytes to sign for ``POST /auth/session``."""
    return AUTH_MESSAGE_PREFIX + nonce.encode("utf-8")
