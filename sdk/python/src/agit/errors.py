"""Typed errors for the agit HTTP API and ``agit --json`` CLI.

Every failure the server or the CLI reports has the shape
``{"ok": false, "error": {"code", "message", "retryable", "details"?}}``.
:class:`AgitError` carries those fields; branch on :attr:`AgitError.code`,
never on the message.
"""

from __future__ import annotations

import json
from typing import Any, Mapping


class AgitError(Exception):
    """Base error. ``code`` is the stable machine-readable error code."""

    #: Error codes this class is selected for (used by subclasses).
    codes: tuple[str, ...] = ()

    def __init__(
        self,
        message: str,
        *,
        code: str = "error",
        retryable: bool = False,
        status: int | None = None,
        details: Mapping[str, Any] | None = None,
        exit_code: int | None = None,
    ) -> None:
        super().__init__(message)
        self.message = message
        self.code = code
        self.retryable = bool(retryable)
        self.status = status
        self.details: dict[str, Any] = dict(details) if isinstance(details, Mapping) else {}
        self.exit_code = exit_code

    def __str__(self) -> str:  # pragma: no cover - cosmetic
        where = f" (HTTP {self.status})" if self.status is not None else ""
        return f"[{self.code}]{where} {self.message}"

    def __repr__(self) -> str:  # pragma: no cover - cosmetic
        return (
            f"{type(self).__name__}(code={self.code!r}, message={self.message!r}, "
            f"retryable={self.retryable}, status={self.status})"
        )

    # -- construction -------------------------------------------------------

    @classmethod
    def from_envelope(
        cls,
        body: Mapping[str, Any],
        *,
        status: int | None = None,
        exit_code: int | None = None,
    ) -> "AgitError":
        """Build the most specific error for an ``{"ok": false, "error": {...}}`` body."""
        err = body.get("error") if isinstance(body, Mapping) else None
        if not isinstance(err, Mapping):
            err = {"code": "error", "message": str(body), "retryable": False}
        code = str(err.get("code") or "error")
        message = str(err.get("message") or code)
        retryable = bool(err.get("retryable", False))
        details = err.get("details")
        klass = error_class_for(code, status)
        return klass(
            message,
            code=code,
            retryable=retryable,
            status=status,
            details=details if isinstance(details, Mapping) else None,
            exit_code=exit_code,
        )

    @classmethod
    def from_http(cls, status: int, body: bytes | str, *, reason: str = "") -> "AgitError":
        """Build an error from an HTTP status and raw body (envelope or not)."""
        text = body.decode("utf-8", "replace") if isinstance(body, bytes) else body
        try:
            parsed = json.loads(text) if text.strip() else None
        except ValueError:
            parsed = None
        if isinstance(parsed, Mapping) and parsed.get("ok") is False:
            return cls.from_envelope(parsed, status=status)
        code = {404: "not_found", 403: "forbidden", 401: "forbidden", 400: "invalid"}.get(status, "http_error")
        message = text.strip() or reason or f"HTTP {status}"
        return error_class_for(code, status)(
            f"server error {status}: {message}",
            code=code,
            retryable=status in (408, 425, 429, 502, 503, 504),
            status=status,
        )


class RefConflict(AgitError):
    """Compare-and-swap on a ref failed: it moved. Re-read, reconcile, retry."""

    codes = ("ref_conflict",)

    @property
    def name(self) -> str | None:
        return self.details.get("name")

    @property
    def expected(self) -> str | None:
        return self.details.get("expected")

    @property
    def actual(self) -> str | None:
        return self.details.get("actual")


class LeaseConflict(AgitError):
    """A path is under someone else's exclusive lease. Not retryable: wait or negotiate."""

    codes = ("lease_conflict",)

    @property
    def path(self) -> str | None:
        return self.details.get("path")

    @property
    def holder(self) -> str | None:
        return self.details.get("holder")

    @property
    def lease_id(self) -> str | None:
        return self.details.get("lease_id")


class MergeConflicts(AgitError):
    """A merge stopped with conflicts."""

    codes = ("merge_conflicts",)

    @property
    def count(self) -> int:
        try:
            return int(self.details.get("count", 1))
        except (TypeError, ValueError):
            return 1


class NotFound(AgitError):
    """The repository, ref, object, lease, note or pull request does not exist."""

    codes = (
        "not_found",
        "ref_not_found",
        "object_not_found",
        "lease_not_found",
        "workspace_not_found",
        "unknown_revision",
        "not_a_repository",
        "path_not_tracked",
    )


class AlreadyExists(AgitError):
    """The repository or workspace already exists."""

    codes = ("already_exists", "workspace_exists")


class Forbidden(AgitError):
    """Missing or wrong bearer token."""

    codes = ("forbidden",)


class Invalid(AgitError):
    """Invalid argument or state (HTTP 400)."""

    codes = ("invalid", "invalid_ref_name", "invalid_object_id", "ambiguous_revision")


class TransportError(AgitError):
    """The request never produced an HTTP response (connection refused, timeout, ...)."""

    codes = ("transport",)


_BY_CODE: dict[str, type[AgitError]] = {}
for _klass in (RefConflict, LeaseConflict, MergeConflicts, NotFound, AlreadyExists, Forbidden, Invalid, TransportError):
    for _code in _klass.codes:
        _BY_CODE[_code] = _klass


def error_class_for(code: str, status: int | None = None) -> type[AgitError]:
    """Pick the exception class for an error code (falling back on the HTTP status)."""
    klass = _BY_CODE.get(code)
    if klass is not None:
        return klass
    if code.endswith("_not_found"):
        return NotFound
    if status == 404:
        return NotFound
    if status in (401, 403):
        return Forbidden
    return AgitError


__all__ = [
    "AgitError",
    "AlreadyExists",
    "Forbidden",
    "Invalid",
    "LeaseConflict",
    "MergeConflicts",
    "NotFound",
    "RefConflict",
    "TransportError",
    "error_class_for",
]
