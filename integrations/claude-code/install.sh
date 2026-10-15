#!/usr/bin/env bash
# Install (or remove) the agit integration for Claude Code.
#
#   integrations/claude-code/install.sh [--repo <dir>] [--agent <id>] [--mcp]
#   integrations/claude-code/install.sh --global [--agent <id>]
#   integrations/claude-code/install.sh --uninstall [--global] [--repo <dir>]
#
# Project mode (default) writes into <repo>/.claude/hooks/agit/, merges the
# hooks into <repo>/.claude/settings.json and appends the agit section to
# <repo>/CLAUDE.md. Global mode uses ~/.claude/ instead. Both are idempotent:
# running the installer twice changes nothing, existing hooks and settings
# are preserved, and --uninstall removes exactly what was installed.
# Requires python3 for the JSON merge.

set -u

SRC_DIR=$(cd "$(dirname "$0")" && pwd)
MODE=install
SCOPE=project
REPO="${CLAUDE_PROJECT_DIR:-$PWD}"
AGENT=claude-code
WRITE_MCP=0
HOOK_FILES="lib.sh pre-edit.sh post-edit.sh checkpoint.sh session-start.sh pre-commit-guard.sh"

usage() {
    sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'
}

while [ $# -gt 0 ]; do
    case "$1" in
        --global) SCOPE=global ;;
        --uninstall) MODE=uninstall ;;
        --agent) shift; AGENT="${1:-}" ;;
        --agent=*) AGENT="${1#--agent=}" ;;
        --repo) shift; REPO="${1:-}" ;;
        --repo=*) REPO="${1#--repo=}" ;;
        --mcp) WRITE_MCP=1 ;;
        -h | --help) usage; exit 0 ;;
        *) echo "unknown option: $1" >&2; usage >&2; exit 1 ;;
    esac
    shift
done
[ -n "$AGENT" ] || { echo "--agent needs a value" >&2; exit 1; }

if ! command -v python3 >/dev/null 2>&1; then
    echo "python3 is required to merge settings.json; install it or merge $SRC_DIR/settings.json by hand." >&2
    exit 1
fi

if [ "$SCOPE" = global ]; then
    BASE="${HOME:?}/.claude"
    HOOKS_DIR="$BASE/hooks/agit"
    SETTINGS="$BASE/settings.json"
    CLAUDE_MD="$BASE/CLAUDE.md"
    PREFIX='"$HOME"/.claude/hooks/agit'
    MCP_SCOPE=user
else
    REPO=$(cd "$REPO" 2>/dev/null && pwd) || { echo "no such directory: $REPO" >&2; exit 1; }
    BASE="$REPO/.claude"
    HOOKS_DIR="$BASE/hooks/agit"
    SETTINGS="$BASE/settings.json"
    CLAUDE_MD="$REPO/CLAUDE.md"
    PREFIX='"$CLAUDE_PROJECT_DIR"/.claude/hooks/agit'
    MCP_SCOPE=project
    if [ ! -d "$REPO/.agit" ] && [ ! -f "$REPO/.agit" ] && [ "$MODE" = install ]; then
        echo "note: $REPO is not an agit repository yet (no .agit). The hooks stay inactive until you run \`agit init\` there." >&2
    fi
fi

# --- hook scripts -----------------------------------------------------------
if [ "$MODE" = install ]; then
    mkdir -p "$HOOKS_DIR" || exit 1
    for f in $HOOK_FILES; do
        cp "$SRC_DIR/hooks/$f" "$HOOKS_DIR/$f" || exit 1
        case "$f" in
            lib.sh) chmod 644 "$HOOKS_DIR/$f" ;;
            *) chmod 755 "$HOOKS_DIR/$f" ;;
        esac
    done
    echo "hooks:     $HOOKS_DIR (pre-edit, post-edit, checkpoint, session-start, pre-commit-guard)"
else
    if [ -d "${HOOKS_DIR:?}" ]; then
        for f in $HOOK_FILES; do
            [ -f "${HOOKS_DIR:?}/$f" ] && rm -f "${HOOKS_DIR:?}/$f"
        done
        rmdir "$HOOKS_DIR" 2>/dev/null
        echo "removed:   $HOOKS_DIR"
    fi
fi

# --- settings.json merge ----------------------------------------------------
mkdir -p "$(dirname "$SETTINGS")" || exit 1
python3 -I - "$SETTINGS" "$SRC_DIR/settings.json" "$MODE" "$AGENT" "$PREFIX" <<'PY' || exit 1
import json, os, sys, tempfile

settings_path, template_path, mode, agent, prefix = sys.argv[1:6]
MARK = "/hooks/agit/"

def load(path):
    if not os.path.exists(path):
        return {}
    with open(path) as f:
        text = f.read().strip()
    if not text:
        return {}
    try:
        return json.loads(text)
    except ValueError as e:
        sys.exit("%s is not valid JSON (%s); fix it or move it away first" % (path, e))

settings = load(settings_path)
with open(template_path) as f:
    template = json.load(f)

def is_ours(entry):
    return any(MARK in (h.get("command") or "") for h in entry.get("hooks", []))

hooks = settings.get("hooks") or {}
for event in list(hooks):
    kept = [e for e in hooks[event] if not is_ours(e)]
    if kept:
        hooks[event] = kept
    else:
        del hooks[event]

if mode == "install":
    for event, entries in template["hooks"].items():
        for entry in entries:
            entry = json.loads(json.dumps(entry))
            for h in entry["hooks"]:
                h["command"] = h["command"].replace('"$CLAUDE_PROJECT_DIR"/.claude/hooks/agit', prefix)
            hooks.setdefault(event, []).append(entry)
    env = settings.setdefault("env", {})
    env.setdefault("AGIT_AGENT", agent)
else:
    env = settings.get("env") or {}
    if env.get("AGIT_AGENT") == agent:
        del env["AGIT_AGENT"]
    if not env and "env" in settings:
        del settings["env"]

if hooks:
    settings["hooks"] = hooks
elif "hooks" in settings:
    del settings["hooks"]

if os.path.exists(settings_path):
    backup = settings_path + ".bak-agit"
    if not os.path.exists(backup):
        with open(settings_path) as src, open(backup, "w") as dst:
            dst.write(src.read())

fd, tmp = tempfile.mkstemp(dir=os.path.dirname(settings_path) or ".", prefix=".settings-", suffix=".json")
with os.fdopen(fd, "w") as f:
    json.dump(settings, f, indent=2)
    f.write("\n")
os.replace(tmp, settings_path)
n = sum(1 for es in hooks.values() for e in es if is_ours(e))
extra = ", env.AGIT_AGENT=%s" % settings.get("env", {}).get("AGIT_AGENT") if mode == "install" else ""
print("settings:  %s (%d agit hook entries%s)" % (settings_path, n, extra))
PY

# --- CLAUDE.md section -------------------------------------------------------
python3 -I - "$CLAUDE_MD" "$SRC_DIR/CLAUDE.md.snippet" "$MODE" <<'PY' || exit 1
import os, sys
path, snippet_path, mode = sys.argv[1:4]
BEGIN, END = "<!-- agit-claude-code:begin -->", "<!-- agit-claude-code:end -->"
with open(snippet_path) as f:
    snippet = f.read().rstrip("\n") + "\n"
text = ""
if os.path.exists(path):
    with open(path) as f:
        text = f.read()
has = BEGIN in text and END in text
if mode == "install":
    if has:
        print("CLAUDE.md: %s (agit section already present)" % path)
        sys.exit(0)
    sep = "" if (not text or text.endswith("\n\n")) else ("\n" if text.endswith("\n") else "\n\n")
    with open(path, "a") as f:
        f.write(sep + snippet)
    print("CLAUDE.md: %s (agit section appended)" % path)
else:
    if not has:
        sys.exit(0)
    start = text.index(BEGIN)
    end = text.index(END) + len(END)
    before, after = text[:start], text[end:]
    new = (before.rstrip("\n") + "\n" + after.lstrip("\n")) if before.strip() else after.lstrip("\n")
    with open(path, "w") as f:
        f.write(new)
    print("CLAUDE.md: %s (agit section removed)" % path)
PY

# --- MCP server -------------------------------------------------------------
if [ "$MODE" = install ]; then
    if [ "$WRITE_MCP" = 1 ] && [ "$SCOPE" = project ]; then
        python3 -I - "$REPO/.mcp.json" "$AGENT" <<'PY' || exit 1
import json, os, sys
path, agent = sys.argv[1:3]
cfg = {}
if os.path.exists(path):
    with open(path) as f:
        cfg = json.load(f) or {}
servers = cfg.setdefault("mcpServers", {})
servers.setdefault("agit", {"command": "agit", "args": ["mcp"], "env": {"AGIT_AGENT": agent}})
with open(path, "w") as f:
    json.dump(cfg, f, indent=2)
    f.write("\n")
print("mcp:       %s (server 'agit')" % path)
PY
    else
        echo
        echo "Register the MCP server (once):"
        echo "  claude mcp add --scope $MCP_SCOPE agit -e AGIT_AGENT=$AGENT -- agit mcp"
        [ "$SCOPE" = project ] && echo "  (or re-run with --mcp to write $REPO/.mcp.json)"
    fi
    echo
    echo "Done. Start claude in the repository; the SessionStart hook prints the agit briefing."
    echo "Read docs/CLAUDE-CODE.md for what each hook does. Set AGIT_HOOKS_DISABLE=1 to pause the hooks."
else
    echo "Done. MCP registrations are not touched; remove with: claude mcp remove agit"
fi
