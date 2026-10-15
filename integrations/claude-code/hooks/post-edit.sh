#!/usr/bin/env bash
# PostToolUse hook for Edit|Write|MultiEdit|NotebookEdit.
#
# After a successful edit, make sure $AGIT_AGENT holds an exclusive lease
# covering the path; claim one (`agit lease claim <path> --ttl 30m`) when it
# does not. Silent on success. If the claim fails because another agent took
# the path in the meantime, a warning is returned as additionalContext.

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

ttl="${AGIT_HOOK_LEASE_TTL:-30m}"

# Already covered by one of our own exclusive leases?
leases=$(run_agit "$root" lease list) || leases=""
covered=1
if [ -n "$leases" ]; then
    if have_py; then
        printf '%s' "$leases" | python3 -I -c '
import json, sys, fnmatch
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
    sys.exit(1)
ok = any(l.get("holder") == me and l.get("mode") == "exclusive" and any(covers(p, rel) for p in l.get("paths", [])) for l in leases)
sys.exit(0 if ok else 1)
' "$rel" "$AGIT_AGENT" 2>/dev/null
        covered=$?
    elif have_jq; then
        # Without python: exact path or directory-prefix match only.
        printf '%s' "$leases" | jq -e --arg rel "$rel" --arg me "$AGIT_AGENT" \
            '[.result[]? | select(.holder == $me and .mode == "exclusive") | .paths[] | select(. == $rel or ($rel | startswith(. + "/")) or . == "." or . == "")] | length > 0' \
            >/dev/null 2>&1
        covered=$?
    fi
fi
[ "$covered" -eq 0 ] && exit 0

claim=$(run_agit "$root" lease claim "$rel" --holder "$AGIT_AGENT" --ttl "$ttl" --note "claude-code edit")
rc=$?
if [ "$rc" -ne 0 ]; then
    code=$(json_get "$claim" error.code)
    if [ "$code" = "lease_conflict" ]; then
        msg=$(json_get "$claim" error.message)
        emit_context PostToolUse "agit warning: $msg. Another agent claimed this path after your edit started; your commit will be rejected with lease_conflict. Coordinate before continuing, or let the human decide."
    fi
fi
exit 0
