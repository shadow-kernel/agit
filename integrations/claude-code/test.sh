#!/usr/bin/env bash
# Tests for the agit Claude Code hooks and installer. No server needed.
#
#   integrations/claude-code/test.sh            # uses agit from PATH or target/release
#   AGIT_BIN=/path/to/agit integrations/claude-code/test.sh
#
# Each hook is fed the JSON Claude Code would send on stdin; exit codes and
# output are asserted. Everything happens in a fresh temporary directory
# (repository, hook cache, fake HOME).

set -u

HERE=$(cd "$(dirname "$0")" && pwd)
HOOKS="$HERE/hooks"
REPO_ROOT=$(cd "$HERE/../.." && pwd)

if [ -z "${AGIT_BIN:-}" ]; then
    if command -v agit >/dev/null 2>&1; then
        AGIT_BIN=$(command -v agit)
    elif [ -x "$REPO_ROOT/target/release/agit" ]; then
        AGIT_BIN="$REPO_ROOT/target/release/agit"
    else
        echo "agit binary not found; set AGIT_BIN or run cargo build --release" >&2
        exit 1
    fi
fi
export AGIT_BIN

WORK=$(mktemp -d "${TMPDIR:-/tmp}/agit-cc-test.XXXXXX") || exit 1
trap 'rm -rf "${WORK:?}"' EXIT
export TMPDIR="$WORK/tmp"
mkdir -p "$TMPDIR"
export AGIT_AGENT=claude-code
unset AGIT_HOOKS_DISABLE CLAUDE_PROJECT_DIR

PASS=0
FAIL=0
LAST_OUT=""
LAST_ERR=""
LAST_RC=0

# run_hook <script> <json>: capture stdout, stderr and exit code.
run_hook() {
    local script="$1" json="$2"
    LAST_OUT=$(printf '%s' "$json" | "$HOOKS/$script" 2>"$WORK/stderr")
    LAST_RC=$?
    LAST_ERR=$(cat "$WORK/stderr")
}

ok() { PASS=$((PASS + 1)); echo "ok   - $1"; }
fail() {
    FAIL=$((FAIL + 1))
    echo "FAIL - $1"
    [ -n "${2:-}" ] && printf '       %s\n' "$2"
}

assert_rc() { # <name> <expected>
    if [ "$LAST_RC" -eq "$2" ]; then ok "$1 (exit $2)"; else fail "$1: expected exit $2, got $LAST_RC" "stdout: $LAST_OUT | stderr: $LAST_ERR"; fi
}
assert_contains() { # <name> <haystack> <needle>
    case "$2" in *"$3"*) ok "$1" ;; *) fail "$1: missing '$3'" "got: $2" ;; esac
}
assert_empty() { # <name> <value>
    if [ -z "$2" ]; then ok "$1"; else fail "$1: expected no output" "got: $2"; fi
}
count_json() { # <json> <python expr over r=result>
    printf '%s' "$1" | python3 -I -c 'import json,sys; r=json.load(sys.stdin)["result"]; print(eval(sys.argv[1]))' "$2"
}

edit_json() { # <cwd> <tool> <file_path>
    printf '{"session_id":"s1","cwd":"%s","hook_event_name":"PreToolUse","tool_name":"%s","tool_input":{"file_path":"%s","content":"x"}}' "$1" "$2" "$3"
}
bash_json() { # <cwd> <command>
    python3 -I -c 'import json,sys; print(json.dumps({"session_id":"s1","cwd":sys.argv[1],"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":sys.argv[2]}}))' "$1" "$2"
}

# --- fixture ---------------------------------------------------------------
R="$WORK/repo"
mkdir -p "$R/src" "$R/docs"
"$AGIT_BIN" -C "$R" init . --json >/dev/null || { echo "agit init failed" >&2; exit 1; }
echo 'fn main() {}' > "$R/src/main.rs"
echo '# notes' > "$R/docs/notes.md"
"$AGIT_BIN" -C "$R" add -A --json >/dev/null
"$AGIT_BIN" -C "$R" commit -m "init" --intent "fixture" --json >/dev/null
OUTSIDE="$WORK/outside"
mkdir -p "$OUTSIDE"
# A per-user ~/.agit (keys, sessions) above the outside directory must not
# make it look like a repository.
mkdir -p "$WORK/.agit/keys" && echo '{}' > "$WORK/.agit/sessions.json"
# agit 0.1.0 could also leave a config.json there; still not a repository.
echo '{"format_version":1,"default_branch":"main","remotes":{}}' > "$WORK/.agit/config.json"

echo "# hooks: $HOOKS"
echo "# agit:  $AGIT_BIN ($("$AGIT_BIN" --version))"
echo "# repo:  $R"

# --- no-op cases -------------------------------------------------------------
echo "## no-op outside a repository / without agit"
run_hook pre-edit.sh "$(edit_json "$OUTSIDE" Edit "$OUTSIDE/x.txt")"
assert_rc "pre-edit outside repo" 0; assert_empty "pre-edit outside repo prints nothing" "$LAST_OUT$LAST_ERR"
run_hook post-edit.sh "$(edit_json "$OUTSIDE" Edit "$OUTSIDE/x.txt")"
assert_rc "post-edit outside repo" 0
run_hook checkpoint.sh "{\"cwd\":\"$OUTSIDE\"}"
assert_rc "checkpoint outside repo" 0
run_hook session-start.sh "{\"cwd\":\"$OUTSIDE\",\"source\":\"startup\"}"
assert_rc "session-start outside repo" 0; assert_empty "session-start outside repo prints nothing" "$LAST_OUT"
run_hook pre-commit-guard.sh "$(bash_json "$OUTSIDE" 'git commit -m x')"
assert_rc "guard outside repo" 0; assert_empty "guard outside repo prints nothing" "$LAST_OUT"
run_hook pre-edit.sh '{"cwd":"/","tool_name":"Edit","tool_input":{}}'
assert_rc "pre-edit without file_path" 0
run_hook pre-edit.sh 'this is not json'
assert_rc "pre-edit with garbage stdin" 0
run_hook pre-edit.sh ''
assert_rc "pre-edit with empty stdin" 0

( export AGIT_BIN=/nonexistent/agit; export PATH=/usr/bin:/bin
  for s in pre-edit.sh post-edit.sh checkpoint.sh; do
      printf '%s' "$(edit_json "$R" Edit "$R/src/main.rs")" | "$HOOKS/$s" >/dev/null 2>&1
      rc=$?
      [ $rc -eq 0 ] && echo "ok   - $s without agit binary (exit 0)" || echo "FAIL - $s without agit binary (exit $rc)"
  done
  printf '{"cwd":"%s"}' "$R" | "$HOOKS/session-start.sh" > "$WORK/ss-noagit" 2>/dev/null
  rc=$?
  if [ $rc -eq 0 ] && grep -q "not on PATH" "$WORK/ss-noagit"; then echo "ok   - session-start without agit binary explains it"; else echo "FAIL - session-start without agit binary"; fi
) | tee "$WORK/noagit.log"
PASS=$((PASS + $(grep -c '^ok' "$WORK/noagit.log"))); FAIL=$((FAIL + $(grep -c '^FAIL' "$WORK/noagit.log")))

run_hook pre-edit.sh "$(edit_json "$R" Edit "$R/.agit/config")"
assert_rc "pre-edit ignores .agit internals" 0; assert_empty "pre-edit ignores .agit internals (no output)" "$LAST_OUT"

AGIT_HOOKS_DISABLE=1 run_hook pre-commit-guard.sh "$(bash_json "$R" 'agit commit -m x')"
assert_rc "AGIT_HOOKS_DISABLE=1 turns hooks off" 0

# --- pre-edit: allowed -----------------------------------------------------
echo "## pre-edit"
run_hook pre-edit.sh "$(edit_json "$R" Edit "$R/src/main.rs")"
assert_rc "edit allowed without leases (no remote)" 0
assert_empty "nothing relevant -> no output" "$LAST_OUT"

# relative file_path resolved against cwd
run_hook pre-edit.sh '{"cwd":"'"$R"'","tool_name":"Write","tool_input":{"file_path":"src/new_file.rs"}}'
assert_rc "relative file_path (new file) allowed" 0

# context call with a dead remote degrades gracefully and is cached
"$AGIT_BIN" -C "$R" remote add origin http://127.0.0.1:1/api/v1/repos/x --json >/dev/null
run_hook pre-edit.sh "$(edit_json "$R" Edit "$R/src/main.rs")"
assert_rc "dead remote: edit still allowed" 0
assert_empty "dead remote: no output" "$LAST_OUT$LAST_ERR"
n=$(ls "$TMPDIR/agit-claude-hooks"/ctx-* 2>/dev/null | wc -l | tr -d ' ')
[ "$n" -ge 1 ] && ok "context result cached in TMPDIR" || fail "context cache file missing"

# a knowledge note about the path reaches Claude before the edit (server
# unreachable here, so it comes from the local store)
"$AGIT_BIN" -C "$R" know --local add -t "Totals are in minor units" --tag kind:gotcha --path src/main.rs -b "integer cents" --json >/dev/null
rm -rf "$TMPDIR/agit-claude-hooks"
run_hook pre-edit.sh "$(edit_json "$R" Edit "$R/src/main.rs")"
assert_rc "edit with a note about the path allowed" 0
assert_contains "note title reaches Claude as context" "$LAST_OUT" "Totals are in minor units"
assert_contains "note kind tag reaches Claude" "$LAST_OUT" "kind:gotcha"
assert_contains "context is additionalContext JSON" "$LAST_OUT" "additionalContext"
rm -rf "$TMPDIR/agit-claude-hooks"

# --- pre-edit: blocked by a foreign lease ------------------------------------
"$AGIT_BIN" -C "$R" lease claim src --holder other-agent --ttl 10m --note "refactor" --json >/dev/null
run_hook pre-edit.sh "$(edit_json "$R" Edit "$R/src/main.rs")"
assert_rc "edit blocked by foreign exclusive lease" 2
assert_contains "block message names the holder" "$LAST_ERR" "held by other-agent"
assert_contains "block message shows expiry" "$LAST_ERR" "expires in about"
assert_empty "blocked edit prints no stdout" "$LAST_OUT"
run_hook pre-edit.sh "$(edit_json "$R" Write "$R/src/sub/brand_new.rs")"
assert_rc "new file under foreign lease is blocked too" 2
run_hook pre-edit.sh '{"cwd":"'"$R"'","tool_name":"NotebookEdit","tool_input":{"notebook_path":"'"$R"'/src/nb.ipynb"}}'
assert_rc "NotebookEdit notebook_path is honoured" 2

# shared lease by another agent: allowed, but mentioned
"$AGIT_BIN" -C "$R" lease claim docs --holder reviewer --ttl 10m --shared --json >/dev/null
run_hook pre-edit.sh "$(edit_json "$R" Edit "$R/docs/notes.md")"
assert_rc "shared foreign lease does not block" 0
assert_contains "shared lease reported as context" "$LAST_OUT" '"hookEventName":"PreToolUse"'
assert_contains "shared lease names the holder" "$LAST_OUT" "reviewer (shared"
len=$(printf '%s' "$LAST_OUT" | python3 -I -c 'import json,sys; print(len(json.load(sys.stdin)["hookSpecificOutput"]["additionalContext"]))')
[ "$len" -le 800 ] && ok "additionalContext under 800 chars ($len)" || fail "additionalContext too long ($len)"

# --- post-edit: lease claim ---------------------------------------------------
echo "## post-edit"
before=$(count_json "$("$AGIT_BIN" -C "$R" lease list --json)" 'len(r)')
run_hook post-edit.sh "$(edit_json "$R" Edit "$R/docs/notes.md")"
assert_rc "post-edit claims a lease" 0
assert_empty "post-edit is silent on success" "$LAST_OUT$LAST_ERR"
after=$(count_json "$("$AGIT_BIN" -C "$R" lease list --json)" 'len(r)')
[ "$after" -eq $((before + 1)) ] && ok "exactly one lease added" || fail "lease count $before -> $after"
mine=$(count_json "$("$AGIT_BIN" -C "$R" lease list --json)" '[l for l in r if l["holder"]=="claude-code" and l["paths"]==["docs/notes.md"] and l["mode"]=="exclusive" and l["note"]=="claude-code edit"]' )
assert_contains "lease has holder, path, mode and note" "$mine" "claude-code edit"
run_hook post-edit.sh "$(edit_json "$R" Edit "$R/docs/notes.md")"
again=$(count_json "$("$AGIT_BIN" -C "$R" lease list --json)" 'len(r)')
[ "$again" -eq "$after" ] && ok "second edit does not duplicate the lease" || fail "duplicate lease created ($after -> $again)"
run_hook pre-edit.sh "$(edit_json "$R" Edit "$R/docs/notes.md")"
assert_rc "pre-edit allowed on own lease" 0

# directory lease covers files beneath it
"$AGIT_BIN" -C "$R" lease claim lib --holder claude-code --ttl 10m --json >/dev/null
mkdir -p "$R/lib"
cnt=$(count_json "$("$AGIT_BIN" -C "$R" lease list --json)" 'len(r)')
run_hook post-edit.sh "$(edit_json "$R" Write "$R/lib/deep/file.rs")"
cnt2=$(count_json "$("$AGIT_BIN" -C "$R" lease list --json)" 'len(r)')
[ "$cnt2" -eq "$cnt" ] && ok "existing directory lease covers new file (no extra claim)" || fail "claimed under an existing directory lease"

# conflict after the edit: warning, not failure
run_hook post-edit.sh "$(edit_json "$R" Edit "$R/src/main.rs")"
assert_rc "post-edit under foreign lease exits 0" 0
assert_contains "post-edit under foreign lease warns" "$LAST_OUT" "lease_conflict"

# --- checkpoint on Stop -------------------------------------------------------
echo "## checkpoint"
cps() { count_json "$("$AGIT_BIN" -C "$R" checkpoint list --json)" 'len(r)'; }
c0=$(cps)
echo 'fn main() { println!("hi"); }' > "$R/src/main.rs"
run_hook checkpoint.sh "{\"session_id\":\"s1\",\"cwd\":\"$R\",\"hook_event_name\":\"Stop\",\"stop_hook_active\":false}"
assert_rc "stop hook with dirty tree" 0
assert_empty "stop hook is silent" "$LAST_OUT$LAST_ERR"
c1=$(cps)
[ "$c1" -eq $((c0 + 1)) ] && ok "checkpoint recorded for dirty tree" || fail "checkpoint count $c0 -> $c1"
msg=$(count_json "$("$AGIT_BIN" -C "$R" checkpoint list --json)" 'r[0]["message"]')
assert_contains "checkpoint message" "$msg" "claude-code stop"
run_hook checkpoint.sh "{\"cwd\":\"$R\"}"
c2=$(cps)
[ "$c2" -eq "$c1" ] && ok "unchanged tree is not checkpointed again" || fail "duplicate checkpoint ($c1 -> $c2)"
echo 'untracked' > "$R/scratch.txt"
run_hook checkpoint.sh "{\"cwd\":\"$R/src\"}"
c3=$(cps)
[ "$c3" -eq $((c2 + 1)) ] && ok "new untracked file triggers a checkpoint (cwd in subdir)" || fail "untracked change missed ($c2 -> $c3)"
echo 'untracked v2' > "$R/scratch.txt"
run_hook checkpoint.sh "{\"cwd\":\"$R\"}"
c4=$(cps)
[ "$c4" -eq $((c3 + 1)) ] && ok "edited untracked file triggers a checkpoint" || fail "untracked edit missed ($c3 -> $c4)"
"$AGIT_BIN" -C "$R" add -A --json >/dev/null
"$AGIT_BIN" -C "$R" commit -m "wip" --intent "test" --force --json >/dev/null
run_hook checkpoint.sh "{\"cwd\":\"$R\"}"
c5=$(cps)
[ "$c5" -eq "$c4" ] && ok "clean tree is not checkpointed" || fail "checkpoint on clean tree ($c4 -> $c5)"

# --- session start ------------------------------------------------------------
echo "## session-start"
run_hook session-start.sh "{\"session_id\":\"s1\",\"cwd\":\"$R\",\"hook_event_name\":\"SessionStart\",\"source\":\"startup\"}"
assert_rc "session-start in repo" 0
assert_contains "session-start event name" "$LAST_OUT" '"hookEventName":"SessionStart"'
ctx=$(printf '%s' "$LAST_OUT" | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["hookSpecificOutput"]["additionalContext"])')
assert_contains "briefing has branch" "$ctx" "branch main"
assert_contains "briefing has agent id" "$ctx" "agent \`claude-code\`"
assert_contains "briefing lists foreign lease" "$ctx" "other-agent: src"
assert_contains "briefing lists own lease as you" "$ctx" "you: docs/notes.md"
assert_contains "briefing has the loop" "$ctx" "--intent"
assert_contains "briefing has remote" "$ctx" "remote origin"

# --- bash guard ---------------------------------------------------------------
echo "## pre-commit-guard"
run_hook pre-commit-guard.sh "$(bash_json "$R" 'git commit -m "x"')"
assert_rc "git commit: not blocked" 0
assert_contains "git commit: reminder" "$LAST_OUT" "Use agit instead of git"
run_hook pre-commit-guard.sh "$(bash_json "$R" 'cd sub && git push origin main')"
assert_rc "git push: not blocked" 0; assert_contains "git push: reminder" "$LAST_OUT" "agit repository"
run_hook pre-commit-guard.sh "$(bash_json "$R" 'agit commit -m "x"')"
assert_rc "agit commit without --intent is blocked" 2
assert_contains "block asks for --intent" "$LAST_ERR" "--intent"
run_hook pre-commit-guard.sh "$(bash_json "$R" 'agit add -A && agit --json commit -m "x" --task T-1')"
assert_rc "agit commit with global flags, no intent: blocked" 2
run_hook pre-commit-guard.sh "$(bash_json "$R" 'agit commit -m "x" --intent "because"')"
assert_rc "agit commit with --intent allowed" 0; assert_empty "no output when compliant" "$LAST_OUT"
run_hook pre-commit-guard.sh "$(bash_json "$R" 'agit commit -m "x" --intent="because"')"
assert_rc "agit commit with --intent=value allowed" 0
run_hook pre-commit-guard.sh "$(bash_json "$R" 'agit commit --help')"
assert_rc "agit commit --help allowed" 0
run_hook pre-commit-guard.sh "$(bash_json "$R" 'agit checkpoint -m "x"')"
assert_rc "agit checkpoint untouched" 0; assert_empty "checkpoint: no output" "$LAST_OUT"
run_hook pre-commit-guard.sh "$(bash_json "$R" 'echo "digit commit"; ls agit-commit-log')"
assert_rc "no false positive on substrings" 0; assert_empty "substrings: no output" "$LAST_OUT"
run_hook pre-commit-guard.sh "$(bash_json "$R" 'agit pr create -t "x"')"
assert_rc "pr create without intent: reminder only" 0; assert_contains "pr create reminder" "$LAST_OUT" "--intent"
run_hook pre-commit-guard.sh "$(bash_json "$R" "$(printf 'agit add -A\nagit commit -m x')")"
assert_rc "multi-line command is inspected" 2

# --- installer ------------------------------------------------------------------
echo "## install.sh"
P="$WORK/project"
mkdir -p "$P/.claude"
"$AGIT_BIN" -C "$P" init . --json >/dev/null
cat > "$P/.claude/settings.json" <<'EOF'
{
  "permissions": {"allow": ["Bash(ls:*)"]},
  "env": {"FOO": "bar"},
  "hooks": {
    "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "echo existing"}]}]
  }
}
EOF
printf '# My project\n\nSome notes.\n' > "$P/CLAUDE.md"
out=$("$HERE/install.sh" --repo "$P" --agent tester --mcp 2>&1); rc=$?
[ $rc -eq 0 ] && ok "install.sh exits 0" || fail "install.sh failed" "$out"
for f in lib.sh pre-edit.sh post-edit.sh checkpoint.sh session-start.sh pre-commit-guard.sh; do
    [ -f "$P/.claude/hooks/agit/$f" ] || fail "missing installed hook $f"
done
[ -x "$P/.claude/hooks/agit/pre-edit.sh" ] && ok "installed hooks are executable" || fail "installed hook not executable"
check=$(python3 -I - "$P/.claude/settings.json" <<'PY'
import json, sys
s = json.load(open(sys.argv[1]))
h = s["hooks"]
pre = h["PreToolUse"]
ours = [e for es in h.values() for e in es if any("/hooks/agit/" in x["command"] for x in e["hooks"])]
print("existing" if any(x["command"] == "echo existing" for e in pre for x in e["hooks"]) else "lost-existing")
print("ours=%d" % len(ours))
print("env=%s,%s" % (s["env"].get("FOO"), s["env"].get("AGIT_AGENT")))
print("perm=%s" % s["permissions"]["allow"][0])
print("events=%s" % ",".join(sorted(h)))
PY
)
assert_contains "existing hook preserved" "$check" "existing"
assert_contains "six agit entries installed" "$check" "ours=6"
assert_contains "env merged, agent set" "$check" "env=bar,tester"
assert_contains "permissions preserved" "$check" "perm=Bash(ls:*)"
assert_contains "all events present" "$check" "events=PostToolUse,PreToolUse,SessionStart,Stop,SubagentStop"
assert_contains "CLAUDE.md section appended" "$(cat "$P/CLAUDE.md")" "agit-claude-code:begin"
assert_contains "CLAUDE.md original kept" "$(cat "$P/CLAUDE.md")" "Some notes."
assert_contains ".mcp.json written" "$(cat "$P/.mcp.json")" '"agit"'
[ -f "$P/.claude/settings.json.bak-agit" ] && ok "backup of settings created" || fail "no settings backup"
snap1=$(cat "$P/.claude/settings.json" "$P/CLAUDE.md")
"$HERE/install.sh" --repo "$P" --agent tester --mcp >/dev/null 2>&1
snap2=$(cat "$P/.claude/settings.json" "$P/CLAUDE.md")
[ "$snap1" = "$snap2" ] && ok "second install is a no-op (idempotent)" || fail "second install changed files"
# installed copy works end to end with $CLAUDE_PROJECT_DIR
cmd=$(python3 -I -c 'import json,sys; s=json.load(open(sys.argv[1])); print([x["command"] for e in s["hooks"]["PreToolUse"] for x in e["hooks"] if "pre-commit-guard" in x["command"]][0])' "$P/.claude/settings.json")
printf '%s' "$(bash_json "$P" 'agit commit -m x')" | CLAUDE_PROJECT_DIR="$P" bash -c "$cmd" >/dev/null 2>&1
[ $? -eq 2 ] && ok "installed hook runs via \$CLAUDE_PROJECT_DIR command" || fail "installed hook command did not run"
out=$("$HERE/install.sh" --repo "$P" --agent tester --uninstall 2>&1); rc=$?
[ $rc -eq 0 ] && ok "uninstall exits 0" || fail "uninstall failed" "$out"
[ ! -d "$P/.claude/hooks/agit" ] && ok "hooks directory removed" || fail "hooks directory still there"
check=$(python3 -I -c 'import json,sys; s=json.load(open(sys.argv[1])); h=s.get("hooks",{}); print(sorted(h), s.get("env"), any("/hooks/agit/" in x["command"] for es in h.values() for e in es for x in e["hooks"]))' "$P/.claude/settings.json")
assert_contains "uninstall keeps existing hook and env" "$check" "['PreToolUse'] {'FOO': 'bar'} False"
case "$(cat "$P/CLAUDE.md")" in *agit-claude-code*) fail "CLAUDE.md section not removed" ;; *) ok "CLAUDE.md section removed" ;; esac
assert_contains "CLAUDE.md original intact after uninstall" "$(cat "$P/CLAUDE.md")" "Some notes."

# global mode into a fake HOME
FAKE_HOME="$WORK/home"
mkdir -p "$FAKE_HOME"
HOME="$FAKE_HOME" "$HERE/install.sh" --global --agent g1 >/dev/null 2>&1 && ok "global install exits 0" || fail "global install failed"
[ -f "$FAKE_HOME/.claude/hooks/agit/pre-edit.sh" ] && ok "global hooks in ~/.claude/hooks/agit" || fail "global hooks missing"
assert_contains "global settings use \$HOME" "$(cat "$FAKE_HOME/.claude/settings.json")" '\"$HOME\"/.claude/hooks/agit/pre-edit.sh'
assert_contains "global CLAUDE.md written" "$(cat "$FAKE_HOME/.claude/CLAUDE.md")" "agit-claude-code:begin"
HOME="$FAKE_HOME" "$HERE/install.sh" --global --agent g1 --uninstall >/dev/null 2>&1
[ ! -d "$FAKE_HOME/.claude/hooks/agit" ] && ok "global uninstall removes hooks" || fail "global uninstall left hooks"

echo
echo "passed: $PASS  failed: $FAIL"
[ "$FAIL" -eq 0 ]
