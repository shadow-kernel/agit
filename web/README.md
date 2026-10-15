# agit web UI

Clean, GitHub-like web UI for agit repositories: code browsing, blame, history,
branches, compare view, pull requests with structured reviews, live agent
activity and notifications, the lease board, the knowledge base, repository
search and settings (policy, webhooks, knowledge rules). It talks **only** to
the public HTTP API in [`docs/API.md`](../docs/API.md); there are no private
endpoints. Features the API does not offer server-side (blame, code search)
are computed in the browser from public endpoints.

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
acting agent id. The server can also serve the built UI itself:
`agit-server --web-dir web/dist` (SPA fallback included), which is how the
Playwright pass below runs it.

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
comments and checks, leases with expiries, eight knowledge notes, a policy,
two webhooks, tag rules and a simulated event stream. A small **mock data**
badge appears in the header. Mutations (reviews, merges, leases, notes, policy,
webhooks, rules, repository deletion) work in memory. Blame and code search
need no extra fixtures: they derive from the mock history through the same
`commits`/`tree` calls the live client uses, and the mock resolves `rev~N`.

## Pages

| route | what |
| --- | --- |
| `/` | repository list |
| `/r/:repo` | repo home: tree of the default branch, latest commit, README, tabs |
| `/r/:repo/tree/:rev/*` | directory browser with breadcrumbs and file finder (`t`) |
| `/r/:repo/blob/:rev/*` | file view: line numbers, syntax highlighting, raw toggle, markdown preview, Code/Blame switch, history link |
| `/r/:repo/blame/:rev/*` | client-side blame: commit, agent/author, age and message per line group, intent on hover, age heat strip |
| `/r/:repo/commits?rev=&agent=&task=&path=` | history grouped by day with agent badges, intents, task chips, filters |
| `/r/:repo/commit/:id` | commit metadata, file list, unified diff |
| `/r/:repo/compare/:base...:head` | diff between two revisions with commits ahead, per-file stats and a "Create pull request" button that prefills `/pulls/new` |
| `/r/:repo/search?q=&tab=code\|commits\|notes&rev=` | repository search (see below) |
| `/r/:repo/pulls` · `/pulls/:n` · `/pulls/new` | pull request list, detail (conversation, commits, files changed with inline comments, checks), creation form |
| `/r/:repo/agents` | agent table plus live event feed (SSE with polling fallback) |
| `/r/:repo/leases` | lease board with countdowns, renew/release, claim form |
| `/r/:repo/knowledge` · `/knowledge/:id` | notes with tag chips, search, path filter, context panel, note view, new note form |
| `/r/:repo/settings` | **Policy**: protected branches, PR/approval/check/intent/agent/signature requirements, allowed merge/push agents, max files per commit (`GET/PUT /policy`) |
| `/r/:repo/settings/webhooks` | **Webhooks**: list with active/signed badges, create (url, kind chips, secret), test (`POST /webhooks/{id}/test`), delete |
| `/r/:repo/settings/rules` | **Knowledge rules**: pattern → tags editor (`GET/PUT /knowledge/rules`) |
| `/r/:repo/settings/danger` | **Danger zone**: delete the repository with typed confirmation |

### Blame

There is no server blame endpoint. `src/lib/blame.ts` walks
`GET /commits?rev=&path=&limit=200` for the file, fetches the blob at each
commit via `GET /tree/{id}?path=` (4 in flight), and attributes lines newest
commit first with a Myers line diff; lines that survive the whole walk belong
to the oldest commit seen (a banner says so when the 200-commit limit was
hit). Results are cached per `(repo, rev, path)`, blobs per `(repo, commit,
path)`. Consecutive lines from one commit form a group with the short id,
agent badge (or author), relative date and message; the gutter tooltip shows
the intent; the left strip fades with age.

### Search

The header box (focus with `/`, Enter opens `/r/:repo/search?q=`) has three
tabs:

- **Code**: `src/lib/search.ts` lists the selected revision's tree
  recursively, then fetches text blobs (≤ 512 KB, known binary extensions
  skipped, at most 2000 files, 6 in flight) and greps them in the browser.
  Progress, skipped counts and a **Cancel** button are shown; matches are
  capped at 25 per file / 300 files and link to `blob/…#L<n>`. Options:
  match case, regex, path filter. Tree listings and blobs are cached.
- **Commits**: the latest 500 commits of the revision filtered by message,
  intent, task, agent id/model and author.
- **Notes**: `GET /knowledge/notes?q=`.

### Notifications

The bell in the header subscribes to the current repository's event stream
(SSE, polling fallback) and keeps the last 20 events with kind badges, actor,
a one-line summary and a link target. Unread state lives in
`localStorage` (`agit.notifications.<repo>`: `upTo` sequence plus individually
read seqs); "Mark all as read" and clicking an item update it. Polling stops on
401/403/404 instead of retrying a missing repository forever.

### Incremental loading

Commits (+50 via `limit`), pull requests (25 per page client-side), events
(the server returns the last `limit` events; "Load older events" refetches with
a larger limit) and notes (+50 via `limit`) load incrementally.

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
      blame.ts          client-side blame: history walk + Myers line diff, caches
      search.ts         recursive tree listing + blob grep with progress/cancel, caches
      useDismiss.ts     close popovers on outside click / Escape
      diff.ts           unified diff parser for the `patch` strings
      highlight.ts      lazy highlight.js, per-line HTML splitting
      markdown.ts       lazy marked + DOMPurify
      format.ts         time ago, short ids, byte sizes, language detection
      useAsync.ts       tiny data hooks (useAsync, useMutation, useNow)
    components/
      Layout.tsx        header (logo, breadcrumb, search box, mock badge, bell, settings popover), footer, toasts
      SearchBox.tsx     header search input with the `/` shortcut
      Notifications.tsx bell, dropdown, unread state, `describeEvent()` summaries
      RepoShell.tsx     repo tabs (Code, Commits, Pull requests, Agents, Leases, Knowledge, Settings) + context
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
- `/` focuses the header search; Enter searches, `Esc` leaves the box.
- `Esc` closes popovers and menus.

## Verified against the real server

The UI was driven with headless Chromium (Playwright) against
`agit-server --data-dir … --web-dir web/dist` holding a repository pushed with
the CLI (five commits by four agents, a PR with a comment and an approval, a
second branch, two notes): every route above, blame on a four-commit file,
search (all tabs, `/` shortcut), compare → prefilled PR form, notifications
(SSE delivery, mark-all-read persisted), policy save/reload, webhook
create/test/delete, rules save, repository deletion, light and dark mode, with
zero console errors apart from the browser's own log line for an intentional
404. Things learned from the real responses and now reflected here: `ref.update`
events carry `data.ref` (not `name`), the server emits `repo.create`,
`pull.comment`, `policy.update`, `webhook.*` and `knowledge.rules` events,
`GET /events?limit=` returns the *last* N events, the policy has a
`require_signed` field, `NoteSummary` has no `body`/`commits`/`pulls`/`created_ms`,
tree responses carry an `id`, `GET /context` returns `path` and
`suggested_tags`, and repository names may not contain `/`.

## Screenshots

_Placeholder: add `docs/screenshots/*.png` (repo home, commit diff, PR
conversation, agent activity, lease board, knowledge) once the server is up._
