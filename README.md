<p align="center">
  <a href="https://shadow-kernel.github.io/agit-site/"><img src="docs/assets/logo.svg" width="104" alt="agit logo"></a>
</p>

<h1 align="center">agit</h1>

<p align="center">
  <strong>git's workflow, re-engineered for AI agents.</strong><br>
  Version control and collaboration for machines that commit thousands of times an hour.
</p>

<p align="center">
  <a href="https://shadow-kernel.github.io/agit-site/"><strong>🌐 Website</strong></a> ·
  <a href="https://shadow-kernel.github.io/agit-site/guide.html">Guide</a> ·
  <a href="https://shadow-kernel.github.io/agit-site/features.html">Features</a> ·
  <a href="https://shadow-kernel.github.io/agit-site/scale.html">Large repos</a> ·
  <a href="docs/handbook/README.md">Handbook</a> ·
  <a href="https://github.com/shadow-kernel/agit/releases/latest">Download</a>
</p>

<p align="center">
  <a href="https://github.com/shadow-kernel/agit/releases/latest"><img src="https://img.shields.io/github/v/release/shadow-kernel/agit?color=22d3ee&label=release" alt="Latest release"></a>
  <a href="https://github.com/shadow-kernel/agit/actions/workflows/ci.yml"><img src="https://github.com/shadow-kernel/agit/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="#license"><img src="https://img.shields.io/badge/license-MIT%20%7C%20Apache--2.0-8b5cf6" alt="License: MIT or Apache-2.0"></a>
  <img src="https://img.shields.io/badge/rust-1.89%2B-f59e0b?logo=rust" alt="Rust 1.89+">
  <a href="https://shadow-kernel.github.io/agit-site/"><img src="https://img.shields.io/badge/website-agit--site-0b1220?logo=githubpages" alt="Website"></a>
</p>

---

> **👉 Start on the website: [shadow-kernel.github.io/agit-site](https://shadow-kernel.github.io/agit-site/)**
> Why agit instead of git, the full installation and server guide, how it handles very large repositories, and every feature with what, why and how.

agit keeps `init`, `add`, `commit`, `branch`, `merge`, `push` and `pull`, so people need to learn nothing new. Underneath, everything is rebuilt for a world where most commits come from AI agents that work in parallel, crash halfway, retry blindly and have to coordinate without anyone watching. Every command speaks JSON, every error has a stable code, every commit carries intent and agent identity, and agents claim work before they collide instead of discovering conflicts at merge time.

<p align="center">
  <img src="docs/screenshots/repo-home.png" width="860" alt="agit web UI: repository home">
</p>

## ✨ Why agit instead of git

git is the best developer tool of the last twenty years. Used by a fleet of agents, it fails in specific, repeatable ways. agit answers each one directly instead of wrapping git.

| With git, agents… | With agit |
| --- | --- |
| parse prose output with regexes that break on locale and version | `--json` on every command, one envelope, stable error codes, a `retryable` flag and exit codes 0 / 1 / 3 / 75 |
| leave `index.lock` behind on every crash and block everyone | advisory `flock` locks the kernel releases when a process dies; nothing to clean up, ever |
| discover at merge time that two of them edited the same module | **leases**: claim `src/auth/**` for 30 minutes, everyone else gets `lease_conflict` in milliseconds |
| race on shared branches, last writer wins | **compare-and-swap** on every ref update, atomic multi-ref transactions |
| bury the *why* in a commit message | `intent`, `task` and `agent {id, model, run_id}` as structured commit fields, Ed25519 signatures |
| pollute history with WIP commits to be able to roll back | **checkpoints** outside branch history, untracked files included, `agit restore` in one command |
| poll `git log` to notice each other | an **event log** with sequence numbers, `agit events --follow`, SSE and signed webhooks |
| never see the decisions and gotchas that live in wikis | a **knowledge base** next to the code: the right notes arrive with the path an agent is about to touch |

## 🚀 Install

**macOS and Linux**, release binaries with checksum verification:

```sh
curl -fsSL https://raw.githubusercontent.com/shadow-kernel/agit/main/scripts/install.sh | sh
agit --version
```

**Docker**, the server with the CLI and the web UI, closed to everyone except your SSH key:

```sh
docker run -d --name agit -p 127.0.0.1:7878:7878 -v agit-data:/data \
  -e AGIT_ADMIN_KEYS="$(cat ~/.ssh/id_ed25519.pub)" ghcr.io/shadow-kernel/agit:latest
```

**From source** (Rust 1.89+):

```sh
git clone https://github.com/shadow-kernel/agit && cd agit
cargo build --release && export PATH="$PWD/target/release:$PATH"
```

Windows: download the zip from [Releases](https://github.com/shadow-kernel/agit/releases/latest). All options, systemd, reverse proxies and TLS: [the guide](https://shadow-kernel.github.io/agit-site/guide.html#install).

## ⚡ 60 seconds

```sh
export AGIT_AGENT=coder-1                # who is working: recorded on commits, used as lease holder
mkdir demo && cd demo && agit init
echo '# demo' > README.md && agit add -A
agit commit -m "Add README" --intent "Bootstrap the project" --task DEMO-1

agit status --json                       # {"ok":true,"result":{"branch":"main","staged":[],…}}
agit lease claim src/auth --ttl 30m      # others now get lease_conflict for src/auth
agit checkpoint -m "before the rewrite"  # roll back any time with: agit restore <id>
agit events --follow                     # watch every commit, lease and ref move live
```

## 🖥️ Your own server

```sh
agit-server --data-dir ./agit-data --listen 0.0.0.0:7878 --admin-key ~/.ssh/id_ed25519.pub
```

```sh
agit auth login --url http://my-server:7878/api/v1            # signs a challenge with your SSH key
agit remote add origin http://my-server:7878/api/v1/repos/shop
agit push --create

AGIT_AGENT=coder-1    agit pr create --push -t "Rotate tokens" --intent "Close SEC-12" --task SEC-12
AGIT_AGENT=reviewer-1 agit pr review 1 --decision approve -b "lgtm"
AGIT_AGENT=reviewer-1 agit pr merge 1                          # server-side merge with compare-and-swap
```

🔐 **Not everyone sees everything.** People sign in with the SSH keys they already have; every key or token gets `read`, `write` or `admin` per repository; a server with no credentials configured refuses to listen on a non-loopback address; every denial and change lands in an audit log. Branch policies keep agents off `main` without a review.

```sh
agit auth key add dana.pub --name "dana laptop" --scope shop:write --scope docs:read
agit auth token create --name ci --agent ci-bot --scope shop:write --ttl 30d
```

## 🤖 Built for agents and Claude Code

```sh
integrations/claude-code/install.sh --mcp --agent coder-1    # run inside your agit repository
```

| What happens | git with Claude | agit with Claude Code |
| --- | --- | --- |
| Before Claude edits a file | nothing | notes, open PRs and other agents' leases for that path appear in its context |
| Another agent owns the file | both edit, conflict later | the edit is **blocked before the write**, with who holds it and until when |
| After an edit | nothing | a lease is claimed automatically so the next agent sees it |
| Claude stops | uncommitted mess | a checkpoint of the whole tree, outside history |
| `commit -m "fix"` | accepted | refused without `--intent`; intent, task, agent and model are recorded |
| What Claude learned | lost with the transcript | stored as a note next to the code for the next session |

Two halves that work on their own and better together: an **MCP server** (`agit mcp`, 33 typed tools with the same JSON and error codes as the CLI) and **five hooks** that make the loop deterministic. The same loop works for any framework through the CLI, the HTTP API or the **Python** and **TypeScript SDKs**. Details: [docs/CLAUDE-CODE.md](docs/CLAUDE-CODE.md) · [docs/AGENT-GUIDE.md](docs/AGENT-GUIDE.md).

## 🧩 What's inside

| | |
| --- | --- |
| **Local core** | BLAKE3 ids, zstd objects verified on every read, append-only pack segments, journal with group commit, index with stat and tree cache, branches, merges with diff3 markers, stash, revert, cherry-pick, blame, grep, reflog, reset, gc, submodules |
| **Coordination** | leases with TTL and globs, workspaces (one working directory per agent, one object store), checkpoints, event log |
| **Server** | one static binary, plain files, no database: clone / push / pull with want/have transfer, CAS refs, SSE events, webhooks, audit log |
| **Platform** | pull requests with typed comments (`blocker`, `suggestion`, `question`), reviews, checks, server-side merge, branch policies, signing-key registry |
| **Knowledge** | notes with typed tags, path rules, BM25 search, staleness when code changes, drafts from every merged PR, one-call context packs, offline store |
| **Interop** | `agit git import` and `agit git export` with full history, web UI, Python and TypeScript SDKs, MCP server |

The full list, 141 features with what, why and how and an honest built / in progress / planned status: **[agit-site/features](https://shadow-kernel.github.io/agit-site/features.html)**.

<table>
  <tr>
    <td><img src="docs/screenshots/pull-request.png" alt="Pull request with typed comments and reviews"></td>
    <td><img src="docs/screenshots/commits.png" alt="Commits with agent, intent and task"></td>
  </tr>
  <tr>
    <td align="center"><sub>Pull requests with typed comments, reviews and checks</sub></td>
    <td align="center"><sub>Commits with agent, intent and task</sub></td>
  </tr>
</table>

## 📦 Bring your git repository

```sh
git clone --mirror https://github.com/you/project.git project.git
agit git import project.git --into project      # full history, branches, tags, submodule pins
cd project && agit remote add origin http://my-server:7878/api/v1/repos/project && agit push --create
```

Original SHAs stay in the commit metadata, `Intent:` and `Task:` trailers become structured fields, and `agit git export` writes back to git so the people who need git keep it.

## 📊 Measured, not promised

| Workload (Apple Silicon laptop) | git | agit |
| --- | ---: | ---: |
| `add` + `commit` of one file, 2,000-file tree | 34.6 ms | **12.5 ms** |
| `log -n 20` | 12.7 ms | **3.4 ms** |
| `status` on a clean tree | 15.3 ms | **11.1 ms** |
| Server, 8 agents committing and pushing (in-process) | – | **673 commits/s** |
| Server, compare-and-swap updates on independent branches | – | **3,565 updates/s** |

Hardware, commands and the full series: [docs/BENCHMARKS.md](docs/BENCHMARKS.md). Very large repositories, the 80 GB game-engine case and today's limits: [agit-site/scale](https://shadow-kernel.github.io/agit-site/scale.html).

## 🗺️ Status

**v0.1** is a working system, not a finished one. The local core, the single-node server and the platform are built and tested. Replication, blob tiers for huge binaries, partial clone and the 1.0 format freeze are ahead.

| Phase | Theme | Status |
| --- | --- | --- |
| 0 | Local core | ✅ |
| 1 | Performance: segments, journal, group commit ✅ · million-file status, token-efficient views 📋 | 🚧 |
| 2 | Protocol and server: clone / push / pull, SSH keys and scopes ✅ · Raft, blob tiers, sharding 📋 | 🚧 |
| 3 | Platform: PRs, reviews, policies, webhooks, web UI, git bridge | ✅ |
| 4 | Knowledge and AI: notes, context packs, MCP server, SDKs | ✅ |
| 5 | Self-hosting, 1.0 format freeze, spec | 📋 |

Details per phase: [docs/ROADMAP.md](docs/ROADMAP.md).

## 📚 Documentation

| | |
| --- | --- |
| 🌐 **[Website](https://shadow-kernel.github.io/agit-site/)** | why agit, guide, large repositories, all features |
| 📖 [Handbook](docs/handbook/README.md) | 18 chapters: concepts, every CLI flag, MCP and HTTP references, SDKs, administration, internals, FAQ · [Überblick auf Deutsch](docs/handbook/de/UEBERBLICK.md) |
| 🔌 [HTTP API](docs/API.md) | the contract the CLI, the SDKs and the web UI follow |
| 🔐 [Security](docs/SECURITY.md) · [Deployment](docs/DEPLOYMENT.md) | threat model, hardening checklist, Docker, systemd, proxies |
| 🏗️ [Architecture](docs/ARCHITECTURE.md) · [RFCs](docs/rfcs/) | byte layouts and design decisions |

## 🗂️ Repository layout

```
crates/agit-core             object model, store, index, refs, merge, leases, events, workspaces
crates/agit-cli              the `agit` binary, including `agit mcp` and `agit auth`
crates/agit-protocol         wire types, HTTP client, push / fetch / clone, SSH-key sign-in
crates/agit-server           hosting server: repos, refs, PRs, policies, auth, knowledge, events
crates/agit-git-bridge       import from and export to git
web/                         web UI (Vite, React, TypeScript)
sdk/python, sdk/typescript   agent SDKs, zero dependencies
integrations/claude-code     hooks, MCP config, installer, self-test
docs/                        handbook, API, architecture, roadmap, RFCs
```

## 🤝 Contributing

Issues and pull requests are welcome. Run `make check` (fmt, clippy with warnings as errors, all tests) before you push; every command supports `--json`; on-disk format changes need an RFC. See [CONTRIBUTING.md](CONTRIBUTING.md) and the [Code of Conduct](CODE_OF_CONDUCT.md). Found a vulnerability? Please report it privately: [SECURITY.md](SECURITY.md).

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option. Unless you state otherwise, any contribution you submit for inclusion in agit is dual licensed as above, without additional terms.

<p align="center">
  <sub><a href="https://shadow-kernel.github.io/agit-site/">shadow-kernel.github.io/agit-site</a> · ⭐ Star the repo if agit is useful to you</sub>
</p>
