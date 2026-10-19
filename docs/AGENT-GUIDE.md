# Working with agit as an AI agent

This is the loop agit is built around. Every step has a CLI form (`--json`
everywhere), an HTTP form (`docs/API.md`) and an MCP tool (`agit mcp`).

## 1. Identify yourself

Set `AGIT_AGENT` (stable id such as `coder-1`), optionally `AGIT_MODEL` and
`AGIT_RUN_ID`. Every commit, lease, review and note you create carries this
identity, and `agit log --agent coder-1` or the web UI's Agents tab finds your
work later.

## 2. Read before you touch

```sh
agit know context src/auth/login.rs      # notes, suggested tags, recent commits, leases, open PRs
agit log --path src/auth -n 10 --json
```

The context call is one request on purpose: it is what you should read before
changing a path, and it is cheap enough to call every time.

## 3. Claim what you will change

```sh
agit lease claim src/auth --ttl 30m --note "refactor session handling"
```

Other agents now see the claim (`agit lease list`, lease board in the UI), on every machine that uses the same server: with a remote, the claim is registered there at once.
A commit by another agent into your exclusive lease fails with
`lease_conflict` unless they pass `--force`. Leases expire on their own, so a
crashed agent never blocks anyone for long. Release early with
`agit lease release --all`.

## 4. Work in your own workspace or branch

```sh
agit ws new coder-1            # separate working directory, branch ws/coder-1, shared object store
agit switch -c feature/x       # or a plain branch in the current working directory
```

## 5. Checkpoint before risky edits

```sh
agit checkpoint -m "before rewriting the parser"
agit checkpoint list
agit restore <checkpoint id>   # whole working tree, including untracked files
```

Checkpoints never appear in the branch history and cost one tree walk.

## 6. Commit with intent

```sh
agit add -A
agit commit -m "Rotate session tokens on login" \
  --intent "Close the fixation window reported in SEC-12" --task SEC-12
```

`intent` answers why, `message` answers what, `task` links it to your tracker.
Server policies can require intent and agent identity on protected branches.

## 7. Push and open a pull request

```sh
agit pr create --push -t "Rotate session tokens" --intent "..." --task SEC-12
agit pr view 1 --json          # commits, changes, mergeability, conflicts, reviews
```

A reviewer agent can comment with a kind (`question`, `suggestion`,
`blocker`, `note`) anchored to a path and line, then `agit pr review 1
--decision approve`. Merging happens on the server with a compare-and-swap on
the target branch; a `ref_conflict` means someone else moved it first and the
call is safe to retry after re-checking.

## 8. Record what you learned

```sh
agit know add -t "Session store is eventually consistent" \
  --tag kind:gotcha --tag area:auth --path src/auth/** --commit HEAD \
  -b "Reads after a token rotation can lag by up to 200 ms; retry once."
```

Tags are `namespace:value`. Tag rules (`PUT /knowledge/rules`) tell you which
tags a path expects; `agit know context` returns them as `suggested_tags`.

## Error handling

Every error has a `code` and a `retryable` flag:

| code | meaning | what to do |
| --- | --- | --- |
| `ref_conflict` | someone moved the ref you based your update on | fetch/pull, re-check, retry |
| `lock_timeout` | a lock was busy | retry |
| `lease_conflict` | another agent holds the path | pick other work, wait, or ask |
| `merge_conflicts` | three-way merge needs a decision | resolve files with markers, `agit add`, commit |
| `policy_violation` | server policy refused the change | read the message; usually missing intent, approvals or a direct push to a protected branch |
| `dirty_worktree` | local edits would be overwritten | checkpoint or commit first |

Exit codes of the CLI: 0 ok, 1 error, 3 conflict, 75 retryable.
