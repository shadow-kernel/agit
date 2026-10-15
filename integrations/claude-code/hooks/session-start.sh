#!/usr/bin/env bash
# SessionStart hook (startup, resume, clear, compact).
#
# Returns a short briefing as additionalContext: repository status, active
# leases (who works where) and the agit loop Claude is expected to follow.

HOOK_DIR=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib.sh
. "$HOOK_DIR/lib.sh"

hook_disabled && exit 0
hook_read_input
cwd=$(hook_cwd)
root=$(agit_find_root "$cwd") || exit 0

if ! agit_available; then
    emit_context SessionStart "This directory is an agit repository ($root) but the \`agit\` binary is not on PATH, so the agit hooks are inactive. Install agit (cargo install --path crates/agit-cli, or see README.md) before committing."
    exit 0
fi

status=$(run_agit "$root" status) || status=""
leases=$(run_agit "$root" lease list) || leases=""
remotes=$(run_agit "$root" remote list) || remotes=""

brief=""
if have_py; then
    brief=$(python3 -I -c '
import json, sys, time
root, me, status, leases, remotes = sys.argv[1:6]
def load(s):
    try:
        return json.loads(s).get("result")
    except Exception:
        return None
st = load(status) or {}
ls = load(leases) or []
rs = load(remotes) or []
parts = []
branch = st.get("branch") or "(detached)"
head = (st.get("head") or "")[:8] or "unborn"
counts = []
for k in ("staged", "unstaged", "untracked", "conflicts"):
    n = len(st.get(k) or [])
    if n:
        counts.append("%d %s" % (n, k))
ws = st.get("workspace")
line = "agit repository %s: branch %s, HEAD %s" % (root, branch, head)
if ws and ws != "main":
    line += ", workspace %s" % ws
line += ", " + (", ".join(counts) if counts else "clean working tree")
if st.get("merge_in_progress"):
    line += ", MERGE IN PROGRESS"
if rs:
    line += "; remote %s -> %s" % (rs[0].get("name"), rs[0].get("url"))
else:
    line += "; no remote configured (knowledge/PR commands need `agit remote add origin <url>`)"
parts.append(line + ".")
parts.append("You act as agent `%s` (AGIT_AGENT). Every commit, lease and note carries this id." % me)
if ls:
    items = []
    for l in ls[:8]:
        left = max(int((l.get("expires_ms", 0) - time.time() * 1000) / 60000), 0)
        who = "you" if l.get("holder") == me else l.get("holder")
        items.append("%s: %s (%s, %dm left%s)" % (who, ", ".join(l.get("paths", [])), l.get("mode"), left, (", " + l["note"]) if l.get("note") else ""))
    more = " (+%d more)" % (len(ls) - 8) if len(ls) > 8 else ""
    parts.append("Active leases: " + "; ".join(items) + more + ". Paths leased exclusively by others are off limits; the pre-edit hook will block edits there.")
else:
    parts.append("No active leases.")
print("\n".join(parts))
' "$root" "$AGIT_AGENT" "$status" "$leases" "$remotes" 2>/dev/null)
fi
[ -n "$brief" ] || brief="agit repository: $root. You act as agent \`$AGIT_AGENT\`."

loop='agit hooks are active in this session: before each edit the lease on the path is checked and context notes are injected; after each edit an exclusive 30m lease is claimed for you; when you stop, a checkpoint of the dirty tree is recorded; `agit commit` without --intent is refused.
The loop: `agit know context <path>` before changing a path (or the agit_context MCP tool) -> `agit lease claim <paths> --ttl 30m --note "..."` for larger scopes -> `agit checkpoint -m "..."` before risky edits -> `agit add` + `agit commit -m "what" --intent "why" --task <id>` -> `agit pr create --push -t "..." --intent "..."` when done -> `agit know add -t "..." --tag kind:gotcha --tag area:<x> --path <glob> -b "..."` for what you learned -> `agit lease release --all` at the end. Use agit, not git, in this repository. Branch on `error.code` in JSON output (lease_conflict: pick other work; ref_conflict: pull and retry; dirty_worktree: checkpoint or commit first).'

emit_context SessionStart "$brief
$loop"
exit 0
