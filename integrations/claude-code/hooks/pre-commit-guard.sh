#!/usr/bin/env bash
# PreToolUse hook for Bash.
#
# Inside an agit repository:
#   * `agit commit` without `--intent` is blocked (exit 2) with a message
#     asking for the intent (and ideally `--task`);
#   * `git commit` / `git push` produce a reminder (additionalContext, not a
#     block) to use the agit equivalents;
#   * `agit pr create` without `--intent` produces a reminder.
# Works without the agit binary: it only needs to know the directory is an
# agit repository.

HOOK_DIR=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib.sh
. "$HOOK_DIR/lib.sh"

hook_disabled && exit 0
hook_read_input
cmd=$(json_get "$HOOK_INPUT" tool_input.command)
[ -n "$cmd" ] || exit 0
root=$(agit_find_root "$(hook_cwd)") || exit 0

# Collapse newlines so multi-line commands match too.
flat=$(printf '%s' "$cmd" | tr '\n' ' ')

# `agit [global opts] commit ...` — global options may precede the subcommand.
re_agit_commit='(^|[^[:alnum:]_/.-])agit([[:space:]]+(-C[[:space:]]*[^[:space:]]+|--json|--agent[[:space:]]*[^[:space:]]+))*[[:space:]]+commit([[:space:]]|$)'
re_agit_pr_create='(^|[^[:alnum:]_/.-])agit([[:space:]]+(-C[[:space:]]*[^[:space:]]+|--json))*[[:space:]]+pr[[:space:]]+(--remote[[:space:]]*[^[:space:]]+[[:space:]]+)?create([[:space:]]|$)'
re_git='(^|[^[:alnum:]_/.-])git([[:space:]]+(-C[[:space:]]*[^[:space:]]+|-c[[:space:]]*[^[:space:]]+|--no-pager|--git-dir=[^[:space:]]+|--work-tree=[^[:space:]]+))*[[:space:]]+(commit|push)([[:space:]]|$)'
re_intent='(^|[[:space:]])--intent([[:space:]]|=)'
re_help='(^|[[:space:]])(-h|--help)([[:space:]]|$)'

if [[ $flat =~ $re_agit_commit ]] && ! [[ $flat =~ $re_help ]]; then
    if ! [[ $flat =~ $re_intent ]]; then
        printf 'agit blocked this command: `agit commit` needs `--intent "<why this change exists>"` (and `--task <id>` when you work on a ticket). The message says what changed, the intent says why; server policies on protected branches reject commits without it. Re-run as: agit commit -m "<what>" --intent "<why>" [--task <id>]\n' >&2
        exit 2
    fi
    exit 0
fi

if [[ $flat =~ $re_agit_pr_create ]] && ! [[ $flat =~ $re_help ]] && ! [[ $flat =~ $re_intent ]]; then
    emit_context PreToolUse 'agit: give the pull request an intent: `agit pr create --push -t "<title>" --intent "<why>" [--task <id>]`. Policies with require_intent reject merges without it.'
    exit 0
fi

if [[ $flat =~ $re_git ]]; then
    emit_context PreToolUse "agit: $root is an agit repository. Use agit instead of git here: \`agit add -A\`, \`agit commit -m \"<what>\" --intent \"<why>\" [--task <id>]\`, \`agit push\` or \`agit pr create --push -t \"...\" --intent \"...\"\`. A git commit would not be seen by other agents, leases or the agit server."
    exit 0
fi
exit 0
