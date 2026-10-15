# agit web UI

Clean, GitHub-like web UI for agit repositories: code browsing, history, branches,
pull requests with structured reviews, live agent activity, the lease board and
the knowledge base. It talks **only** to the public HTTP API in
[`docs/API.md`](../docs/API.md); there are no private endpoints.

Status: 🚧 client-side SPA (Vite + React 18 + TypeScript + react-router). Server
side rendering from RFC 0006 is a later step; the page structure and API usage
stay the same.

## Run

```sh
cd web
pnpm install
pnpm dev          # http://localhost:5173, expects the server on :7878
pnpm build        # type-check (tsc -b) + production bundle in dist/
pnpm preview      # serve dist/
```

Point the UI at a server with `VITE_AGIT_API`, or change it at runtime in the
header's settings popover (⚙), where you can also set a bearer token and the
acting agent id.

### Environment variables

| variable | default | meaning |
| --- | --- | --- |
| `VITE_AGIT_API` | `http://localhost:7878/api/v1` | base URL of the agit server API |
| `VITE_AGIT_MOCK` | unset | `1` forces mock mode (fixture data, no server) |

Settings stored in `localStorage` (`agit.settings`): API base URL override,
bearer token (sent as `Authorization: Bearer …`), acting agent id (sent as
`X-Agit-Agent`).

### Mock mode

`pnpm dev` with `VITE_AGIT_MOCK=1`, or automatically whenever `GET /health`
fails at startup, the UI runs against `src/api/mock.ts`: two repositories,
~30 commits with agents, intents and tasks, four pull requests with reviews,
comments and checks, leases with expiries, eight knowledge notes, and a
simulated event stream. A small **mock data** badge appears in the header.
Mutations (reviews, merges, leases, notes) work in memory.

## Pages

| route | what |
| --- | --- |
| `/` | repository list |
| `/r/:repo` | repo home: tree of the default branch, latest commit, README, tabs |
| `/r/:repo/tree/:rev/*` | directory browser with breadcrumbs and file finder (`t`) |
| `/r/:repo/blob/:rev/*` | file view: line numbers, syntax highlighting, raw toggle, markdown preview, history link |
| `/r/:repo/commits?rev=&agent=&task=&path=` | history grouped by day with agent badges, intents, task chips, filters |
| `/r/:repo/commit/:id` | commit metadata, file list, unified diff |
| `/r/:repo/pulls` · `/pulls/:n` · `/pulls/new` | pull request list, detail (conversation, commits, files changed with inline comments, checks), creation form |
| `/r/:repo/agents` | agent table plus live event feed (SSE with polling fallback) |
| `/r/:repo/leases` | lease board with countdowns, renew/release, claim form |
| `/r/:repo/knowledge` · `/knowledge/:id` | notes with tag chips, search, path filter, context panel, note view, new note form |

## Architecture

```
web/
  index.html
  vite.config.ts
  src/
    main.tsx            entry, global styles
    App.tsx             router; pages are React.lazy chunks
    styles.css          single stylesheet, CSS variables, light + dark (prefers-color-scheme)
    api/
      types.ts          TypeScript types for every object in docs/API.md
      client.ts         typed fetch wrapper, error envelope -> ApiError, SSE/polling stream,
                        live/mock facade (`api`), `initApi()` health probe
      mock.ts           in-memory implementation of the same `Api` interface with fixtures
      settings.ts       localStorage settings (base URL, token, agent)
    lib/
      diff.ts           unified diff parser for the `patch` strings
      highlight.ts      lazy highlight.js, per-line HTML splitting
      markdown.ts       lazy marked + DOMPurify
      format.ts         time ago, short ids, byte sizes, language detection
      useAsync.ts       tiny data hooks (useAsync, useMutation, useNow)
    components/
      Layout.tsx        header (logo, breadcrumb, mock badge, settings popover), footer, toasts
      RepoShell.tsx     repo tabs (Code, Commits, Pull requests, Agents, Leases, Knowledge) + context
      TreeBrowser.tsx   tree table, breadcrumbs, latest commit bar, README, file finder
      BranchSelector.tsx, CommitList.tsx, CodeView.tsx, DiffView.tsx, Markdown.tsx,
      Toasts.tsx, ui.tsx (badges, skeletons, empty/error states)
    pages/              one file per route
```

Design rules: system font stack, 8px spacing grid, GitHub-like density, no UI
kit. Every request goes through `src/api/client.ts`; adding an endpoint means
adding it to `Api` (and to the mock) so the public-API audit stays trivial.

## Keyboard

- `t` focuses the file finder on code pages.
- `Esc` closes popovers and menus.

## Screenshots

_Placeholder: add `docs/screenshots/*.png` (repo home, commit diff, PR
conversation, agent activity, lease board, knowledge) once the server is up._
