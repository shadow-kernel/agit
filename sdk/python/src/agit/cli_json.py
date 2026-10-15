"""Run the local ``agit`` binary with ``--json`` and unwrap the envelope.

    >>> from agit.cli_json import run
    >>> run(["status"], cwd="/path/to/repo")
    {...}

The binary is ``agit`` on ``$PATH`` unless ``$AGIT_BIN`` points elsewhere.
Failures (``{"ok": false, "error": {...}}``) raise :class:`~agit.errors.AgitError`
with the CLI's exit code on ``.exit_code``.
"""

from __future__ import annotations

import json
import os
import subprocess
from typing import Any, Iterator, Mapping, Sequence

from .errors import AgitError

DEFAULT_BIN = "agit"


def binary() -> str:
    """Path of the ``agit`` binary (``$AGIT_BIN`` or ``agit``)."""
    return os.environ.get("AGIT_BIN") or DEFAULT_BIN


def _command(args: Sequence[str]) -> list[str]:
    cmd = [binary(), *[str(a) for a in args]]
    if "--json" not in cmd:
        cmd.append("--json")
    return cmd


def _env(env: Mapping[str, str] | None, agent: str | None, model: str | None) -> dict[str, str]:
    merged = dict(os.environ)
    if env:
        merged.update(env)
    if agent:
        merged["AGIT_AGENT"] = agent
    if model:
        merged["AGIT_MODEL"] = model
    merged["AGIT_JSON"] = "1"
    return merged


def _unwrap(text: str, *, exit_code: int, stderr: str = "") -> Any:
    try:
        body = json.loads(text)
    except ValueError:
        msg = stderr.strip() or text.strip() or f"agit exited with {exit_code} and no JSON output"
        raise AgitError(msg, code="cli", retryable=exit_code == 75, exit_code=exit_code) from None
    if not isinstance(body, dict):
        return body
    if body.get("ok") is False:
        raise AgitError.from_envelope(body, exit_code=exit_code)
    if "result" in body:
        return body["result"]
    return body


def run(
    args: Sequence[str],
    cwd: str | os.PathLike[str] | None = None,
    *,
    env: Mapping[str, str] | None = None,
    agent: str | None = None,
    model: str | None = None,
    timeout: float | None = None,
    input: str | bytes | None = None,
) -> Any:
    """Run ``agit <args> --json`` and return the ``result`` field.

    Raises :class:`AgitError` when the CLI reports ``{"ok": false}`` (the
    error's ``code``, ``message``, ``retryable`` and ``details`` are kept) or
    when the binary cannot be started.
    """
    cmd = _command(args)
    try:
        proc = subprocess.run(
            cmd,
            cwd=cwd,
            env=_env(env, agent, model),
            capture_output=True,
            text=isinstance(input, str) or input is None,
            input=input,
            timeout=timeout,
            check=False,
        )
    except FileNotFoundError:
        raise AgitError(
            f"agit binary not found: {cmd[0]!r} (set $AGIT_BIN or add agit to PATH)", code="cli_not_found"
        ) from None
    except subprocess.TimeoutExpired as e:
        raise AgitError(f"agit timed out after {timeout}s: {' '.join(cmd)}", code="cli_timeout", retryable=True) from e
    stdout = proc.stdout if isinstance(proc.stdout, str) else proc.stdout.decode("utf-8", "replace")
    stderr = proc.stderr if isinstance(proc.stderr, str) else proc.stderr.decode("utf-8", "replace")
    return _unwrap(stdout, exit_code=proc.returncode, stderr=stderr)


def stream(
    args: Sequence[str],
    cwd: str | os.PathLike[str] | None = None,
    *,
    env: Mapping[str, str] | None = None,
    agent: str | None = None,
    model: str | None = None,
) -> Iterator[Any]:
    """Run a streaming command (``events --follow``) and yield one object per JSON line."""
    cmd = _command(args)
    try:
        proc = subprocess.Popen(cmd, cwd=cwd, env=_env(env, agent, model), stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, text=True)
    except FileNotFoundError:
        raise AgitError(f"agit binary not found: {cmd[0]!r}", code="cli_not_found") from None
    assert proc.stdout is not None
    try:
        for line in proc.stdout:
            line = line.strip()
            if not line:
                continue
            yield _unwrap(line, exit_code=0)
        code = proc.wait()
        if code != 0:
            err = proc.stderr.read() if proc.stderr else ""
            raise AgitError(err.strip() or f"agit exited with {code}", code="cli", exit_code=code)
    finally:
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:  # pragma: no cover
                proc.kill()


__all__ = ["binary", "run", "stream", "DEFAULT_BIN"]
