from __future__ import annotations

import json
import os
import stat
import sys

import pytest

from agit import AgitError, RefConflict
from agit.cli_json import binary, run, stream

FAKE = r'''#!/bin/sh
# fake agit: echo args back inside the JSON envelope (--json may come first, as a global flag)
ALL="$*"
[ "$1" = "--json" ] && shift
case "$1" in
  status) printf '{"ok":true,"result":{"args":"%s","cwd":"%s","agent":"%s","json":"%s"}}\n' "$ALL" "$(pwd)" "$AGIT_AGENT" "$AGIT_JSON" ;;
  push)   printf '{"ok":false,"error":{"code":"ref_conflict","message":"ref moved","retryable":true,"details":{"name":"refs/heads/main"}}}\n'; exit 75 ;;
  crash)  echo "panic: boom" >&2; exit 1 ;;
  events) printf '{"ok":true,"result":{"seq":1}}\n{"ok":true,"result":{"seq":2}}\n' ;;
  *)      printf '{"ok":false,"error":{"code":"invalid","message":"unknown command","retryable":false}}\n'; exit 1 ;;
esac
'''


@pytest.fixture
def fake_bin(tmp_path, monkeypatch):
    path = tmp_path / "agit"
    path.write_text(FAKE)
    path.chmod(path.stat().st_mode | stat.S_IEXEC)
    monkeypatch.setenv("AGIT_BIN", str(path))
    return path


@pytest.mark.skipif(sys.platform == "win32", reason="shell script fake")
def test_run_ok(fake_bin, tmp_path):
    assert binary() == str(fake_bin)
    out = run(["status", "--short"], cwd=tmp_path, agent="coder-1")
    assert out["args"] == "status --short --json"  # --json appended once
    assert os.path.realpath(out["cwd"]) == os.path.realpath(str(tmp_path))
    assert out["agent"] == "coder-1" and out["json"] == "1"
    out = run(["--json", "status"])
    assert out["args"] == "--json status"


@pytest.mark.skipif(sys.platform == "win32", reason="shell script fake")
def test_run_error_envelope(fake_bin):
    with pytest.raises(RefConflict) as ei:
        run(["push"])
    e = ei.value
    assert e.code == "ref_conflict" and e.retryable is True and e.exit_code == 75 and e.status is None
    assert e.name == "refs/heads/main"
    with pytest.raises(AgitError) as ei:
        run(["nope"])
    assert ei.value.code == "invalid" and ei.value.exit_code == 1


@pytest.mark.skipif(sys.platform == "win32", reason="shell script fake")
def test_run_non_json_output(fake_bin):
    with pytest.raises(AgitError) as ei:
        run(["crash"])
    assert ei.value.code == "cli" and "panic: boom" in ei.value.message and ei.value.exit_code == 1


def test_missing_binary(monkeypatch, tmp_path):
    monkeypatch.setenv("AGIT_BIN", str(tmp_path / "does-not-exist"))
    with pytest.raises(AgitError) as ei:
        run(["status"])
    assert ei.value.code == "cli_not_found"


@pytest.mark.skipif(sys.platform == "win32", reason="shell script fake")
def test_stream(fake_bin):
    assert [o["seq"] for o in stream(["events", "--follow"])] == [1, 2]


def test_unwrap_shapes():
    from agit.cli_json import _unwrap

    assert _unwrap(json.dumps({"ok": True, "result": [1]}), exit_code=0) == [1]
    assert _unwrap(json.dumps({"plain": 1}), exit_code=0) == {"plain": 1}
