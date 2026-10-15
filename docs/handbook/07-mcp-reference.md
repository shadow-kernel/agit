# 7. MCP reference

`agit mcp` turns the CLI into a [Model Context Protocol](https://modelcontextprotocol.io) server over stdio: JSON-RPC 2.0, one message per line, protocol version `2025-06-18`. Any MCP-capable agent can then use agit without shelling out. Tool results carry the same JSON as `agit --json`; failures come back as `isError: true` with the same error codes. The server is in `crates/agit-cli/src/mcp.rs`; the examples below were produced by piping requests into `agit mcp` inside a repository.

## Registering the server

The server discovers the repository from its working directory (or `-C <dir>`), so start it inside the repository. Agent identity comes from the environment:

```json
{ "mcpServers": { "agit": { "command": "agit", "args": ["mcp"], "env": { "AGIT_AGENT": "coder-1", "AGIT_MODEL": "my-model", "AGIT_TOKEN": "…" } } } }
```

`--remote <name>` (default `origin`) selects the remote used by the `pr`, `know`, `context`, `push` and `pull` tools. For Claude Code:

```sh
claude mcp add --scope project agit -e AGIT_AGENT=coder-1 -- agit mcp
```

or copy `integrations/claude-code/mcp.json` to `.mcp.json`.

## Handshake

```
→ {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"handbook","version":"0"}}}
← {"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"agit","version":"0.1.0"},"instructions":"agit version control for agents. Typical flow: agit_context(path) before editing, agit_lease_claim for the paths you will change, agit_checkpoint before risky edits, agit_add + agit_commit with intent/task/agent, agit_pr_create when done, agit_know_add to record what you learned."}}
→ {"jsonrpc":"2.0","method":"notifications/initialized"}
```

The `instructions` string is the recommended loop. `ping` answers `{}`; `resources/list` and `prompts/list` return empty lists; notifications get no reply; an unknown method is `{"error":{"code":-32601,"message":"method not found: …"}}`; a malformed line is `-32700`.

## Tool list

`tools/list` returns 33 tools (the README still says 32; `agit_know_sync` was added). Each has an `inputSchema` with `additionalProperties: false`. Arguments shown as `name*` are required.

### Repository state

| Tool | Arguments | Result |
| --- | --- | --- |
| `agit_status` | none | `status` object (`workspace, branch, head, staged, unstaged, untracked, conflicts, merge_in_progress`) |
| `agit_log` | `rev` (HEAD), `limit` (20), `agent`, `task`, `path` | `[{id, …commit}]` |
| `agit_show` | `rev*`, `patch` (true) | `{id, commit, changes, patch}` |
| `agit_diff` | `staged`, `from`, `to`, `context` (3) | `{changes, stats, patch}`; no args: worktree vs index; `staged`: index vs HEAD; `from`/`to`: revisions |
| `agit_read_file` | `path*`, `rev` | `{path, rev?, id?, size, binary, content|null}` |
| `agit_ls` | `path` (""), `rev` (HEAD) | `{path, entries:[{name, mode, id, size}]}` |
| `agit_branches` | none | `[{name, id, current}]` |
| `agit_workspaces` | none | `[{name, path, created_ms, agent?}]` |
| `agit_events` | `since` (0), `limit` (100), `kind` | last `limit` events after `since`, filtered by kind prefix |

### Writing

| Tool | Arguments | Result |
| --- | --- | --- |
| `agit_add` | `paths[]`, `all` | `{added, updated, removed, unchanged}` |
| `agit_commit` | `message*`, `intent`, `task`, `agent`, `model`, `run_id`, `all`, `force`, `meta` (object), `sign` (key name, "" = default) | commit result `{id, tree, parents, branch, changes, summary}`; refuses paths under another agent's exclusive lease unless `force` |
| `agit_checkpoint` | `message` | `{id, tree, previous, changes_vs_head, created}` |
| `agit_checkpoint_list` | `limit` (20) | `[{id, …commit}]` |
| `agit_restore` | `rev*` | list of applied changes (overwrites local changes) |
| `agit_switch` | `target*`, `create`, `force` | `{head:{kind,name?}, id, files_changed}` |
| `agit_merge` | `rev*`, `message`, `no_ff`, `agent` | merge result (`fast_forward`, `merged`, `already_up_to_date` or `conflicts` with paths) |

Agent identity for `agit_commit`, `agit_checkpoint`, `agit_merge` and `agit_pull` is `agent`/`model`/`run_id` from the arguments, else `AGIT_AGENT`/`AGIT_MODEL`/`AGIT_RUN_ID` from the environment. Signing follows the CLI rules (`AGIT_SIGN`, key named after the agent).

### Coordination

| Tool | Arguments | Result |
| --- | --- | --- |
| `agit_lease_claim` | `holder*`, `paths[]*`, `ttl_secs` (3600), `note`, `shared` | lease object; `lease_conflict` when someone else holds an overlapping exclusive lease |
| `agit_lease_release` | `id` or `holder` | `[released leases]` |
| `agit_lease_list` | none | `[lease]` |
| `agit_lease_check` | `holder*`, `paths[]*` | `{allowed:true}` or `lease_conflict` |

### Remote and pull requests

| Tool | Arguments | Result |
| --- | --- | --- |
| `agit_push` | `remote`, `branch` (current), `force`, `create` | push result |
| `agit_pull` | `remote` | `{fetch, merge}` |
| `agit_pr_create` | `title*`, `body`, `intent`, `task`, `source` (current branch), `target` (default branch), `push` (**true**) | PR object |
| `agit_pr_list` | `state` ("open") | `[pr]` |
| `agit_pr_view` | `number*` | `{pr, commits, changes, mergeable, conflicts}` |
| `agit_pr_review` | `number*`, `decision*` (`approve|request_changes|comment`), `body` | PR |
| `agit_pr_comment` | `number*`, `body*`, `path`, `line`, `kind` (`question|suggestion|blocker|note`, default `note`) | comment |
| `agit_pr_merge` | `number*`, `strategy` (`merge|ff`), `message` | `{pr, merged_commit}` |

Note that `agit_pr_create` pushes by default, unlike the CLI where `--push` is opt-in.

### Knowledge

| Tool | Arguments | Result |
| --- | --- | --- |
| `agit_context` | `path*` | context pack `{path, notes, suggested_tags, recent_commits, leases, open_pulls, agents_active}` plus `source: server|local` |
| `agit_know_search` | `tags[]` (all must match), `path`, `query`, `limit` (20) | note summaries |
| `agit_know_get` | `id*` | full note |
| `agit_know_add` | `title*`, `body*`, `tags[]`, `paths[]`, `commits[]` (revisions), `confidence` | note; stored on the server, or locally in `.agit/knowledge` when no server is reachable |
| `agit_know_sync` | none | `{pushed, pulled, rules_pulled}` |

Full schema of the three most used write tools, as returned by `tools/list`:

```json
{"name":"agit_commit","inputSchema":{"type":"object","properties":{"message":{"type":"string"},"intent":{"type":"string"},"task":{"type":"string"},"agent":{"type":"string"},"model":{"type":"string"},"run_id":{"type":"string"},"all":{"type":"boolean","description":"stage tracked modifications first"},"force":{"type":"boolean"},"meta":{"type":"object"},"sign":{"type":"string","description":"key name to sign with (empty string = default key)"}},"required":["message"],"additionalProperties":false}}
{"name":"agit_lease_claim","inputSchema":{"type":"object","properties":{"holder":{"type":"string"},"paths":{"type":"array","items":{"type":"string"}},"ttl_secs":{"type":"integer","default":3600},"note":{"type":"string"},"shared":{"type":"boolean"}},"required":["holder","paths"],"additionalProperties":false}}
{"name":"agit_know_add","inputSchema":{"type":"object","properties":{"title":{"type":"string"},"body":{"type":"string"},"tags":{"type":"array","items":{"type":"string"}},"paths":{"type":"array","items":{"type":"string"}},"commits":{"type":"array","items":{"type":"string"}},"confidence":{"type":"number"}},"required":["title","body"],"additionalProperties":false}}
```

## Calling a tool

Success. The result JSON is delivered twice: serialized in `content[0].text` for clients that only read text, and as `structuredContent` for clients that read structured output:

```
→ {"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"agit_status","arguments":{}}}
← {"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"{\"workspace\":\"main\",\"branch\":\"feature/unsigned\",…}"}],"structuredContent":{"workspace":"main","branch":"feature/unsigned","head":"f7d8f796…","staged":[],"unstaged":[],"untracked":[],"conflicts":[],"merge_in_progress":false},"isError":false}}
```

```
→ {"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"agit_log","arguments":{"limit":2,"agent":"coder-1"}}}
← … "structuredContent":[{"id":"f7d8f796…","message":"Unsigned change","agent":{"id":"coder-1",…},…},{"id":"83e741ad…","message":"Punctuation",…}] …
```

## Error shape

A failed tool call is **not** a JSON-RPC error; it is a successful response with `isError: true` and the CLI error envelope as content:

```
→ {"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"agit_show","arguments":{"rev":"nope"}}}
← {"jsonrpc":"2.0","id":4,"result":{"content":[{"type":"text","text":"{\"ok\":false,\"error\":{\"code\":\"unknown_revision\",\"message\":\"unknown revision: nope\",\"retryable\":false}}"}],"structuredContent":{"ok":false,"error":{"code":"unknown_revision","message":"unknown revision: nope","retryable":false}},"isError":true}}
```

Core errors keep their stable `code` and `retryable`; errors from the tool layer (missing required argument, unknown tool, transport failure) have `code: "error"`. When no remote is configured, `agit_context`, `agit_know_search`, `agit_know_get` and `agit_know_add` fall back to the local store; `agit_pr_*` and `agit_push`/`agit_pull` report an error.

## Recommended tool sequence

1. `agit_status` to learn branch, HEAD and dirty state (the Claude Code session-start hook does this for you).
2. `agit_context(path)` for every path you intend to change; read `notes` (fresh ones first), note `suggested_tags`, `leases`, `open_pulls` and `agents_active`.
3. `agit_lease_check(holder, paths)` if you only want to know; `agit_lease_claim(holder, paths, ttl_secs, note)` when you start. Prefer one directory lease over many file leases.
4. `agit_checkpoint(message)` before a destructive edit; `agit_restore(rev)` if it went wrong.
5. Edit with your own tools. `agit_read_file`, `agit_ls`, `agit_diff` help you look around.
6. `agit_add(paths|all)`, then `agit_commit(message, intent, task)`; on `lease_conflict` stop and pick other work, do not pass `force` unless told to.
7. `agit_pr_create(title, intent, task)` (pushes first). A reviewer agent uses `agit_pr_view`, `agit_pr_comment(kind=blocker|suggestion|question)`, `agit_pr_review(decision)`, `agit_pr_merge`.
8. `agit_know_add(title, body, tags, paths, commits)` with the suggested tags plus a `kind:` tag; `agit_know_sync` if you worked offline.
9. `agit_lease_release(holder)` at the end.
10. On `ref_conflict` or `lock_timeout` (retryable): `agit_pull`, re-check, retry. On `policy_violation`: read the message; usually a missing intent, approval or signature, or a direct push to a protected branch.

## Claude Code integration

`integrations/claude-code/` adds five hooks that make the loop deterministic even when the model forgets: a pre-edit hook that blocks edits on paths under another agent's exclusive lease (exit 2) and injects the context pack as `additionalContext`; a post-edit hook that claims a 30-minute lease on every file edited; a stop hook that checkpoints a dirty tree at the end of each turn; a session-start hook that briefs Claude with repository state, active leases and the loop; and a Bash guard that blocks `agit commit` without `--intent` and reminds about `git commit`/`git push`. `install.sh` copies the scripts into `.claude/hooks/agit/`, merges `settings.json`, sets `env.AGIT_AGENT` (default `claude-code`), appends `CLAUDE.md.snippet` to `CLAUDE.md`, and with `--mcp` writes `.mcp.json`; `--global` installs into `~/.claude`; `--uninstall` removes exactly what was installed. `AGIT_HOOKS_DISABLE=1` pauses all hooks, `AGIT_BIN` points at a non-PATH binary, `AGIT_HOOK_LEASE_TTL` changes the lease TTL.

The self-test passes against the 0.1.0 binary:

```sh
$ AGIT_BIN=$PWD/target/release/agit bash integrations/claude-code/test.sh | tail -1
passed: 100  failed: 0
```

The full description, the session transcript, troubleshooting and the security note are in [docs/CLAUDE-CODE.md](../CLAUDE-CODE.md). Read `integrations/claude-code/hooks/*.sh` before installing them, as you would any pre-commit hook; they only run `agit` subcommands and `python3 -I`/`jq`.
