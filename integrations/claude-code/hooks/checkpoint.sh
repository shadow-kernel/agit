#!/usr/bin/env bash
# Stop / SubagentStop hook.
#
# When the working tree is dirty, record `agit checkpoint -m "claude-code
# stop"` so the state at the end of every turn can be restored with
# `agit restore <id>`. A fingerprint of the tree avoids a new checkpoint when
# nothing changed since the last one. Always exits 0 and never prints: a Stop
# hook that fails or exits 2 would keep Claude from stopping.

HOOK_DIR=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib.sh
. "$HOOK_DIR/lib.sh"

hook_disabled && exit 0
agit_available || exit 0
hook_read_input
root=$(agit_find_root "$(hook_cwd)") || exit 0

status=$(run_agit "$root" status) || exit 0
[ -n "$status" ] || exit 0

dirty=1
untracked=""
if have_py; then
    untracked=$(printf '%s' "$status" | python3 -I -c '
import json, sys
try:
    r = json.load(sys.stdin).get("result") or {}
except Exception:
    sys.exit(1)
n = len(r.get("staged") or []) + len(r.get("unstaged") or []) + len(r.get("untracked") or []) + len(r.get("conflicts") or [])
for p in (r.get("untracked") or [])[:500]:
    print(p)
sys.exit(0 if n else 1)
' 2>/dev/null)
    dirty=$?
elif have_jq; then
    untracked=$(printf '%s' "$status" | jq -r '.result.untracked[]?' 2>/dev/null | head -n 500)
    printf '%s' "$status" | jq -e '((.result.staged|length)+(.result.unstaged|length)+(.result.untracked|length)+(.result.conflicts|length)) > 0' >/dev/null 2>&1
    dirty=$?
fi
[ "$dirty" -eq 0 ] || exit 0

# Fingerprint: status JSON (carries blob ids of tracked changes) plus the
# content checksum of untracked files, so an unchanged tree is not
# checkpointed twice.
fp="$status"
if [ -n "$untracked" ]; then
    while IFS= read -r p; do
        [ -n "$p" ] || continue
        [ -f "$root/$p" ] && fp="$fp
$(cksum < "$root/$p" 2>/dev/null)"
    done <<EOF_UNTRACKED
$untracked
EOF_UNTRACKED
fi
fp=$(cache_key "$fp")

cache_dir_ready || true
mark="$HOOK_CACHE_DIR/cp-$(cache_key "$root")"
if [ -f "$mark" ] && [ "$(cat "$mark" 2>/dev/null)" = "$fp" ]; then
    exit 0
fi

if run_agit "$root" checkpoint -m "claude-code stop" --agent "$AGIT_AGENT" >/dev/null; then
    printf '%s' "$fp" > "$mark" 2>/dev/null
fi
exit 0
