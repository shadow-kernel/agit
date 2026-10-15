#!/usr/bin/env bash
# PreToolUse hook for Edit|Write|MultiEdit|NotebookEdit.
#
# 1. `agit lease check <path> --holder $AGIT_AGENT`: if another agent holds an
#    exclusive lease on the path, exit 2 so Claude Code blocks the edit and
#    shows the reason to Claude.
# 2. `agit know context <path> --json` (cached 60 s per path): a compact
#    summary (note titles, suggested tags, open PRs touching the path, local
#    leases by other agents) is returned as additionalContext. Nothing is
#    printed when there is nothing relevant; failures (no remote, server down)
#    are silent.

HOOK_DIR=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib.sh
. "$HOOK_DIR/lib.sh"

hook_disabled && exit 0
agit_available || exit 0
hook_read_input
abs=$(hook_target_path) || exit 0
root=$(agit_find_root "$(dirname "$abs")") || exit 0
rel=$(rel_path "$abs" "$root")
case "$rel" in .agit | .agit/*) exit 0 ;; esac

# --- 1. lease check ---------------------------------------------------------
check=$(run_agit "$root" lease check "$rel" --holder "$AGIT_AGENT")
rc=$?
if [ "$rc" -ne 0 ]; then
    code=$(json_get "$check" error.code)
    if [ "$code" = "lease_conflict" ]; then
        msg=$(json_get "$check" error.message)
        expires=""
        if have_py; then
            leases=$(run_agit "$root" lease list)
            expires=$(printf '%s' "$leases" | python3 -I -c '
import json, sys, time, re
msg = sys.argv[1]
m = re.search(r"\(lease ([0-9a-f]+)\)", msg)
if not m:
    sys.exit(0)
try:
    leases = json.load(sys.stdin).get("result") or []
except Exception:
    sys.exit(0)
for l in leases:
    if l.get("id") == m.group(1):
        left = int((l.get("expires_ms", 0) - time.time() * 1000) / 60000)
        note = l.get("note") or ""
        print(" It expires in about %d min%s." % (max(left, 0), (" (note: %s)" % note) if note else ""))
' "$msg" 2>/dev/null)
        fi
        printf 'agit blocked this edit: %s.%s\nAnother agent is working on this path. Do not edit it now: pick other work, wait for the lease to expire (`agit lease list`), or ask the human to coordinate. Only a human decision should override a lease (`agit commit --force`).\n' "$msg" "$expires" >&2
        exit 2
    fi
    # Other errors (path outside worktree, unborn repo, ...) are not ours to report.
fi

# --- 2. context summary (cached) -------------------------------------------
cache_dir_ready || exit 0
key=$(cache_key "$root" "$rel" "$AGIT_AGENT")
cache="$HOOK_CACHE_DIR/ctx-$key"
summary=""
if cache_fresh "$cache" 1; then
    summary=$(cat "$cache" 2>/dev/null)
else
    ctx=$(run_agit "$root" know context "$rel")
    if [ $? -eq 0 ] && [ -n "$ctx" ] && have_py; then
        summary=$(printf '%s' "$ctx" | python3 -I -c '
import json, sys
try:
    r = json.load(sys.stdin).get("result") or {}
except Exception:
    sys.exit(0)
lines = []
notes = r.get("notes") or []
if notes:
    items = []
    for n in notes[:4]:
        tags = [t for t in n.get("tags", []) if t.startswith("kind:")]
        items.append("\"%s\"%s (%s)" % (n.get("title", "?"), (" [" + ",".join(tags) + "]") if tags else "", n.get("id", "?")))
    more = " (+%d more: `agit know search --path %s`)" % (len(notes) - 4, sys.argv[1]) if len(notes) > 4 else ""
    lines.append("- notes: " + "; ".join(items) + more + ". Read one with `agit know get <id>`.")
tags = r.get("suggested_tags") or []
if tags:
    lines.append("- tags for notes about this path: " + ", ".join(tags[:8]))
pulls = r.get("open_pulls") or []
if pulls:
    items = ["#%s \"%s\" (%s)" % (p.get("number"), p.get("title", ""), (p.get("author") or {}).get("id", "?")) for p in pulls[:4]]
    lines.append("- open PRs touching it: " + "; ".join(items))
if lines:
    print("\n".join(lines))
' "$rel" 2>/dev/null)
    fi
    # Cache even an empty result so a missing remote is not retried per edit.
    printf '%s' "$summary" > "$cache" 2>/dev/null
fi

# Local leases by other agents on this path (shared mode, informational).
local_note=""
if have_py; then
    leases=$(run_agit "$root" lease list)
    [ -n "$leases" ] && local_note=$(printf '%s' "$leases" | python3 -I -c '
import json, sys, fnmatch, time
rel, me = sys.argv[1], sys.argv[2]
def covers(pattern, path):
    pattern = pattern.rstrip("/"); path = path.rstrip("/")
    if pattern in ("", "."): return True
    if any(c in pattern for c in "*?["):
        cur = path
        while True:
            if fnmatch.fnmatchcase(cur, pattern) or fnmatch.fnmatchcase(cur, pattern.replace("**/", "")): return True
            i = cur.rfind("/")
            if i < 0: return False
            cur = cur[:i]
    return path == pattern or path.startswith(pattern + "/")
try:
    leases = json.load(sys.stdin).get("result") or []
except Exception:
    sys.exit(0)
others = [l for l in leases if l.get("holder") != me and any(covers(p, rel) for p in l.get("paths", []))]
if others:
    items = []
    for l in others[:3]:
        left = max(int((l.get("expires_ms", 0) - time.time() * 1000) / 60000), 0)
        items.append("%s (%s, %dm left)" % (l.get("holder"), l.get("mode"), left))
    print("- other agents on this path: " + "; ".join(items))
' "$rel" "$AGIT_AGENT" 2>/dev/null)
fi

out=""
if [ -n "$summary" ] || [ -n "$local_note" ]; then
    out="agit context for $rel:"
    [ -n "$summary" ] && out="$out
$summary"
    [ -n "$local_note" ] && out="$out
$local_note"
    out=$(truncate_text "$out" 800)
    emit_context PreToolUse "$out"
fi
exit 0
