"""High-level helper for the agent loop the README describes.

    context(path)  ->  claim(paths)  ->  edit / commit (CLI)  ->  open_pr(...)
                   ->  remember(what you learned)  ->  release leases on exit

    >>> from agit import Client, AgentSession
    >>> with AgentSession(Client(), "demo", "coder-1") as s:
    ...     ctx = s.context("src/auth/login.rs")
    ...     s.claim(["src/auth/**"], ttl_secs=1800, note="DEMO-7")
    ...     ...  # edit, `agit commit`, `agit push`
    ...     pr = s.open_pr("Add login", source="refs/heads/ws/coder-1", task="DEMO-7")
    ...     s.remember("login.rs validates email first", tags=["area:auth"], paths=["src/auth/**"])
    ... # leases are released here
"""

from __future__ import annotations

from typing import Any, Iterator, Sequence

from .client import Client
from .errors import AgitError, NotFound
from .types import ContextResponse, Event, Lease, Note, PullRequest

DEFAULT_TTL_SECS = 30 * 60


class AgentSession:
    """One agent working in one repository.

    Args:
        client: A :class:`~agit.Client`. If ``agent_id``/``model`` are given, a
            derived client sending those headers is used.
        repo: Repository name on the server.
        agent_id: Acting agent id; defaults to ``client.agent`` (``$AGIT_AGENT``).
        model: Optional model name (``X-Agit-Model``).
        default_ttl_secs: TTL used by :meth:`claim` when none is given.
    """

    def __init__(
        self,
        client: Client,
        repo: str,
        agent_id: str | None = None,
        *,
        model: str | None = None,
        default_ttl_secs: int = DEFAULT_TTL_SECS,
    ) -> None:
        self.client = client.with_agent(agent_id).with_model(model)
        self.repo = repo
        self.agent_id = self.client.agent
        if not self.agent_id:
            raise ValueError("AgentSession needs an agent id (argument, Client(agent=...) or $AGIT_AGENT)")
        self.default_ttl_secs = default_ttl_secs
        self.leases: list[Lease] = []
        self.pulls: list[PullRequest] = []
        self.notes: list[Note] = []

    def __repr__(self) -> str:  # pragma: no cover - cosmetic
        return f"AgentSession(repo={self.repo!r}, agent_id={self.agent_id!r}, leases={len(self.leases)})"

    # -- read before you write ---------------------------------------------

    def context(self, path: str, limit: int | None = None) -> ContextResponse:
        """What to read before touching ``path``: notes, recent commits, leases, open PRs."""
        return self.client.context(self.repo, path, limit=limit)

    def events(self, since: int = 0, limit: int | None = None) -> list[Event]:
        return self.client.events(self.repo, since=since, limit=limit)

    def follow(self, since: int | None = None) -> Iterator[Event]:
        return self.client.stream_events(self.repo, since=since)

    # -- coordination -------------------------------------------------------

    def claim(
        self,
        paths: Sequence[str] | str,
        ttl_secs: int | None = None,
        note: str | None = None,
        mode: str = "exclusive",
    ) -> Lease:
        """Claim a lease on ``paths`` for this agent and remember it for :meth:`release_all`."""
        lease = self.client.claim_lease(
            self.repo,
            self.agent_id,
            paths,
            mode=mode,
            ttl_secs=self.default_ttl_secs if ttl_secs is None else ttl_secs,
            note=note,
        )
        self.leases.append(lease)
        return lease

    def check(self, paths: Sequence[str] | str) -> bool:
        """``True`` when nobody else holds an exclusive lease on ``paths``."""
        try:
            return bool(self.client.check_lease(self.repo, self.agent_id, paths).get("allowed", True))
        except AgitError as e:
            if e.code == "lease_conflict":
                return False
            raise

    def renew_all(self, ttl_secs: int | None = None) -> list[Lease]:
        ttl = self.default_ttl_secs if ttl_secs is None else ttl_secs
        renewed = [self.client.renew_lease(self.repo, lease.id, ttl) for lease in self.leases]
        self.leases = renewed
        return renewed

    def release(self, lease: Lease | str) -> None:
        lease_id = lease.id if isinstance(lease, Lease) else str(lease)
        try:
            self.client.release_lease(self.repo, lease_id)
        except NotFound:
            pass  # already expired or released
        self.leases = [l for l in self.leases if l.id != lease_id]

    def release_all(self) -> list[str]:
        """Release every lease claimed through this session. Returns the released ids."""
        released: list[str] = []
        errors: list[AgitError] = []
        for lease in list(self.leases):
            try:
                self.client.release_lease(self.repo, lease.id)
                released.append(lease.id)
            except NotFound:
                released.append(lease.id)
            except AgitError as e:
                errors.append(e)
        self.leases = [l for l in self.leases if l.id not in released]
        if errors:
            raise errors[0]
        return released

    # -- deliverables -------------------------------------------------------

    def open_pr(
        self,
        title: str,
        source: str,
        target: str | None = None,
        body: str | None = None,
        intent: str | None = None,
        task: str | None = None,
    ) -> PullRequest:
        """Open a pull request from ``source`` (a full ref such as ``refs/heads/ws/coder-1``)."""
        pr = self.client.create_pull(self.repo, title, source, target=target, body=body, intent=intent, task=task)
        self.pulls.append(pr)
        return pr

    def remember(
        self,
        title: str,
        body: str = "",
        tags: Sequence[str] | str = (),
        paths: Sequence[str] | str = (),
        commits: Sequence[str] | str = (),
        pulls: Sequence[int] = (),
        confidence: float | None = None,
    ) -> Note:
        """Record what you learned as a knowledge note (``create_note``)."""
        note = self.client.create_note(
            self.repo, title, body=body, tags=tags, paths=paths, commits=commits, pulls=pulls, confidence=confidence
        )
        self.notes.append(note)
        return note

    # -- context manager ----------------------------------------------------

    def __enter__(self) -> "AgentSession":
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        try:
            self.release_all()
        except AgitError:
            if exc_type is None:
                raise
            # Do not mask the original exception with a release failure.


__all__ = ["AgentSession", "DEFAULT_TTL_SECS"]
