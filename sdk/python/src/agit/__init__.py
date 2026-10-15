"""agit Python SDK: client for the agit server API and the ``agit --json`` CLI.

    from agit import Client, AgentSession, AgitError

    client = Client("http://127.0.0.1:7878/api/v1", agent="coder-1")
    with AgentSession(client, "demo") as session:
        ctx = session.context("src/auth/login.rs")
        session.claim(["src/auth/**"], ttl_secs=1800)
        ...
"""

from .agent import AgentSession
from .client import DEFAULT_BASE_URL, OBJECT_STREAM_CONTENT_TYPE, Client, __version__, iter_sse
from .errors import (
    AgitError,
    AlreadyExists,
    Forbidden,
    Invalid,
    LeaseConflict,
    MergeConflicts,
    NotFound,
    RefConflict,
    TransportError,
)
from .types import (
    Actor,
    AgentActivity,
    BranchEntry,
    Change,
    Check,
    Comment,
    CommitDetail,
    CommitEntry,
    Conflict,
    ContextResponse,
    DiffResponse,
    Event,
    FileStat,
    Health,
    Lease,
    MergeResponse,
    Note,
    NoteSummary,
    PullDetail,
    PullRequest,
    RefEntry,
    RefUpdate,
    RefUpdateResponse,
    RepoSummary,
    Review,
    TagCount,
    TagRule,
    expect_absent,
    expect_any,
    expect_is,
)

__all__ = [
    "__version__",
    "DEFAULT_BASE_URL",
    "OBJECT_STREAM_CONTENT_TYPE",
    "Client",
    "AgentSession",
    "iter_sse",
    # errors
    "AgitError",
    "AlreadyExists",
    "Forbidden",
    "Invalid",
    "LeaseConflict",
    "MergeConflicts",
    "NotFound",
    "RefConflict",
    "TransportError",
    # types
    "Actor",
    "AgentActivity",
    "BranchEntry",
    "Change",
    "Check",
    "Comment",
    "CommitDetail",
    "CommitEntry",
    "Conflict",
    "ContextResponse",
    "DiffResponse",
    "Event",
    "FileStat",
    "Health",
    "Lease",
    "MergeResponse",
    "Note",
    "NoteSummary",
    "PullDetail",
    "PullRequest",
    "RefEntry",
    "RefUpdate",
    "RefUpdateResponse",
    "RepoSummary",
    "Review",
    "TagCount",
    "TagRule",
    "expect_absent",
    "expect_any",
    "expect_is",
]
