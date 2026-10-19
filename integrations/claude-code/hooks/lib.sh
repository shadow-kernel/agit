# shellcheck shell=bash
# Shared helpers for the agit Claude Code hooks. Sourced, not executed.
#
# Design rules (every hook follows them):
#   * never break Claude Code: no `set -e`, every external call is guarded,
#     and a hook that has nothing to say exits 0 without output;
#   * no-op fast when agit is not installed or the path is not inside an
#     agit repository (detected by walking up to a `.agit` directory or a
#     `.agit` workspace pointer file, no process spawned);
#   * bash 3.2 compatible (macOS /bin/bash); JSON is handled by jq when
#     present, otherwise by `python3 -I`.

AGIT_BIN="${AGIT_BIN:-agit}"
AGIT_AGENT="${AGIT_AGENT:-claude-code}"
export AGIT_AGENT
HOOK_CACHE_DIR="${TMPDIR:-/tmp}"
HOOK_CACHE_DIR="${HOOK_CACHE_DIR%/}/agit-claude-hooks"

# Read the whole hook payload from stdin into HOOK_INPUT.
hook_read_input() {
    HOOK_INPUT=$(cat 2>/dev/null || true)
}

# True when the hooks are globally disabled.
hook_disabled() {
    [ "${AGIT_HOOKS_DISABLE:-0}" = "1" ]
}

# True when the agit binary can be executed.
agit_available() {
    command -v "$AGIT_BIN" >/dev/null 2>&1
}

have_jq() { command -v jq >/dev/null 2>&1; }
have_py() { command -v python3 >/dev/null 2>&1; }

# json_get <json> <dotted.path>  -> prints the scalar value or nothing.
json_get() {
    local json="$1" path="$2"
    [ -n "$json" ] || return 0
    if have_jq; then
        printf '%s' "$json" | jq -r ".$path // empty" 2>/dev/null
    elif have_py; then
        printf '%s' "$json" | python3 -I -c '
import json, sys
try:
    cur = json.load(sys.stdin)
except Exception:
    sys.exit(0)
for key in sys.argv[1].split("."):
    if isinstance(cur, dict) and key in cur:
        cur = cur[key]
    else:
        sys.exit(0)
if cur is None or isinstance(cur, (dict, list)):
    sys.exit(0)
print(cur if not isinstance(cur, bool) else str(cur).lower())
' "$path" 2>/dev/null
    fi
}

# Walk up from <dir> until a directory containing an agit repository is found:
# a `.agit` directory with HEAD and objects/, or a workspace pointer file.
# A bare `~/.agit` (signing keys, sessions) is not a repository. Prints the
# repository root or nothing.
agit_find_root() {
    local dir="$1"
    case "$dir" in
        /*) ;;
        *) dir="$PWD/$dir" ;;
    esac
    while :; do
        if [ -f "$dir/.agit" ] || { [ -f "$dir/.agit/HEAD" ] && [ -d "$dir/.agit/objects" ]; }; then
            printf '%s\n' "$dir"
            return 0
        fi
        case "$dir" in
            / | "") return 1 ;;
        esac
        dir=$(dirname "$dir")
    done
}

# Normalise a possibly relative tool path against the hook's cwd.
hook_abs_path() {
    local p="$1" base="$2"
    case "$p" in
        "") return 1 ;;
        /*) ;;
        *) p="${base:-$PWD}/$p" ;;
    esac
    # collapse "./" and "a/../" segments textually (the file may not exist yet)
    if have_py; then
        python3 -I -c 'import os, sys; print(os.path.normpath(sys.argv[1]))' "$p" 2>/dev/null || printf '%s\n' "$p"
    else
        printf '%s\n' "$p"
    fi
}

# Path of the file being edited, taken from tool_input (Edit/Write/MultiEdit
# use file_path, NotebookEdit uses notebook_path). Prints the absolute path.
hook_target_path() {
    local p cwd
    cwd=$(json_get "$HOOK_INPUT" cwd)
    p=$(json_get "$HOOK_INPUT" tool_input.file_path)
    [ -n "$p" ] || p=$(json_get "$HOOK_INPUT" tool_input.notebook_path)
    [ -n "$p" ] || return 1
    hook_abs_path "$p" "$cwd"
}

# Directory Claude Code is working in (hook payload cwd, then project dir).
hook_cwd() {
    local c
    c=$(json_get "$HOOK_INPUT" cwd)
    [ -n "$c" ] || c="${CLAUDE_PROJECT_DIR:-$PWD}"
    printf '%s\n' "$c"
}

# run_agit <root> <args...>: run agit in JSON mode rooted at <root>.
# stdout: the JSON envelope; exit code: agit's. Pass repository-relative
# paths: agit canonicalises the work dir, so an absolute path that goes
# through a symlink (macOS /var -> /private/var) would not resolve.
run_agit() {
    local root="$1"
    shift
    "$AGIT_BIN" -C "$root" --json "$@" 2>/dev/null
}

# Emit hookSpecificOutput with additionalContext for <event>.
emit_context() {
    local event="$1" text="$2"
    [ -n "$text" ] || return 0
    if have_jq; then
        jq -cn --arg e "$event" --arg t "$text" \
            '{hookSpecificOutput:{hookEventName:$e, additionalContext:$t}}'
    elif have_py; then
        python3 -I -c 'import json, sys; print(json.dumps({"hookSpecificOutput": {"hookEventName": sys.argv[1], "additionalContext": sys.argv[2]}}))' "$event" "$text"
    fi
}

# Stable short key for cache file names.
cache_key() {
    printf '%s' "$*" | cksum | cut -d' ' -f1
}

cache_dir_ready() {
    [ -d "$HOOK_CACHE_DIR" ] || mkdir -p "$HOOK_CACHE_DIR" 2>/dev/null
    chmod 700 "$HOOK_CACHE_DIR" 2>/dev/null
    [ -d "$HOOK_CACHE_DIR" ]
}

# cache_fresh <file> <max-age-minutes>: true when the file exists and is
# younger than the given number of minutes (find -mmin is portable).
cache_fresh() {
    local f="$1" mins="${2:-1}"
    [ -f "$f" ] || return 1
    [ -n "$(find "$f" -mmin "-$mins" -print 2>/dev/null)" ]
}

# Truncate text to <n> characters with an ellipsis.
truncate_text() {
    local text="$1" n="${2:-800}"
    if [ "${#text}" -gt "$n" ]; then
        printf '%s...' "${text:0:$((n - 3))}"
    else
        printf '%s' "$text"
    fi
}

# Repository-relative path of <abs> under <root>.
rel_path() {
    local abs="$1" root="$2"
    printf '%s\n' "${abs#"$root"/}"
}

# Short id helper.
short_id() { printf '%s' "${1:0:8}"; }
