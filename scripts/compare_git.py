#!/usr/bin/env python3
"""Side-by-side comparison of agit and git on identical workloads.

Everything runs in a fresh temporary directory. The git "remote" is a local
bare repository on disk and the agit remote is an agit-server bound to
127.0.0.1: nothing is ever pushed over the network, and the script refuses to
run a git push whose target is not a local path.

    python3 scripts/compare_git.py                      # defaults, about 10 minutes
    python3 scripts/compare_git.py --quick              # small sizes, about 2 minutes
    python3 scripts/compare_git.py --real-repo https://github.com/BurntSushi/ripgrep.git
    python3 scripts/compare_git.py --out results.json

Measured, for both tools, out of the box (default durability, no tuning):

  single     status, add+commit of one file, log, diff, branch+switch,
             snapshot and rollback of a dirty tree, reset
  crash      SIGKILL during `add -A` of many files; does the next command work
             or does it need manual cleanup (git's index.lock)?
  agents     N agents in their own worktrees (git) or workspaces (agit)
             committing concurrently: aggregate commits/s, latency, errors
  overlap    8 agents, two pairs want the same file: conflicts found at merge
             time (git) versus overlaps refused up front by leases (agit)
  push       N agents pushing every commit to a local remote
  history    on a real repository: import, log, log --path, blame, status

Needs: python3 >= 3.8, git, agit and agit-server on PATH (or --agit-bin-dir).
"""

import argparse
import hashlib
import json
import os
import random
import shutil
import signal
import socket
import statistics
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

# ---------------------------------------------------------------------------
# process helpers
# ---------------------------------------------------------------------------

ENV = {}


def run(cmd, cwd, check=True, env=None, timeout=600):
    t0 = time.perf_counter()
    p = subprocess.run(
        cmd, cwd=str(cwd), env=env or ENV, capture_output=True, text=True, timeout=timeout
    )
    dt = (time.perf_counter() - t0) * 1000.0
    if check and p.returncode != 0:
        raise RuntimeError(
            "command failed (%d): %s\n  in %s\n%s%s" % (p.returncode, " ".join(cmd), cwd, p.stdout[-800:], p.stderr[-800:])
        )
    return p.returncode, p.stdout, p.stderr, dt


def ms(cmd, cwd, env=None):
    return run(cmd, cwd, env=env)[3]


def med(xs):
    return round(statistics.median(xs), 2) if xs else None


def p95(xs):
    if not xs:
        return None
    s = sorted(xs)
    return round(s[min(len(s) - 1, int(round(0.95 * (len(s) - 1))))], 2)


def log(msg):
    print(msg, flush=True)


def tree_digest(root):
    """Digest of every file below root, ignoring .git and .agit."""
    h = hashlib.blake2b(digest_size=16)
    root = Path(root)
    for p in sorted(root.rglob("*")):
        rel = p.relative_to(root)
        if rel.parts and rel.parts[0] in (".git", ".agit"):
            continue
        if p.is_file() and not p.is_symlink():
            h.update(str(rel).encode())
            h.update(p.read_bytes())
    return h.hexdigest()


# ---------------------------------------------------------------------------
# fixtures
# ---------------------------------------------------------------------------


def write_tree(root, files, dirs, size):
    rnd = random.Random(42)
    root = Path(root)
    for i in range(files):
        p = root / "src" / ("mod%03d" % (i % dirs)) / ("file%05d.rs" % i)
        p.parent.mkdir(parents=True, exist_ok=True)
        body = "".join("fn f%d_%d() -> u64 { %d }\n" % (i, j, rnd.randint(0, 1 << 30)) for j in range(max(1, size // 32)))
        p.write_text("// file %d\n%s" % (i, body))
    (root / "README.md").write_text("# bench\n")


def git_init(path):
    run(["git", "init", "-q", "-b", "main", str(path)], path.parent)


def git_commit_all(path, msg):
    run(["git", "add", "-A"], path)
    run(["git", "commit", "-q", "-m", msg], path)


def agit_init(path):
    run(["agit", "init", str(path)], path.parent)


def agit_commit_all(path, msg):
    run(["agit", "add", "-A"], path)
    run(["agit", "commit", "-m", msg, "--intent", msg], path)


def make_pair(base, files, dirs, size):
    """Two repositories with identical content: <base>/git and <base>/agit."""
    g, a = base / "git", base / "agit"
    g.mkdir(parents=True)
    a.mkdir(parents=True)
    write_tree(g, files, dirs, size)
    write_tree(a, files, dirs, size)
    git_init(g)
    git_commit_all(g, "seed")
    agit_init(a)
    agit_commit_all(a, "seed")
    return g, a


# ---------------------------------------------------------------------------
# phases
# ---------------------------------------------------------------------------


def phase_single(work, files, dirs, size, n):
    log("\n== single agent: %d files, %d iterations" % (files, n))
    g, a = make_pair(work / "single", files, dirs, size)
    tools = {
        "git": {
            "status": ["git", "status", "--porcelain=v2"],
            "add": lambda f: ["git", "add", f],
            "commit": lambda m: ["git", "commit", "-q", "-m", m],
            "log": ["git", "log", "-n", "20", "--format=%H%x09%an%x09%s"],
            "diff": ["git", "diff"],
            "switch_new": lambda b: ["git", "switch", "-q", "-c", b],
            "switch": lambda b: ["git", "switch", "-q", b],
        },
        "agit": {
            "status": ["agit", "status", "--json"],
            "add": lambda f: ["agit", "add", f],
            "commit": lambda m: ["agit", "commit", "-m", m, "--intent", m, "--json"],
            "log": ["agit", "log", "-n", "20", "--json"],
            "diff": ["agit", "diff"],
            "switch_new": lambda b: ["agit", "switch", "-c", b],
            "switch": lambda b: ["agit", "switch", b],
        },
    }
    out = {}
    for name, repo in (("git", g), ("agit", a)):
        t = tools[name]
        r = {}
        r["status_clean_ms"] = med([ms(t["status"], repo) for _ in range(20)])
        commit_ms, cycle_ms, status_dirty, diff_ms = [], [], [], []
        for i in range(n):
            rel = "src/mod%03d/file%05d.rs" % (i % dirs, i % files)
            (repo / rel).write_text("// edited %d %s\n" % (i, time.time()))
            if i < 20:
                status_dirty.append(ms(t["status"], repo))
                diff_ms.append(ms(t["diff"], repo))
            t0 = time.perf_counter()
            ms(t["add"](rel), repo)
            c = ms(t["commit"]("micro %d" % i), repo)
            cycle_ms.append((time.perf_counter() - t0) * 1000.0)
            commit_ms.append(c)
        r["status_dirty_ms"] = med(status_dirty)
        r["diff_ms"] = med(diff_ms)
        r["add_commit_ms"] = med(cycle_ms)
        r["add_commit_p95_ms"] = p95(cycle_ms)
        r["commits_per_s"] = round(1000.0 / statistics.mean(cycle_ms), 1)
        r["log20_ms"] = med([ms(t["log"], repo) for _ in range(20)])
        sw = []
        for i in range(10):
            t0 = time.perf_counter()
            ms(t["switch_new"]("b%d" % i), repo)
            ms(t["switch"]("main"), repo)
            sw.append((time.perf_counter() - t0) * 1000.0)
        r["branch_switch_ms"] = med(sw)
        out[name] = r
        log("  %-5s status %.1f ms | add+commit %.1f ms (%.0f/s) | log %.1f ms | switch %.1f ms"
            % (name, r["status_clean_ms"], r["add_commit_ms"], r["commits_per_s"], r["log20_ms"], r["branch_switch_ms"]))
    out["files"] = files
    out["iterations"] = n
    return out


def dirty_up(repo, files, dirs, seed):
    rnd = random.Random(seed)
    for i in rnd.sample(range(files), min(200, files)):
        p = repo / ("src/mod%03d/file%05d.rs" % (i % dirs, i))
        p.write_text("// work in progress %d %d\n" % (seed, i))
    for j in range(20):
        p = repo / "notes" / ("scratch%02d.md" % j)
        p.parent.mkdir(exist_ok=True)
        p.write_text("untracked %d %d\n" % (seed, j))


def wreck(repo, files, dirs):
    for i in range(0, files, 7):
        p = repo / ("src/mod%03d/file%05d.rs" % (i % dirs, i))
        if p.exists():
            p.write_text("broken\n")
    shutil.rmtree(repo / "notes", ignore_errors=True)
    (repo / "junk.txt").write_text("junk\n")


def phase_rollback(work, files, dirs, size, n):
    log("\n== snapshot and rollback of a dirty tree (200 modified, 20 untracked files)")
    g, a = make_pair(work / "rollback", files, dirs, size)
    out = {}
    for name, repo in (("git", g), ("agit", a)):
        snap, back, ok = [], [], 0
        for k in range(n):
            dirty_up(repo, files, dirs, k)
            want = tree_digest(repo)
            if name == "git":
                t0 = time.perf_counter()
                run(["git", "stash", "push", "-u", "-q"], repo)
                run(["git", "stash", "apply", "-q"], repo)
                snap.append((time.perf_counter() - t0) * 1000.0)
                wreck(repo, files, dirs)
                t0 = time.perf_counter()
                run(["git", "reset", "--hard", "-q"], repo)
                run(["git", "clean", "-fdq"], repo)
                run(["git", "stash", "apply", "-q"], repo)
                back.append((time.perf_counter() - t0) * 1000.0)
            else:
                t0 = time.perf_counter()
                _, o, _, _ = run(["agit", "checkpoint", "-m", "snap %d" % k, "--json"], repo)
                snap.append((time.perf_counter() - t0) * 1000.0)
                cid = json.loads(o)["result"]["id"]
                wreck(repo, files, dirs)
                t0 = time.perf_counter()
                run(["agit", "restore", cid], repo)
                back.append((time.perf_counter() - t0) * 1000.0)
            if tree_digest(repo) == want:
                ok += 1
            # back to a clean tree for the next round
            if name == "git":
                run(["git", "reset", "--hard", "-q"], repo)
                run(["git", "clean", "-fdq"], repo)
            else:
                run(["agit", "restore", "HEAD"], repo)
                shutil.rmtree(repo / "notes", ignore_errors=True)
                if (repo / "junk.txt").exists():
                    (repo / "junk.txt").unlink()
        hist = None
        if name == "git":
            _, o, _, _ = run(["git", "log", "--oneline"], repo)
            hist = len(o.splitlines())
        else:
            _, o, _, _ = run(["agit", "log", "--json"], repo)
            hist = len(json.loads(o)["result"])
        out[name] = {"snapshot_ms": med(snap), "rollback_ms": med(back), "exact_restores": ok, "rounds": n, "branch_history_commits": hist}
        log("  %-5s snapshot %.1f ms | rollback %.1f ms | exact %d/%d | history still %d commit(s)" % (name, med(snap), med(back), ok, n, hist))
    return out


def phase_crash(work, files, dirs, size, trials):
    log("\n== crash: SIGKILL during `add -A` of %d changed files, %d trials" % (files, trials))
    g, a = make_pair(work / "crash", files, dirs, size)
    out = {}
    for name, repo in (("git", g), ("agit", a)):
        add = ["git", "add", "-A"] if name == "git" else ["agit", "add", "-A"]
        status = ["git", "status", "--porcelain"] if name == "git" else ["agit", "status", "--json"]
        rnd = random.Random(7)
        broken, killed_mid = 0, 0
        samples = []
        for t in range(trials):
            for i in range(files):
                p = repo / ("src/mod%03d/file%05d.rs" % (i % dirs, i))
                p.write_text("// trial %d file %d %s\n" % (t, i, "x" * 200))
            proc = subprocess.Popen(add, cwd=str(repo), env=ENV, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            time.sleep(rnd.uniform(0.005, 0.08))
            if proc.poll() is None:
                killed_mid += 1
                proc.send_signal(signal.SIGKILL)
            proc.wait()
            # An agent retries the interrupted operation.
            rc, o, e, _ = run(add, repo, check=False)
            if rc != 0:
                broken += 1
                if len(samples) < 1:
                    samples.append((e or o).strip().splitlines()[0][:160])
                # what a human would have to do before anything works again
                lock = repo / ".git" / "index.lock"
                if lock.exists():
                    lock.unlink()
                run(add, repo, check=False)
            run(status, repo, check=False)
            if name == "git":
                run(["git", "commit", "-q", "-m", "trial %d" % t], repo, check=False)
            else:
                run(["agit", "commit", "-m", "trial %d" % t, "--intent", "crash trial"], repo, check=False)
        fsck = run(["git", "fsck", "--no-progress"] if name == "git" else ["agit", "fsck"], repo, check=False)[0] == 0
        out[name] = {"trials": trials, "killed_mid_operation": killed_mid, "next_command_failed": broken, "fsck_clean": fsck, "sample_error": samples[0] if samples else None}
        log("  %-5s killed mid-op %d/%d | next command failed %d | fsck clean %s%s"
            % (name, killed_mid, trials, broken, fsck, ("  e.g. " + samples[0]) if samples else ""))
    return out


def setup_agents(base, files, dirs, size, k, with_remote=False, server_url=None):
    g, a = make_pair(base, files, dirs, size)
    gw, aw = [], []
    for i in range(k):
        d = base / ("git-wt-%02d" % i)
        run(["git", "worktree", "add", "-q", "-b", "agent-%02d" % i, str(d)], g)
        gw.append(d)
        d = base / ("agit-ws-%02d" % i)
        run(["agit", "ws", "new", "agent-%02d" % i, "--path", str(d), "--agent", "agent-%02d" % i], a)
        aw.append(d)
    return g, a, gw, aw


def agent_loop(tool, wd, i, m, files, dirs, lat, errs, push=None):
    env = dict(ENV)
    env["AGIT_AGENT"] = "agent-%02d" % i
    for j in range(m):
        rel = "src/agent%02d/work%03d.rs" % (i, j % 10)
        p = wd / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text("// agent %d step %d %s\n" % (i, j, time.time()))
        t0 = time.perf_counter()
        try:
            if tool == "git":
                run(["git", "add", rel], wd, env=env)
                run(["git", "commit", "-q", "-m", "agent %d step %d" % (i, j)], wd, env=env)
                if push:
                    run(["git", "push", "-q", "origin", "agent-%02d" % i], wd, env=env)
            else:
                run(["agit", "add", rel], wd, env=env)
                run(["agit", "commit", "-m", "agent %d step %d" % (i, j), "--intent", "parallel work"], wd, env=env)
                if push:
                    run(["agit", "push", "origin", "ws/agent-%02d" % i], wd, env=env)
        except Exception as ex:  # noqa: BLE001 - count and continue
            errs.append(str(ex).splitlines()[0][:200])
            continue
        lat.append((time.perf_counter() - t0) * 1000.0)


def phase_agents(work, files, dirs, size, ks, m):
    log("\n== many agents in one repository, %d commits each" % m)
    out = []
    for k in ks:
        base = work / ("agents-%d" % k)
        g, a, gw, aw = setup_agents(base, files, dirs, size, k)
        row = {"agents": k}
        for name, wds in (("git", gw), ("agit", aw)):
            lat, errs = [], []
            threads = [threading.Thread(target=agent_loop, args=(name, wd, i, m, files, dirs, lat, errs)) for i, wd in enumerate(wds)]
            t0 = time.perf_counter()
            for t in threads:
                t.start()
            for t in threads:
                t.join()
            wall = time.perf_counter() - t0
            row[name] = {"commits": len(lat), "errors": len(errs), "commits_per_s": round(len(lat) / wall, 1), "p50_ms": med(lat), "p95_ms": p95(lat), "first_error": errs[0] if errs else None}
        out.append(row)
        log("  %2d agents | git %6.1f commits/s (p95 %6.1f ms, errors %d) | agit %6.1f commits/s (p95 %6.1f ms, errors %d)"
            % (k, row["git"]["commits_per_s"], row["git"]["p95_ms"] or 0, row["git"]["errors"], row["agit"]["commits_per_s"], row["agit"]["p95_ms"] or 0, row["agit"]["errors"]))
    return out


def phase_overlap(work, files, dirs, size):
    log("\n== 8 agents, two pairs want the same file")
    k = 8
    targets = {0: 1, 1: 1, 2: 2, 3: 2, 4: 4, 5: 5, 6: 6, 7: 7}  # agent -> file number
    base = work / "overlap"
    g, a, gw, aw = setup_agents(base, files, dirs, size, k)

    def target(i):
        return "src/mod%03d/file%05d.rs" % (targets[i] % dirs, targets[i])

    def spare(i):
        return "src/mod%03d/file%05d.rs" % ((100 + i) % dirs, 100 + i)

    # git: everyone edits, commits; conflicts appear when integrating
    def git_agent(i):
        wd = gw[i]
        (wd / target(i)).write_text("// agent %d rewrote this\n" % i)
        run(["git", "commit", "-q", "-am", "agent %d" % i], wd)

    ts = [threading.Thread(target=git_agent, args=(i,)) for i in range(k)]
    [t.start() for t in ts]
    [t.join() for t in ts]
    git_conf = 0
    for i in range(k):
        rc, _, _, _ = run(["git", "merge", "-q", "--no-edit", "agent-%02d" % i], g, check=False)
        if rc != 0:
            git_conf += 1
            run(["git", "merge", "--abort"], g, check=False)

    # agit: claim before editing; a refused claim costs milliseconds and the
    # agent moves to other work
    refused, detect_ms = [], []
    lock = threading.Lock()

    def agit_agent(i):
        wd = aw[i]
        env = dict(ENV)
        env["AGIT_AGENT"] = "agent-%02d" % i
        rel = target(i)
        rc, o, e, dt = run(["agit", "lease", "claim", rel, "--ttl", "10m", "--json"], wd, check=False, env=env)
        if rc != 0:
            with lock:
                refused.append(i)
                detect_ms.append(dt)
            rel = spare(i)
            run(["agit", "lease", "claim", rel, "--ttl", "10m"], wd, env=env)
        (wd / rel).write_text("// agent %d rewrote this\n" % i)
        run(["agit", "commit", "-a", "-m", "agent %d" % i, "--intent", "overlap test"], wd, env=env)
        # done: hand the paths back, as the agent loop prescribes
        run(["agit", "lease", "release", "--all"], wd, env=env)

    ts = [threading.Thread(target=agit_agent, args=(i,)) for i in range(k)]
    [t.start() for t in ts]
    [t.join() for t in ts]
    agit_conf, agit_err = 0, None
    for i in range(k):
        rc, o, e, _ = run(["agit", "merge", "ws/agent-%02d" % i, "-m", "merge %d" % i, "--intent", "integrate"], a, check=False)
        if rc != 0:
            agit_conf += 1
            agit_err = agit_err or (e or o).strip().splitlines()[-1][:200]
            run(["agit", "merge", "--abort"], a, check=False)
    out = {
        "agents": k,
        "overlapping_pairs": 2,
        "git": {"merge_conflicts_after_work": git_conf},
        "agit": {"merge_conflicts_after_work": agit_conf, "overlaps_refused_before_work": len(refused), "refusal_ms": med(detect_ms), "merge_error": agit_err},
    }
    log("  git  : %d merge conflicts, found after both agents finished their work" % git_conf)
    log("  agit : %d overlaps refused before any edit (median %.1f ms), %d merge conflicts%s" % (len(refused), med(detect_ms) or 0, agit_conf, ("  (" + agit_err + ")") if agit_err else ""))
    return out


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


def phase_push(work, files, dirs, size, ks, m):
    log("\n== pushing every commit to a local remote, %d commits per agent" % m)
    out = []
    for k in ks:
        base = work / ("push-%d" % k)
        g, a, gw, aw = setup_agents(base, files, dirs, size, k)
        # git: local bare repository, never a network URL
        bare = base / "remote.git"
        run(["git", "init", "-q", "--bare", str(bare)], base)
        url = str(bare)
        assert "://" not in url and "@" not in url, "refusing a non-local git remote"
        run(["git", "remote", "add", "origin", url], g)
        run(["git", "push", "-q", "origin", "main"], g)
        # agit: a local agit-server on loopback
        port = free_port()
        data = base / "agit-server-data"
        srv = subprocess.Popen(["agit-server", "--data-dir", str(data), "--listen", "127.0.0.1:%d" % port], env=ENV, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            for _ in range(100):
                try:
                    socket.create_connection(("127.0.0.1", port), timeout=0.2).close()
                    break
                except OSError:
                    time.sleep(0.05)
            run(["agit", "remote", "add", "origin", "http://127.0.0.1:%d/api/v1/repos/bench" % port], a)
            run(["agit", "push", "--create"], a)
            row = {"agents": k}
            for name, wds in (("git", gw), ("agit", aw)):
                lat, errs = [], []
                threads = [threading.Thread(target=agent_loop, args=(name, wd, i, m, files, dirs, lat, errs, True)) for i, wd in enumerate(wds)]
                t0 = time.perf_counter()
                [t.start() for t in threads]
                [t.join() for t in threads]
                wall = time.perf_counter() - t0
                row[name] = {"commits_pushed": len(lat), "errors": len(errs), "per_s": round(len(lat) / wall, 1), "p50_ms": med(lat), "p95_ms": p95(lat), "first_error": errs[0] if errs else None}
            out.append(row)
            log("  %2d agents | git %6.1f commit+push/s (p50 %6.1f ms, errors %d) | agit %6.1f commit+push/s (p50 %6.1f ms, errors %d)"
                % (k, row["git"]["per_s"], row["git"]["p50_ms"] or 0, row["git"]["errors"], row["agit"]["per_s"], row["agit"]["p50_ms"] or 0, row["agit"]["errors"]))
        finally:
            srv.terminate()
            srv.wait()
    return out


def phase_history(work, url):
    log("\n== history on a real repository: %s" % url)
    base = work / "history"
    base.mkdir()
    mirror = base / "src.git"
    run(["git", "clone", "-q", "--mirror", url, str(mirror)], base, timeout=1200)
    g = base / "git"
    run(["git", "clone", "-q", str(mirror), str(g)], base)
    t0 = time.perf_counter()
    run(["agit", "git", "import", str(mirror), "--into", str(base / "agit")], base, timeout=3600)
    import_s = time.perf_counter() - t0
    a = base / "agit"
    _, o, _, _ = run(["git", "rev-list", "--count", "HEAD"], g)
    commits = int(o.strip())
    _, o, _, _ = run(["git", "ls-files"], g)
    tracked = o.splitlines()
    # a file with a long history
    _, o, _, _ = run(["git", "log", "--format=", "--name-only", "-n", "400"], g)
    counts = {}
    for line in o.splitlines():
        if line and line in tracked:
            counts[line] = counts.get(line, 0) + 1
    hot = max(counts, key=counts.get) if counts else tracked[0]
    out = {"url": url, "commits": commits, "files": len(tracked), "agit_import_s": round(import_s, 2), "hot_file": hot}
    for name, repo in (("git", g), ("agit", a)):
        if name == "git":
            cmds = {
                "status": ["git", "status", "--porcelain=v2"],
                "log20": ["git", "log", "-n", "20", "--format=%H%x09%an%x09%s"],
                "log_path": ["git", "log", "-n", "20", "--format=%H%x09%s", "--", hot],
                "blame": ["git", "blame", "--porcelain", hot],
            }
        else:
            cmds = {
                "status": ["agit", "status", "--json"],
                "log20": ["agit", "log", "-n", "20", "--json"],
                "log_path": ["agit", "log", "-n", "20", "--path", hot, "--json"],
                "blame": ["agit", "blame", hot, "--json"],
            }
        out[name] = {k: med([ms(c, repo) for _ in range(10)]) for k, c in cmds.items()}
        log("  %-5s status %.1f | log -n 20 %.1f | log --path %.1f | blame %.1f ms" % (name, out[name]["status"], out[name]["log20"], out[name]["log_path"], out[name]["blame"]))
    log("  agit git import: %d commits, %d files in %.1f s" % (commits, len(tracked), import_s))
    return out


# ---------------------------------------------------------------------------


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--files", type=int, default=5000)
    ap.add_argument("--dirs", type=int, default=100)
    ap.add_argument("--size", type=int, default=2048, help="approximate bytes per file")
    ap.add_argument("--iterations", type=int, default=100)
    ap.add_argument("--agents", default="1,4,8,16")
    ap.add_argument("--agent-commits", type=int, default=25)
    ap.add_argument("--crash-trials", type=int, default=20)
    ap.add_argument("--real-repo", default=None, help="git URL or path for the history phase (cloned read-only)")
    ap.add_argument("--phases", default="single,rollback,crash,agents,overlap,push,history")
    ap.add_argument("--agit-bin-dir", default=None)
    ap.add_argument("--keep", action="store_true", help="keep the temporary directory")
    ap.add_argument("--quick", action="store_true")
    ap.add_argument("--out", default=None)
    args = ap.parse_args()
    if args.quick:
        args.files, args.iterations, args.agent_commits, args.crash_trials, args.agents = 1000, 30, 10, 8, "1,4,8"

    global ENV
    work = Path(tempfile.mkdtemp(prefix="agit-vs-git-"))
    home = work / "home"
    home.mkdir()
    gitconfig = home / ".gitconfig"
    gitconfig.write_text("[user]\n\tname = bench\n\temail = bench@example.com\n[commit]\n\tgpgsign = false\n[init]\n\tdefaultBranch = main\n")
    ENV = dict(os.environ)
    ENV.update({
        "HOME": str(home),
        "GIT_CONFIG_GLOBAL": str(gitconfig),
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_TERMINAL_PROMPT": "0",
        "AGIT_AUTHOR_NAME": "bench",
        "AGIT_AUTHOR_EMAIL": "bench@example.com",
        "AGIT_AGENT": "bench",
        "AGIT_AUTO_LOGIN": "0",
        "LC_ALL": "C",
    })
    for k in ("AGIT_TOKEN", "AGIT_SSH_KEY", "AGIT_FSYNC", "AGIT_PACK", "AGIT_JSON"):
        ENV.pop(k, None)
    if args.agit_bin_dir:
        ENV["PATH"] = args.agit_bin_dir + os.pathsep + ENV["PATH"]

    ver = {
        "git": run(["git", "--version"], work)[1].strip(),
        "agit": run(["agit", "--version"], work)[1].strip(),
    }
    log("agit vs git, out of the box, everything local in %s" % work)
    log("%s | %s | %s" % (ver["agit"], ver["git"], " ".join(os.uname()[0:1] + os.uname()[2:3] + os.uname()[4:5])))
    phases = args.phases.split(",")
    ks = [int(x) for x in args.agents.split(",")]
    res = {"versions": ver, "machine": os.uname()._asdict() if hasattr(os.uname(), "_asdict") else list(os.uname()), "args": vars(args), "started": time.strftime("%Y-%m-%d %H:%M:%S")}
    try:
        if "single" in phases:
            res["single"] = phase_single(work, args.files, args.dirs, args.size, args.iterations)
        if "rollback" in phases:
            res["rollback"] = phase_rollback(work, args.files, args.dirs, args.size, 10)
        if "crash" in phases:
            res["crash"] = phase_crash(work, max(2000, args.files // 2), args.dirs, args.size, args.crash_trials)
        if "agents" in phases:
            res["agents"] = phase_agents(work, args.files, args.dirs, args.size, ks, args.agent_commits)
        if "overlap" in phases:
            res["overlap"] = phase_overlap(work, args.files, args.dirs, args.size)
        if "push" in phases:
            res["push"] = phase_push(work, min(args.files, 2000), args.dirs, args.size, [k for k in ks if k > 1] or [4], args.agent_commits)
        if "history" in phases and args.real_repo:
            res["history"] = phase_history(work, args.real_repo)
    finally:
        res["finished"] = time.strftime("%Y-%m-%d %H:%M:%S")
        if args.out:
            Path(args.out).write_text(json.dumps(res, indent=2))
            log("\nresults written to %s" % args.out)
        if not args.keep:
            shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
