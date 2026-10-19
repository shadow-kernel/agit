#!/usr/bin/env bash
# End-to-end load test against a running agit-server (for example the Docker
# image): N agents, each in its own clone and on its own branch, commit and
# push micro-commits concurrently. Reports aggregate commits per second.
#
#   scripts/loadtest.sh <base-url> [agents] [commits-per-agent] [repo-name]
#   scripts/loadtest.sh http://127.0.0.1:7878/api/v1 8 100
#
# Needs `agit` on PATH and credentials for the server: AGIT_TOKEN, or an SSH
# key the server knows (`agit auth login --url <base-url>` first, or let the
# first request sign in automatically). Everything happens in a temporary
# directory; the repository on the server is created if missing and keeps
# the pushed branches `load/<run>/agent-<n>` afterwards.
#
# What is measured, end to end over HTTP: `agit add` + `agit commit` (local
# work including the client's fsync) + `agit push` (want/have negotiation,
# object upload, compare-and-swap ref update) per micro-commit. Phase 2
# measures only pushes of already-made commits, which is the server's ref
# and object path without the client's commit cost.

set -euo pipefail

BASE="${1:?usage: loadtest.sh <base-url> [agents] [commits] [repo]}"
AGENTS="${2:-8}"
COMMITS="${3:-100}"
REPO="${4:-loadtest}"
FILES="${LOADTEST_FILES:-500}"
AGIT="${AGIT_BIN:-agit}"

now() { python3 -c 'import time; print(time.time())'; }
elapsed() { python3 -c "print(round($(now) - $1, 2))"; }
rate() { python3 -c "print(round($1 / max($(now) - $2, 1e-9), 1))"; }

command -v "$AGIT" >/dev/null || { echo "agit not found on PATH (set AGIT_BIN)"; exit 1; }
WORK="$(mktemp -d "${TMPDIR:-/tmp}/agit-loadtest.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
RUN="$(date +%Y%m%d%H%M%S)"
URL="$BASE/repos/$REPO"

echo "server:  $BASE"
echo "repo:    $REPO  agents: $AGENTS  commits/agent: $COMMITS  seed files: $FILES"
echo "workdir: $WORK"
echo

# --- seed ------------------------------------------------------------------
mkdir -p "$WORK/seed" && cd "$WORK/seed"
"$AGIT" init >/dev/null
for i in $(seq 1 "$FILES"); do
  d="src/mod$((i % 25))"; mkdir -p "$d"; printf '// file %s\nfn f%s() {}\n' "$i" "$i" > "$d/file$i.rs"
done
"$AGIT" add -A >/dev/null
AGIT_AGENT=seed "$AGIT" commit -m "seed $FILES files" --intent "load test seed" >/dev/null
"$AGIT" remote add origin "$URL" >/dev/null
if ! "$AGIT" push --create --json >/dev/null 2>&1; then
  "$AGIT" push --force >/dev/null
fi
echo "seeded $REPO with $FILES files"

# --- clone one working copy per agent --------------------------------------
t0=$(now)
for a in $(seq 1 "$AGENTS"); do
  ( cd "$WORK" && "$AGIT" clone "$URL" "agent-$a" >/dev/null 2>&1 ) &
done
wait
echo "cloned $AGENTS working copies in $(elapsed "$t0")s"
echo

# --- phase 1: commit + push per micro-commit --------------------------------
agent_loop() {
  local a="$1" dir="$WORK/agent-$1" fail=0
  cd "$dir"
  "$AGIT" switch -c "load/$RUN/agent-$a" >/dev/null
  export AGIT_AGENT="agent-$a" AGIT_RUN_ID="$RUN"
  for i in $(seq 1 "$COMMITS"); do
    printf '// agent %s edit %s %s\n' "$a" "$i" "$(now)" > "src/mod$((i % 25))/file$(( (a * 7 + i) % FILES + 1 )).rs"
    "$AGIT" add -A >/dev/null
    "$AGIT" commit -m "micro $i" --intent "load test agent $a" --task "LOAD-$a" >/dev/null
    "$AGIT" push >/dev/null 2>&1 || fail=$((fail + 1))
  done
  echo "$fail" > "$dir/.failures"
}

t1=$(now)
for a in $(seq 1 "$AGENTS"); do agent_loop "$a" & done
wait
total=$((AGENTS * COMMITS))
failed=0; for a in $(seq 1 "$AGENTS"); do failed=$((failed + $(cat "$WORK/agent-$a/.failures"))); done
echo "phase 1  commit+push:   $total micro-commits in $(elapsed "$t1")s  =  $(rate "$total" "$t1") commits/s aggregate  (push failures: $failed)"

# --- phase 2: push-only (commits made offline first) ------------------------
prep_loop() {
  local a="$1" dir="$WORK/agent-$1"
  cd "$dir"
  export AGIT_AGENT="agent-$a"
  for i in $(seq 1 "$COMMITS"); do
    printf '// offline %s %s\n' "$i" "$(now)" > "src/mod$((i % 25))/file$(( (a * 11 + i) % FILES + 1 )).rs"
    "$AGIT" add -A >/dev/null
    "$AGIT" commit -m "offline $i" --intent "load test phase 2" >/dev/null
    "$AGIT" tag "p2-$a-$i" >/dev/null 2>&1 || true
  done
}
for a in $(seq 1 "$AGENTS"); do prep_loop "$a" & done
wait

push_loop() {
  local a="$1" dir="$WORK/agent-$1"
  cd "$dir"
  # push the branch forward one commit at a time, oldest first
  for i in $(seq 1 "$COMMITS"); do
    id=$("$AGIT" rev-parse "p2-$a-$i" --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["result"]["id"])')
    "$AGIT" branch -f "push/$a" "$id" >/dev/null 2>&1 || "$AGIT" branch "push/$a" "$id" >/dev/null 2>&1 || true
    "$AGIT" push origin "push/$a" >/dev/null 2>&1 || true
  done
}
t2=$(now)
for a in $(seq 1 "$AGENTS"); do push_loop "$a" & done
wait
echo "phase 2  push only:     $total pushes in $(elapsed "$t2")s  =  $(rate "$total" "$t2") pushes/s aggregate"

# --- phase 3: read path -------------------------------------------------------
t3=$(now)
reads=0
for a in $(seq 1 "$AGENTS"); do
  ( cd "$WORK/agent-$a"; for i in $(seq 1 20); do "$AGIT" fetch >/dev/null 2>&1; done ) &
done
wait
reads=$((AGENTS * 20))
echo "phase 3  fetch (no-op):  $reads fetches in $(elapsed "$t3")s  =  $(rate "$reads" "$t3") fetches/s aggregate"
echo
echo "branches on the server: load/$RUN/agent-1..$AGENTS and push/1..$AGENTS in repo $REPO"
