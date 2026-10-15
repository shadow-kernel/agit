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
CHANGE_KINDS = ("added", "modified", "deleted", "type_changed")


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
    """One changed path. ``old``/``new`` are ``{"id", "mode"}`` sides (dicts)."""

    path: str = ""
    kind: str = ""
    old: dict[str, Any] | None = None
    new: dict[str, Any] | None = None
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)

    @property
    def letter(self) -> str:
        return {"added": "A", "modified": "M", "deleted": "D", "type_changed": "T"}.get(self.kind, "?")


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


@dataclass
class CommitDetail(Model):
    """``GET /repos/{repo}/commits/{rev}``: ``{id, commit, changes, patch}``."""

    id: str = ""
    commit: dict[str, Any] = field(default_factory=dict)
    changes: list[Change] = field(default_factory=list)
    patch: str = ""
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)
    _parsers = {"changes": _list_of(Change)}


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
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)


@dataclass
class NoteSummary(Model):
    """A note without its body, plus a 200-char ``excerpt``."""

    id: str = ""
    title: str = ""
    tags: list[str] = field(default_factory=list)
    paths: list[str] = field(default_factory=list)
    author: str = ""
    confidence: float = 1.0
    updated_ms: int = 0
    excerpt: str = ""
    superseded_by: str | None = None
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)


@dataclass
class TagCount(Model):
    tag: str = ""
    count: int = 0
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)


@dataclass
class TagRule(Model):
    pattern: str = ""
    tags: list[str] = field(default_factory=list)
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)

    def to_dict(self) -> dict[str, Any]:
        return {"pattern": self.pattern, "tags": list(self.tags)}


@dataclass
class ContextResponse(Model):
    """``GET /repos/{repo}/context?path=``: what to read before touching a path."""

    path: str = ""
    notes: list[NoteSummary] = field(default_factory=list)
    suggested_tags: list[str] = field(default_factory=list)
    recent_commits: list[CommitEntry] = field(default_factory=list)
    leases: list[Lease] = field(default_factory=list)
    open_pulls: list[PullRequest] = field(default_factory=list)
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)
    _parsers = {
        "notes": _list_of(NoteSummary),
        "recent_commits": _list_of(CommitEntry),
        "leases": _list_of(Lease),
        "open_pulls": _list_of(PullRequest),
    }


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
    "ContextResponse",
    "DiffResponse",
    "Event",
    "FileStat",
    "Health",
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
    "TagCount",
    "TagRule",
    "as_list",
    "expect_absent",
    "expect_any",
    "expect_is",
]
