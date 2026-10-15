# agit handbook

The complete user and developer handbook for **agit**, version control and collaboration for AI agents with git's workflow. It covers agit 0.1.0 as built on 2026-10-15; every command and response shown in it was run against the real binaries. The primary documents in [`docs/`](..) remain normative for the HTTP contract ([API.md](../API.md)) and the on-disk formats ([ARCHITECTURE.md](../ARCHITECTURE.md)); the handbook explains, connects and verifies them.

A German overview is in [de/UEBERBLICK.md](de/UEBERBLICK.md).

## Chapters

| # | Chapter | One line |
| --- | --- | --- |
| 0 | [Introduction](00-introduction.md) | What agit is, who it is for, how it differs from git on one page, the status legend used throughout. |
| 1 | [Why agit](01-why.md) | The ten concrete ways git fails agents, the design answer to each, and the honest limits of 0.1.0. |
| 2 | [Installation](02-installation.md) | cargo, release binaries via `scripts/install.sh`, Docker, the Homebrew template, first checks, optional components. |
| 3 | [Quick start for developers](03-quickstart-developers.md) | A full first session: init, commit, branch, a merge conflict and its resolution, stash, blame, reflog undo, checkpoints, gc, submodules, git import and export. |
| 4 | [Quick start for agents](04-quickstart-agents.md) | The agent loop against a running server: identity, context, lease, checkpoint, commit with intent, PR, review by a second agent, merge under a policy, signing, knowledge notes, stale-note validation, with the JSON. |
| 5 | [Concepts](05-concepts.md) | Objects and ids, trees, commits and metadata, refs and CAS, the index, workspaces, leases, checkpoints, reflog, signatures and trust, policies, events, the knowledge model, submodules, remotes, pack segments and durability. |
| 6 | [CLI reference](06-cli-reference.md) | Every command with synopsis, flags, JSON result shape, exit codes, error codes and environment variables, grouped by local, coordination, remote, platform, knowledge, keys, bridge and maintenance. |
| 7 | [MCP reference](07-mcp-reference.md) | Registering `agit mcp`, the instructions string, all 33 tools with arguments, example calls, the error shape, the recommended sequence, and the Claude Code hooks. |
| 8 | [HTTP API](08-http-api.md) | A guided tour of `docs/API.md` with curl: auth and scoped tokens, the error envelope, CAS refs, object streams, browsing, SSE, leases, the PR lifecycle, policy, keys, webhooks, knowledge, context packs. |
| 9 | [SDKs](09-sdks.md) | Python and TypeScript walkthroughs mirroring the agent loop, `AgentSession`, errors, CAS, streaming, and when to use CLI, MCP or SDK. |
| 10 | [Web UI](10-web-ui.md) | Every page and what it shows, mock mode, settings, client-side blame and search, compare, notifications, keyboard shortcuts, screenshots. |
| 11 | [Knowledge base](11-knowledge-base.md) | Why knowledge lives with the code, the tag taxonomy, tag rules, how context is assembled, staleness and validation, drafts from merges, note templates, query strategies, offline store, limits. |
| 12 | [Multi-agent workflows](12-multi-agent-workflows.md) | One workspace per agent, lease etiquette, checkpoint cadence, PR-based integration, reviewer agents, policy templates (solo, team, strict), handling the conflicts, recovery. |
| 13 | [Server administration](13-server-administration.md) | Running, data layout, backups, root and scoped tokens, audit log, policies, keys, webhooks, monitoring, upgrades, hardening, troubleshooting. |
| 14 | [Architecture and internals](14-architecture-internals.md) | Formats (AGO1, AGP/AGX1 segments, AGT1, AGI2), publish-on-flush, durability levels, locking, merge and blame algorithms, the git bridge mapping, server internals, what is planned. |
| 15 | [FAQ](15-faq.md) | Short answers, troubleshooting, and the list of discrepancies between the documentation and the 0.1.0 binaries. |
| 16 | [Glossary](16-glossary.md) | Every term with a one-line definition and a pointer. |
| 17 | [Comparison](17-comparison.md) | agit versus git and GitHub-style platforms, feature tables, when (not) to use agit. |

## Suggested reading order

**Developers** who know git and will use agit day to day:
[0](00-introduction.md) → [3](03-quickstart-developers.md) → [5](05-concepts.md) → [6](06-cli-reference.md) → [15](15-faq.md). Add [4](04-quickstart-agents.md) when a server enters the picture and [17](17-comparison.md) when someone asks "why not git".

**Agent authors and operators of agent fleets**:
[1](01-why.md) → [4](04-quickstart-agents.md) → [7](07-mcp-reference.md) → [9](09-sdks.md) → [11](11-knowledge-base.md) → [12](12-multi-agent-workflows.md) → [6](06-cli-reference.md#exit-codes-and-error-codes) for the error table. [docs/AGENT-GUIDE.md](../AGENT-GUIDE.md) is the one-page version to paste into a system prompt; [docs/CLAUDE-CODE.md](../CLAUDE-CODE.md) is the Claude Code setup.

**Operators** hosting `agit-server`:
[2](02-installation.md) → [13](13-server-administration.md) → [8](08-http-api.md) → [10](10-web-ui.md) → [14](14-architecture-internals.md#durability-levels), then [docs/DEPLOYMENT.md](../DEPLOYMENT.md) and [docs/SECURITY.md](../SECURITY.md) for the unit files, proxy configurations and the threat model.

**Contributors**:
[5](05-concepts.md) → [14](14-architecture-internals.md) → the RFCs in [docs/rfcs/](../rfcs) → [docs/ROADMAP.md](../ROADMAP.md) → [15](15-faq.md#known-discrepancies-and-rough-edges) for things worth fixing.

## How the handbook was verified

All examples were executed in a throwaway repository with the binaries in `target/release/` (agit 0.1.0, workspace commit `b1fc6ff`), including a local `agit-server` on port 17979 with a bearer token, a second clone acting as a reviewer agent, a webhook receiver, the Python and TypeScript SDK scripts, the MCP server over stdio and the Claude Code hook self-test. Outputs are pasted as produced, with long ids shortened and unrelated fields elided (`…`). Where the primary documentation and the binaries disagreed, the handbook follows the binaries and records the difference in [15. FAQ](15-faq.md#known-discrepancies-and-rough-edges).

## Conventions

- `$` prefixes a command; following lines are output.
- Status marks: ✅ built and verified, 🚧 partially built or newer than the verified binaries, 📋 planned.
- `docs/...` links point at the normative documents; chapter links stay inside the handbook.
