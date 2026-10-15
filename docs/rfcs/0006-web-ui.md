# RFC 0006: Web UI

- Status: **Draft**
- Phase: 3 (code browsing first, then PRs), with knowledge views in Phase 4
- Depends on: RFC 0004 (server), RFC 0005 (PRs)

## Summary

A clean, GitHub-like, fast web UI for agit repositories: code browsing with syntax highlighting and symbol navigation, history and blame, diffs, pull requests and reviews, branches and workspaces, live agent activity, a lease board, and a knowledge browser. TypeScript and React with server-side rendering. The UI talks exclusively to the public API that agents use.

## Motivation

Humans supervise agents. They need to see what agents did, what they are doing right now, where they are working, and what they decided, in a form that is faster to scan than a terminal. The UI should feel like the code hosting humans already know, with the agent views added rather than bolted on. Building it strictly on the public API keeps the API honest: if the UI can show it, an agent can query it.

## Design

### Principles

1. **Public API only.** No private endpoints, no server-side shortcuts. An audit script in CI enumerates every request the UI makes and fails if an endpoint is not in the published API.
2. **Fast first paint.** Server-side rendering for every page; hydration only where interaction needs it. Target under 100 ms server render for a file view.
3. **Clean.** Dense but calm layouts, monospace where it matters, no decoration that does not carry information.
4. **Read-your-own-data.** The UI shows exactly the data the API returns, including ids and sequence numbers, so what a human sees can be reproduced from a shell.

### Views, in delivery order

**Milestone 1: code**

- Repository home: README, default branch, recent commits with intent and agent, open leases count.
- Tree and file view: syntax highlighting (tree-sitter grammars, rendered server-side), line linking, file size and mode from the tree entry (no blob fetch for the listing).
- Symbol navigation: definitions and references from a server-side symbol index (tree-sitter based), with jump-to-definition in the file view.
- History: commit list with `intent`, `task`, `agent.id`, filterable by agent, task, path, and with checkpoints hidden by default.
- Blame: per-line last commit with intent shown on hover.
- Diff: side-by-side and unified, from the core diff output, with `--compact` grouping.

**Milestone 2: collaboration**

- Pull requests: list and detail with structured comments, verdicts, checks, policy results and the decision log (RFC 0005). Blocking comments are visually distinct from questions and suggestions; suggestions can be applied with one click, producing a commit.
- Branches and workspaces: which workspace is on which commit, owned by which agent.
- Live agent activity: a stream fed by the server event log over SSE, grouped by agent, with links to the resulting commits, leases and PRs.
- Lease board: active leases by path pattern, holder, expiry, workspace; conflicts highlighted; filter by path.

**Milestone 3 (Phase 4): knowledge**

- Knowledge browser: notes by tag and path, validity, supersession chain; "what should I read before touching this path" as a UI action; note history from `refs/knowledge/*`.

### Stack

TypeScript, React, a server-rendering framework with streaming, a small design system owned by the project. Highlighting and symbol extraction run server-side with tree-sitter so the client ships no grammars. The client subscribes to the event stream for live views and otherwise fetches on navigation.

### Performance budget

| page | server render target |
| --- | --- |
| file view (10k lines) | < 100 ms |
| diff (5k changed lines) | < 300 ms |
| PR detail with 200 comments | < 200 ms |
| history page (100 commits) | < 100 ms |

Measured on the reference server configuration and published with Phase 3.

### Authentication

Human login via OIDC; the browser session holds a scoped token like any agent token. Permissions are the same policy objects the server applies to agents.

## Alternatives

- **Client-side rendered SPA.** Rejected: first paint and search-engine indexing suffer, and large files become a client memory problem.
- **Reusing an existing forge UI.** Rejected: the agent views (activity, leases, intents, structured reviews) do not fit an existing data model, and the strict public-API rule would be hard to retrofit.
- **Terminal UI instead of web.** Not instead; a TUI may come later for `agit events` and the lease board, but supervision of many agents needs a browser.

## Open questions

- Symbol index scope: per branch head only, or historical.
- Whether live activity should also render locally from `.agit/events.jsonl` in a single-user mode without a server.
- Accessibility and keyboard navigation standards to adopt from day one.
- How much of the design system to publish as a standalone package for SDK authors building dashboards.
