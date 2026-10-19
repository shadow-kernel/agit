# Claude Code integration

agit ships a first-class integration for [Claude Code](https://docs.anthropic.com/en/docs/claude-code) in `integrations/claude-code/`. It has two halves that work independently and are better together:

| Half | What it gives Claude | Relies on |
| --- | --- | --- |
| **MCP server** (`agit mcp`) | 32 typed tools (`agit_status`, `agit_context`, `agit_lease_claim`, `agit_commit`, `agit_pr_create`, `agit_know_add`, ...) with the same JSON and error codes as the CLI | Claude choosing to call them |
| **Hooks** (five bash scripts) | Lease checks that *block* edits, context injected before edits, automatic lease claims, checkpoints on every stop, a guard on `git commit` and on `agit commit` without `--intent` | nothing: Claude Code runs them deterministically |

The hooks exist because an agent's discipline is not a reliable control. A model that forgot to call `agit_context` still gets the notes; a model that never claims leases still ends up with one on every file it touched; a model that tries to edit a path another agent owns is stopped before the write happens.

## Setup

Requirements: `agit` on `PATH` (`cargo install --path crates/agit-cli` or a release binary), `python3` or `jq` for JSON (python3 is required by the installer), bash 3.2 or newer.

### 1. Hooks (per repository)

```sh
cd /path/to/your/agit/repo
/path/to/agit/integrations/claude-code/install.sh            # project scope
/path/to/agit/integrations/claude-code/install.sh --mcp      # also writes .mcp.json
```

The installer

- copies `hooks/*.sh` to `<repo>/.claude/hooks/agit/`,
- merges `integrations/claude-code/settings.json` into `<repo>/.claude/settings.json` (existing hooks, permissions and env are preserved; agit entries are recognised by their path and replaced, so re-running never duplicates them; the first write leaves a `settings.json.bak-agit`),
- sets `env.AGIT_AGENT` (default `claude-code`, `--agent <id>` to change) unless already set,
- appends `CLAUDE.md.snippet` to `<repo>/CLAUDE.md` between `<!-- agit-claude-code:begin/end -->` markers if absent,
- prints the `claude mcp add` command.

Options: `--repo <dir>` (default: current directory), `--agent <id>`, `--mcp` (merge the server into `<repo>/.mcp.json`), `--global` (install into `~/.claude/` so every agit repository on the machine gets the hooks; outside agit repositories they exit immediately), `--uninstall` (removes exactly what was installed, in either scope).

### 2. MCP server

```sh
claude mcp add --scope project agit -e AGIT_AGENT=claude-code -- agit mcp
```

or copy `integrations/claude-code/mcp.json` into `.mcp.json`:

```json
{ "mcpServers": { "agit": { "command": "agit", "args": ["mcp"], "env": { "AGIT_AGENT": "claude-code" } } } }
```

The server starts in Claude Code's working directory and discovers the repository from there. Add `AGIT_MODEL` and `AGIT_RUN_ID` to the env if you want them recorded on commits.

### 3. Check

Start `claude` in the repository. The first message of the session carries the agit briefing (status, leases, the loop). Run `/hooks` to see the five registered hooks and `/mcp` to see the `agit` server.

## The hooks

All scripts read the hook JSON from stdin, exit 0 with no output when there is nothing to say, and no-op in a few milliseconds when the path is not inside an agit repository (`.agit` directory or workspace pointer file) or `agit` is not installed. They never make Claude Code fail. `AGIT_HOOKS_DISABLE=1` pauses all of them.

| Script | Event / matcher | Behaviour |
| --- | --- | --- |
| `pre-edit.sh` | `PreToolUse` on `Edit\|Write\|MultiEdit\|NotebookEdit` | Runs `agit lease check <path> --holder $AGIT_AGENT`. If the path is under **another agent's exclusive lease, exits 2**: the edit is not performed and Claude sees who holds the lease and when it expires. Otherwise runs `agit know context <path> --json` (cached 60 s per path in `$TMPDIR/agit-claude-hooks/`) and returns a summary (note titles with their `kind:` tag, suggested tags, open PRs touching the path, other agents' shared leases) as `additionalContext`, at most 800 characters. No remote or an unreachable server: silent. |
| `post-edit.sh` | `PostToolUse` on the same tools | If no exclusive lease of `$AGIT_AGENT` covers the path, claims one: `agit lease claim <path> --ttl 30m --note "claude-code edit"` (`AGIT_HOOK_LEASE_TTL` changes the TTL). Silent; existing leases (including directory or glob leases that cover the file) are reused, never duplicated. If another agent took the path between the check and the edit, a warning is returned instead. |
| `checkpoint.sh` | `Stop`, `SubagentStop` | When `agit status --json` reports staged, unstaged, untracked or conflicted paths, records `agit checkpoint -m "claude-code stop"`. A fingerprint of the tree avoids a second checkpoint for an unchanged tree. Always exits 0, never prints (a failing Stop hook would prevent Claude from stopping). |
| `session-start.sh` | `SessionStart` (startup, resume, clear, compact) | Returns the briefing: repository root, branch, HEAD, dirty counts, remote; the agent id; active leases with holder, paths, mode and minutes left ("you" for your own); which hooks are active; the loop and the error-code rules. If agit is missing it says so instead. |
| `pre-commit-guard.sh` | `PreToolUse` on `Bash` | `agit commit` without `--intent` **is blocked (exit 2)** with the corrected command. `git commit` / `git push` inside an agit repository get a reminder to use agit (not blocked, so a repository that is also a git mirror keeps working). `agit pr create` without `--intent` gets a reminder. `--help` invocations pass. Needs no agit binary. |

`lib.sh` holds the shared helpers (JSON access via `jq` or `python3 -I`, repository discovery, cache, output formatting). The installer copies it along.

## What a session looks like

```
> claude
[SessionStart] agit repository /work/shop: branch feature/cart, HEAD 7f3c12a9, clean working tree;
  remote origin -> http://agit.internal/api/v1/repos/shop.
  You act as agent `claude-code` ...
  Active leases: coder-2: src/payments (exclusive, 22m left, refactor stripe client); ...

user: fix the rounding bug in the cart totals

claude: [Edit src/cart/total.rs]
[PreToolUse] agit context for src/cart/total.rs:
  - notes: "Totals are computed in minor units" [kind:fact]; "Rounding differs per currency" [kind:gotcha] ...
  - tags for notes about this path: area:cart, kind:gotcha
  - open PRs touching it: #14 "Cart: multi-currency" (coder-3)
claude: (reads the gotcha, edits accordingly)
[PostToolUse] (claims an exclusive 30 m lease on src/cart/total.rs, silently)

claude: [Edit src/payments/stripe.rs]
[PreToolUse, exit 2] agit blocked this edit: lease conflict: src/payments/stripe.rs is held by coder-2 (lease 2f28ca01).
  It expires in about 22 min (note: refactor stripe client). Another agent is working on this path ...
claude: coder-2 owns src/payments right now; I will leave the Stripe call as is and note it in the PR.

claude: [Bash: agit commit -m "Round cart totals per currency"]
[PreToolUse, exit 2] agit blocked this command: `agit commit` needs `--intent ...`
claude: [Bash: agit commit -m "Round cart totals per currency" --intent "Fix off-by-one-cent totals for JPY (SHOP-311)" --task SHOP-311]
claude: [Bash: agit pr create --push -t "Round cart totals per currency" --intent "..." --task SHOP-311]
claude: [Bash: agit know add -t "JPY has no minor unit" --tag kind:gotcha --tag area:cart --path "src/cart/**" -b "..."]

[Stop] (working tree clean, no checkpoint)
```

Hook output that goes to Claude is prefixed `agit ...` so it is easy to spot in the transcript. Blocking messages arrive via stderr and exit code 2, which Claude Code shows to the model as the reason the tool call was refused.

## Troubleshooting

- **Nothing happens at session start.** Check `/hooks` in Claude Code. If the hooks are listed, run one by hand: `echo '{"cwd":"'"$PWD"'"}' | .claude/hooks/agit/session-start.sh`. No output means no `.agit` was found walking up from `cwd`, or `agit` is not on the `PATH` Claude Code uses (GUI launches may have a shorter PATH; set `AGIT_BIN=/full/path/agit` in `settings.json` `env`).
- **Edits are blocked but no other agent is working.** `agit lease list` shows the holder. Leases of a crashed agent expire by their TTL; `agit lease release <id>` ends one early. Your own stale leases under a different id (for example after changing `AGIT_AGENT`) count as foreign.
- **Context never appears.** `agit know context <path> --json` needs a remote (`agit remote add origin <url>`) and a reachable server; failures are cached for 60 seconds per path. Delete `$TMPDIR/agit-claude-hooks/` to reset the cache.
- **Too many leases.** The post-edit hook claims one lease per file. Claim a directory yourself before a larger change (`agit lease claim src/cart --ttl 1h`); the hook detects covering leases and stays quiet. `agit lease release --all` at the end of a task.
- **Checkpoints pile up.** One per turn that changed the tree, by design. `agit checkpoint list -n 5` shows the newest; they are not part of branch history.
- **What the hooks cost.** About 120 ms per hook call and roughly 250 ms per file edit (pre and post), measured with `integrations/claude-code/bench_hooks.py`; most of it is `python3` start-up for JSON handling. See docs/BENCHMARKS.md.
- **Hooks time out.** Timeouts in `settings.json` are 10 to 30 seconds. The context call waits up to 10 seconds for a TCP connect; a server that accepts connections but does not answer can hold the hook until the timeout, after which Claude Code continues without the context.
- **Windows.** The scripts are bash; use WSL or Git Bash with `python3` or `jq` available.

Run the self-test any time: `integrations/claude-code/test.sh` (needs only the `agit` binary; it builds a temporary repository and simulates every hook).

## Security note

Hooks are shell scripts executed by Claude Code with your user's permissions on every matching tool call, with the tool input on stdin. Read `integrations/claude-code/hooks/*.sh` before installing them, as you would any pre-commit hook, and review changes when you update. The scripts only run `agit` subcommands (`lease check/list/claim`, `know context`, `status`, `checkpoint`, `remote list`), `python3 -I` or `jq` for JSON, and write a small cache under `$TMPDIR/agit-claude-hooks/`. They do not read file contents or send anything except the path to your configured agit server. Project-scope settings (`.claude/settings.json`) are shared with everyone who clones the repository; Claude Code asks for confirmation before enabling project hooks for that reason.
