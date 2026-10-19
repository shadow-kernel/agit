#!/usr/bin/env python3
"""Cost of the agit Claude Code hooks per tool call.

Builds a throwaway repository with a knowledge note, starts an agit-server on
127.0.0.1, installs the hooks with install.sh and feeds each hook the JSON
Claude Code would send, many times. Reports the median and p95 wall time per
hook and what it returned. Nothing outside the temporary directory is touched.

    python3 integrations/claude-code/bench_hooks.py
    AGIT_BIN=/path/to/agit python3 integrations/claude-code/bench_hooks.py --runs 50
"""

import argparse
import json
import os
import shutil
import socket
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--runs", type=int, default=30)
    ap.add_argument("--out", default=None)
    args = ap.parse_args()

    agit = os.environ.get("AGIT_BIN") or shutil.which("agit")
    server = shutil.which("agit-server", path=str(Path(agit).parent)) or shutil.which("agit-server")
    if not agit or not server:
        sys.exit("agit and agit-server must be on PATH (or set AGIT_BIN)")
    work = Path(tempfile.mkdtemp(prefix="agit-hooks-bench-"))
    env = dict(os.environ)
    env.update({
        "HOME": str(work / "home"),
        "TMPDIR": str(work / "tmp"),
        "AGIT_BIN": agit,
        "AGIT_AGENT": "coder-1",
        "AGIT_AUTO_LOGIN": "0",
        "AGIT_AUTHOR_NAME": "bench",
        "AGIT_AUTHOR_EMAIL": "bench@example.com",
        "PATH": str(Path(agit).parent) + os.pathsep + env["PATH"],
    })
    env.pop("AGIT_TOKEN", None)
    (work / "home").mkdir()
    (work / "tmp").mkdir()

    def sh(cmd, cwd, extra=None, check=True):
        e = dict(env)
        if extra:
            e.update(extra)
        p = subprocess.run(cmd, cwd=str(cwd), env=e, capture_output=True, text=True)
        if check and p.returncode != 0:
            raise RuntimeError("%s failed: %s %s" % (cmd, p.stdout, p.stderr))
        return p

    port = free_port()
    srv = subprocess.Popen([server, "--data-dir", str(work / "srv"), "--listen", "127.0.0.1:%d" % port], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    results = {}
    try:
        for _ in range(100):
            try:
                socket.create_connection(("127.0.0.1", port), timeout=0.2).close()
                break
            except OSError:
                time.sleep(0.05)
        repo = work / "shop"
        (repo / "src").mkdir(parents=True)
        for i in range(200):
            (repo / "src" / ("m%03d.rs" % i)).write_text("pub fn f%d() -> u64 { %d }\n" % (i, i))
        (repo / "src" / "pricing.rs").write_text("pub fn total(cents: u64, qty: u64) -> u64 { cents * qty }\n")
        sh([agit, "init", "."], repo)
        sh([agit, "add", "-A"], repo)
        sh([agit, "commit", "-m", "seed", "--intent", "fixture"], repo)
        sh([agit, "remote", "add", "origin", "http://127.0.0.1:%d/api/v1/repos/shop" % port], repo)
        sh([agit, "push", "--create"], repo)
        sh([agit, "know", "add", "-t", "Totals are in minor units", "--tag", "kind:gotcha", "--tag", "area:pricing",
            "--path", "src/pricing.rs", "-b", "All money values are integer cents; never use floats."], repo)
        sh([str(HERE / "install.sh"), "--repo", str(repo), "--agent", "coder-1"], repo)
        hooks = repo / ".claude" / "hooks" / "agit"
        cache = work / "tmp" / "agit-claude-hooks"

        def hook(name, payload, extra=None):
            e = dict(env)
            if extra:
                e.update(extra)
            t0 = time.perf_counter()
            p = subprocess.run([str(hooks / name)], input=json.dumps(payload), cwd=str(repo), env=e, capture_output=True, text=True)
            return (time.perf_counter() - t0) * 1000.0, p

        def edit(path, tool="Edit"):
            return {"session_id": "s1", "cwd": str(repo), "hook_event_name": "PreToolUse", "tool_name": tool,
                    "tool_input": {"file_path": str(repo / path), "old_string": "a", "new_string": "b"}}

        def bash(cmd):
            return {"session_id": "s1", "cwd": str(repo), "hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": cmd}}

        def measure(label, fn, n=args.runs):
            times, last = [], None
            for i in range(n):
                dt, p = fn(i)
                times.append(dt)
                last = p
            s = sorted(times)
            r = {"median_ms": round(statistics.median(times), 1), "p95_ms": round(s[int(0.95 * (len(s) - 1))], 1),
                 "exit": last.returncode, "output": (last.stdout or last.stderr).strip()[:300]}
            results[label] = r
            print("%-44s %7.1f ms  (p95 %6.1f)  exit %d" % (label, r["median_ms"], r["p95_ms"], r["exit"]), flush=True)

        print("hook cost per call, %d runs each (agit %s)\n" % (args.runs, sh([agit, "--version"], repo).stdout.strip()), flush=True)
        measure("SessionStart: briefing", lambda i: hook("session-start.sh", {"cwd": str(repo), "source": "startup", "hook_event_name": "SessionStart"}))

        def pre_cold(i):
            shutil.rmtree(cache, ignore_errors=True)
            return hook("pre-edit.sh", edit("src/pricing.rs"))
        measure("PreToolUse Edit: lease check + context (cold)", pre_cold)
        measure("PreToolUse Edit: lease check + context (cached)", lambda i: hook("pre-edit.sh", edit("src/pricing.rs")))

        measure("PostToolUse Edit: claim lease (new file each)", lambda i: hook("post-edit.sh", dict(edit("src/m%03d.rs" % i), hook_event_name="PostToolUse")))
        measure("PostToolUse Edit: lease already held", lambda i: hook("post-edit.sh", dict(edit("src/m000.rs"), hook_event_name="PostToolUse")))

        # another agent holds src/pricing.rs exclusively: the edit is blocked
        sh([agit, "lease", "claim", "src/pricing.rs", "--holder", "coder-2", "--ttl", "30m", "--note", "tax rules"], repo)
        measure("PreToolUse Edit: blocked by coder-2's lease", lambda i: hook("pre-edit.sh", edit("src/pricing.rs")))
        sh([agit, "lease", "release", "--all", "--holder", "coder-2"], repo)

        def stop(i):
            (repo / "src" / "m199.rs").write_text("pub fn f199() -> u64 { %d }\n" % (1000 + i))
            return hook("checkpoint.sh", {"cwd": str(repo), "hook_event_name": "Stop"})
        measure("Stop: checkpoint of a changed tree", stop, n=max(10, args.runs // 2))

        measure("PreToolUse Bash: `cargo test` (passes)", lambda i: hook("pre-commit-guard.sh", bash("cargo test")))
        measure("PreToolUse Bash: `agit commit -m x` (blocked)", lambda i: hook("pre-commit-guard.sh", bash("agit commit -m x")))

        per_edit = results["PreToolUse Edit: lease check + context (cached)"]["median_ms"] + results["PostToolUse Edit: lease already held"]["median_ms"]
        per_edit_cold = results["PreToolUse Edit: lease check + context (cold)"]["median_ms"] + results["PostToolUse Edit: claim lease (new file each)"]["median_ms"]
        results["per_edit_ms"] = {"cached": round(per_edit, 1), "first_touch": round(per_edit_cold, 1)}
        print("\nper edit (PreToolUse + PostToolUse): %.0f ms when cached, %.0f ms on the first touch of a file" % (per_edit, per_edit_cold))
    finally:
        srv.terminate()
        srv.wait()
        if args.out:
            Path(args.out).write_text(json.dumps(results, indent=2))
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
