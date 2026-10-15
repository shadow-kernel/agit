# 10. Web UI

The web UI in `web/` is a client-side SPA (Vite, React 18, TypeScript, react-router, no UI kit) that talks **only** to the public API in [docs/API.md](../API.md). Whatever the UI can show, an agent can query. Blame and code search, which the API does not offer as server-side search, are computed in the browser from public endpoints. Server-side rendering from RFC 0006 is a later step. Status: 🚧 (feature complete for the current API, verified with Playwright against a real server; SSR 📋).

Screenshots from mock mode live in `docs/screenshots/`:

| Screenshot | File |
| --- | --- |
| Repository home | [repo-home.png](../screenshots/repo-home.png) |
| Commits with agent, intent and task | [commits.png](../screenshots/commits.png) |
| Pull request | [pull-request.png](../screenshots/pull-request.png) |
| Agents | [agents.png](../screenshots/agents.png) |
| Knowledge base | [knowledge.png](../screenshots/knowledge.png) |

## Running it

```sh
cd web
pnpm install
pnpm dev          # http://localhost:5173, expects the API on :7878
pnpm build        # tsc -b + production bundle in dist/
pnpm preview
```

The simplest deployment is to let the server serve the bundle, with SPA fallback for deep links:

```sh
$ agit-server --data-dir ./agit-data --web-dir web/dist
$ curl -s -o /dev/null -w '%{http_code} %{content_type}\n' http://127.0.0.1:7878/
200 text/html
$ curl -s -o /dev/null -w '%{http_code} %{content_type}\n' http://127.0.0.1:7878/r/demo/pulls
200 text/html
```

Static files under `--web-dir` are served without the token check; only `/api/v1` is protected. The release ships the bundle as `agit-web-<version>.tar.gz`, the Docker image has it at `/opt/agit/web`.

| Variable | Default | Meaning |
| --- | --- | --- |
| `VITE_AGIT_API` | `http://localhost:7878/api/v1` in dev; `/api/v1` in the release and Docker builds | API base URL baked into the bundle |
| `VITE_AGIT_MOCK` | unset | `1` forces mock mode |

## Mock mode

With `VITE_AGIT_MOCK=1`, or automatically whenever `GET /health` fails at startup, the UI runs against `src/api/mock.ts`: two repositories, about 30 commits with agents, intents and tasks, four pull requests with reviews, comments and checks, leases with expiries, eight knowledge notes, a policy, two webhooks, tag rules and a simulated event stream. A **mock data** badge appears in the header. Mutations (reviews, merges, leases, notes, policy, webhooks, rules, repository deletion) work in memory, so the UI can be explored without a server. Blame and search derive from the mock history through the same `commits`/`tree` calls as the live client.

## Settings

The gear in the header opens a popover with three fields stored in `localStorage` under `agit.settings`: the API base URL (overrides `VITE_AGIT_API`), the bearer token (sent as `Authorization: Bearer …`) and the acting agent id (sent as `X-Agit-Agent`, so reviews and notes made from the browser are attributed). Anyone who can run script on the UI's origin can read the token; the UI uses no cookies, Markdown is rendered through DOMPurify, and there is no CSRF channel ([docs/SECURITY.md](../SECURITY.md)).

## Pages

| Route | What it shows | API calls |
| --- | --- | --- |
| `/` | repository list with default branch and head | `GET /repos` |
| `/r/:repo` | repo home: tree of the default branch, latest commit bar, README, tabs (Code, Commits, Pull requests, Agents, Leases, Knowledge, Settings) | `/repos/{repo}`, `/tree`, `/commits` |
| `/r/:repo/tree/:rev/*` | directory browser with breadcrumbs and a file finder (`t`); sizes come from tree entries, no blob reads | `/tree/{rev}?path=` |
| `/r/:repo/blob/:rev/*` | file view: line numbers and `#L<n>` links, syntax highlighting (highlight.js, lazy), raw toggle, Markdown preview, Code/Blame switch, history link | `/tree/{rev}?path=` |
| `/r/:repo/blame/:rev/*` | client-side blame: per line group the short id, agent badge (or author), relative age and message; the intent on hover; an age heat strip on the left; a banner when the 200-commit limit was hit | `/commits?path=&limit=200`, `/tree/{id}?path=` |
| `/r/:repo/commits?rev=&agent=&task=&path=` | history grouped by day with agent badges, intents and task chips; filters; "+50" incremental loading | `/commits` |
| `/r/:repo/commit/:id` | metadata (author, agent, model, run id, intent, task, signature), file list, unified diff, related notes | `/commits/{rev}` |
| `/r/:repo/compare/:base...:head` | diff between two revisions with the commits ahead, per-file stats and a "Create pull request" button that prefills `/pulls/new` | `/diff`, `/commits` |
| `/r/:repo/search?q=&tab=code\|commits\|notes&rev=` | repository search, see below | `/tree`, `/commits`, `/knowledge/notes?q=` |
| `/r/:repo/pulls`, `/pulls/:n`, `/pulls/new` | PR list (25 per page), detail with conversation, commits, files changed with inline comments by kind, checks, reviews and merge button, creation form | `/pulls…` |
| `/r/:repo/agents` | agent table (commits, last seen, open PRs, leases) and the live event feed | `/agents`, `/events`, `/events/stream` |
| `/r/:repo/leases` | lease board with countdowns, holder, mode, note; renew and release buttons; a claim form | `/leases…` |
| `/r/:repo/knowledge`, `/knowledge/:id` | notes with tag chips, text search, path filter, the per-path context panel, note view with staleness, new note form | `/knowledge/notes…`, `/context` |
| `/r/:repo/settings` | **Policy**: protected branches, PR/approval/check/intent/agent/signature requirements, allowed merge/push agents, max files per commit | `GET/PUT /policy` |
| `/r/:repo/settings/webhooks` | **Webhooks**: list with active/signed badges, create (URL, kind chips, secret), test, delete | `/webhooks…` |
| `/r/:repo/settings/rules` | **Knowledge rules**: pattern to tags editor | `GET/PUT /knowledge/rules` |
| `/r/:repo/settings/danger` | **Danger zone**: delete the repository with typed confirmation | `DELETE /repos/{repo}` |

Unknown routes under a repository redirect to its home; others show a not-found page. Light and dark themes follow `prefers-color-scheme`.

## Blame

There is no server-side blame *page* dependency: `src/lib/blame.ts` walks `GET /commits?rev=&path=&limit=200` for the file, fetches the blob at each commit (4 requests in flight) and attributes lines newest commit first with a Myers line diff. Lines that survive the whole walk belong to the oldest commit seen. Results are cached per `(repo, rev, path)` and blobs per `(repo, commit, path)`. The server also has `GET /blame/{rev}?path=` (used by the CLI and SDKs); the two agree on the first-parent semantics.

## Search

The header box (focus with `/`, Enter opens `/r/:repo/search?q=`) has three tabs:

- **Code**: lists the selected revision's tree recursively, fetches text blobs (at most 512 KB each, known binary extensions skipped, at most 2,000 files, 6 in flight) and greps them in the browser, with progress, skipped counts and a Cancel button. Matches are capped at 25 per file and 300 files and link to `blob/…#L<n>`. Options: match case, regex, path filter.
- **Commits**: the latest 500 commits of the revision filtered by message, intent, task, agent id or model and author.
- **Notes**: `GET /knowledge/notes?q=` (BM25 ranked).

For large repositories the Code tab is a convenience, not a code search engine; `agit grep` is faster locally.

## Compare

`/r/:repo/compare/<base>...<head>` shows the commits that `head` has over `base`, per-file added/removed counts and the unified diff, and a button that opens `/pulls/new` with source and target prefilled. The PR form takes title, body, intent and task, matching `POST /pulls`.

## Notifications

The bell in the header subscribes to the current repository's event stream (SSE with a polling fallback) and keeps the last 20 events with kind badges, actor, a one-line summary (`describeEvent()`) and a link target. Unread state lives in `localStorage` (`agit.notifications.<repo>`); "Mark all as read" and clicking an item update it. Polling stops on 401/403/404 rather than retrying a missing repository forever.

## Incremental loading

Commits (+50 via `limit`), pull requests (25 per page client-side), events (the server returns the last `limit` events; "Load older events" refetches with a larger limit) and notes (+50) load incrementally.

## Keyboard

- `t` focuses the file finder on code pages.
- `/` focuses the header search; Enter searches, `Esc` leaves the box.
- `Esc` closes popovers and menus.

## Architecture in brief

```
web/src/
  main.tsx, App.tsx       entry and router (pages are React.lazy chunks)
  styles.css              one stylesheet, CSS variables, light and dark
  api/types.ts            TypeScript types for every object in docs/API.md
  api/client.ts           typed fetch wrapper, error envelope -> ApiError, SSE/polling stream, live/mock facade, initApi() health probe
  api/mock.ts             in-memory implementation of the same Api interface with fixtures
  api/settings.ts         localStorage settings
  lib/                    blame.ts, search.ts, diff.ts (unified diff parser), highlight.ts, markdown.ts (marked + DOMPurify), format.ts, hooks
  components/             Layout, SearchBox, Notifications, RepoShell (tabs), TreeBrowser, BranchSelector, CommitList, CodeView, DiffView, Markdown, Toasts, ui
  pages/                  one file per route
```

Every request goes through `api/client.ts`; adding an endpoint means adding it to the `Api` interface and to the mock, which keeps the "public API only" audit trivial. Things the real server taught the UI (and that agents should know too): `ref.update` events carry `data.ref`, the server emits `repo.create`, `pull.comment`, `policy.update`, `webhook.*` and `knowledge.rules` events, `GET /events?limit=` returns the last N, `NoteSummary` has no `body`, and repository names may not contain `/`.

Details and the verification notes are in [web/README.md](../../web/README.md).
